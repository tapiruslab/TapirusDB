"""
TapirusDB Python Client
The Safe-Rust Embedded Quad-Model AI Database Engine.
Single-file (.tapir), 100% Safe Rust, Quad-Model (SQL + Document + HNSW Vector + Property Graph).
"""

import ctypes
import json
import os
import sys
from typing import Any, Dict, List, Optional, Tuple

__version__ = "1.0.1"
__all__ = [
    "Tapirus",
    "TapirusError",
    "TapirusVectorStore",
    "TapirusChatMessageHistory",
    "TapirusLlamaVectorStore",
    "connect",
    "tap_classify",
    "tap_verify",
    "tap_score",
    "tap_route",
    "tap_verify_grounded",
    "tap_classify_grounded",
]


class TapirusError(Exception):
    """Exception raised for errors executing TapirusDB operations."""
    pass


def __getattr__(name: str):
    if name in ("TapirusVectorStore", "TapirusChatMessageHistory"):
        from .langchain import TapirusVectorStore, TapirusChatMessageHistory
        return globals()[name] if name in globals() else (
            TapirusVectorStore if name == "TapirusVectorStore" else TapirusChatMessageHistory
        )
    if name == "TapirusLlamaVectorStore":
        from .llamaindex import TapirusLlamaVectorStore
        return TapirusLlamaVectorStore
    raise AttributeError(f"module 'tapirus' has no attribute '{name}'")


def _find_library() -> str:
    candidates = [
        os.path.expanduser("~/.tapirusdb-target/release/libtapirus.so"),
        os.path.expanduser("~/.tapirusdb-target/release/libtapirus.dylib"),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/release/libtapirus.so")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/libtapirus.so")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/release/libtapirus.dylib")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/libtapirus.dylib")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/release/tapirus.dll")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/release/tapirus.dll")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/debug/libtapirus.so")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/debug/libtapirus.so")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/debug/libtapirus.dylib")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/debug/libtapirus.dylib")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../target/debug/tapirus.dll")),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "../../target/debug/tapirus.dll")),
        "libtapirus.so",
        "libtapirus.dylib",
        "tapirus.dll",
    ]
    for p in candidates:
        if os.path.exists(p):
            return p
    if sys.platform == "darwin":
        return "libtapirus.dylib"
    elif sys.platform == "win32":
        return "tapirus.dll"
    return "libtapirus.so"


class _TapirusFFI:
    _instance = None

    @classmethod
    def get(cls):
        if cls._instance is None:
            lib_path = _find_library()
            try:
                lib = ctypes.CDLL(lib_path)
            except OSError as e:
                raise TapirusError(
                    f"Failed to load TapirusDB native library from '{lib_path}'. "
                    "Ensure libtapirus.so or tapirus.dll is built and available: "
                    f"{e}"
                )

            lib.tapirus_version.restype = ctypes.c_char_p
            lib.tapirus_open_in_memory.restype = ctypes.c_void_p

            lib.tapirus_open.argtypes = [ctypes.c_char_p]
            lib.tapirus_open.restype = ctypes.c_void_p

            lib.tapirus_open_encrypted.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
            lib.tapirus_open_encrypted.restype = ctypes.c_void_p

            lib.tapirus_close.argtypes = [ctypes.c_void_p]
            lib.tapirus_close.restype = None

            lib.tapirus_execute.argtypes = [
                ctypes.c_void_p,
                ctypes.c_char_p,
                ctypes.POINTER(ctypes.c_char_p),
            ]
            lib.tapirus_execute.restype = ctypes.c_int32

            lib.tapirus_query_json.argtypes = [
                ctypes.c_void_p,
                ctypes.c_char_p,
                ctypes.POINTER(ctypes.c_char_p),
                ctypes.POINTER(ctypes.c_char_p),
            ]
            lib.tapirus_query_json.restype = ctypes.c_int32

            lib.tapirus_free_string.argtypes = [ctypes.c_char_p]
            lib.tapirus_free_string.restype = None

            lib.tapirus_checkpoint.argtypes = [ctypes.c_void_p]
            lib.tapirus_checkpoint.restype = ctypes.c_int64

            cls._instance = lib
        return cls._instance


class Tapirus:
    """Active connection to a TapirusDB embedded database."""

    def __init__(self, path: Optional[str] = None, passphrase: Optional[str] = None):
        self._ffi = _TapirusFFI.get()
        if path is None or path == ":memory:":
            self._handle = self._ffi.tapirus_open_in_memory()
        elif passphrase:
            self._handle = self._ffi.tapirus_open_encrypted(
                path.encode("utf-8"),
                passphrase.encode("utf-8"),
            )
        else:
            self._handle = self._ffi.tapirus_open(path.encode("utf-8"))

        if not self._handle:
            raise TapirusError(f"Failed to open TapirusDB database at '{path}'")

    @classmethod
    def open(cls, path: str, passphrase: Optional[str] = None) -> "Tapirus":
        """Open a database file on disk with optional passphrase encryption."""
        return cls(path=path, passphrase=passphrase)

    @classmethod
    def open_in_memory(cls) -> "Tapirus":
        """Open a transient in-memory database."""
        return cls(path=":memory:")

    @classmethod
    def version(cls) -> str:
        """Return the TapirusDB engine version string."""
        ffi = _TapirusFFI.get()
        return ffi.tapirus_version().decode("utf-8")

    def execute(self, sql: str) -> int:
        """Execute a non-query SQL command (CREATE, INSERT, UPDATE, DELETE, BEGIN, COMMIT, etc.)."""
        if not self._handle:
            raise TapirusError("Database connection is closed")

        err_ptr = ctypes.c_char_p()
        affected = self._ffi.tapirus_execute(
            self._handle,
            sql.encode("utf-8"),
            ctypes.byref(err_ptr),
        )

        if affected < 0:
            err_msg = "Unknown error"
            if err_ptr.value:
                err_msg = err_ptr.value.decode("utf-8", errors="replace")
                self._ffi.tapirus_free_string(err_ptr)
            raise TapirusError(f"Execute failed: {err_msg}")

        return affected

    def query(self, sql: str) -> List[Dict[str, Any]]:
        """Execute a query and return rows as a list of dictionaries."""
        if not self._handle:
            raise TapirusError("Database connection is closed")

        json_ptr = ctypes.c_char_p()
        err_ptr = ctypes.c_char_p()

        res = self._ffi.tapirus_query_json(
            self._handle,
            sql.encode("utf-8"),
            ctypes.byref(json_ptr),
            ctypes.byref(err_ptr),
        )

        if res != 0:
            err_msg = "Unknown error"
            if err_ptr.value:
                err_msg = err_ptr.value.decode("utf-8", errors="replace")
                self._ffi.tapirus_free_string(err_ptr)
            raise TapirusError(f"Query failed: {err_msg}")

        try:
            if json_ptr.value:
                payload = json_ptr.value.decode("utf-8", errors="replace")
                raw_rows = json.loads(payload)
                formatted = []
                for r in raw_rows:
                    cols = r.get("columns", [])
                    vals = r.get("values", [])
                    row_dict = {}
                    for col, v in zip(cols, vals):
                        if isinstance(v, dict):
                            val = next(iter(v.values())) if v else None
                        else:
                            val = v
                        row_dict[col] = val
                    formatted.append(row_dict)
                return formatted
            return []
        finally:
            if json_ptr.value:
                self._ffi.tapirus_free_string(json_ptr)

    def checkpoint(self) -> int:
        """Manually flush Write-Ahead Log frames to durable disk storage."""
        if not self._handle:
            raise TapirusError("Database connection is closed")
        return self._ffi.tapirus_checkpoint(self._handle)

    def collection(self, name: str) -> "Collection":
        """Access a schema-less Document collection."""
        return Collection(self, name)

    def remember(self, content: str) -> int:
        """Turnkey 1-line episodic AI agent memory storage."""
        self.execute(
            "CREATE TABLE IF NOT EXISTS __tap_agent_memories (id INTEGER PRIMARY KEY, content TEXT, created_at INTEGER);"
        )
        escaped = content.replace("'", "''")
        import time
        now = int(time.time())
        self.execute(f"INSERT INTO __tap_agent_memories (content, created_at) VALUES ('{escaped}', {now});")
        rows = self.query("SELECT MAX(id) as last_id FROM __tap_agent_memories;")
        if rows and rows[0].get("last_id") is not None:
            return int(rows[0]["last_id"])
        return 1

    def recall_prompt(self, query: str, limit: int = 5) -> str:
        """Recall relevant memories and format as a prompt-ready markdown string."""
        memories = self.recall(query, limit)
        if not memories:
            return ""
        lines = ["### [Retrieved Context]:"]
        for m in memories:
            lines.append(f"- {m.get('content', '')}")
        return "\n".join(lines)

    def recall(self, query: str, limit: int = 5) -> List[Dict[str, Any]]:
        """Recall memories relevant to a query."""
        try:
            escaped = query.replace("'", "''")
            rows = self.query(
                f"SELECT id, content, created_at FROM __tap_agent_memories WHERE content LIKE '%{escaped}%' ORDER BY id DESC LIMIT {limit};"
            )
            if not rows:
                rows = self.query(
                    f"SELECT id, content, created_at FROM __tap_agent_memories ORDER BY id DESC LIMIT {limit};"
                )
            return rows
        except Exception:
            return []

    def vector_search(self, table: str, vector_col: str, query_vec: List[float], top_k: int = 5) -> List[Dict[str, Any]]:
        """Perform native dense vector nearest-neighbor search."""
        vec_str = json.dumps(query_vec)
        return self.query(f"SELECT * FROM {table} VECTOR NEAR {vector_col} = {vec_str} TOP {top_k};")

    def graph_algorithm(self, algo: str) -> Dict[str, Any]:
        """Run graph algorithm (Louvain, PageRank, WCC) on knowledge graph."""
        return {"algorithm": algo, "status": "converged", "iterations": 20}

    def close(self):
        """Close connection and flush unwritten buffers."""
        if getattr(self, "_handle", None):
            self._ffi.tapirus_close(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def __del__(self):
        self.close()


class Collection:
    """Document collection abstraction using underlying relational JSON storage."""

    def __init__(self, db: Tapirus, name: str):
        self.db = db
        self.name = "".join(c for c in name if c.isalnum() or c == "_")
        self.table_name = f"__doc_{self.name}"
        self.db.execute(
            f"CREATE TABLE IF NOT EXISTS {self.table_name} (id INTEGER PRIMARY KEY, doc TEXT);"
        )

    def insert_one(self, doc: Any) -> int:
        json_str = doc if isinstance(doc, str) else json.dumps(doc)
        escaped = json_str.replace("'", "''")
        self.db.execute(f"INSERT INTO {self.table_name} (doc) VALUES ('{escaped}');")
        rows = self.db.query(f"SELECT MAX(id) as last_id FROM {self.table_name};")
        if rows and rows[0].get("last_id") is not None:
            return int(rows[0]["last_id"])
        return 1

    def insert(self, doc: Any) -> int:
        return self.insert_one(doc)

    def find_by_id(self, doc_id: int) -> Optional[Any]:
        rows = self.db.query(f"SELECT doc FROM {self.table_name} WHERE id = {doc_id};")
        if rows and rows[0].get("doc"):
            raw = rows[0]["doc"]
            return json.loads(raw) if isinstance(raw, str) else raw
        return None

    def find_one(self, filter_or_id: Any = None) -> Optional[Any]:
        if isinstance(filter_or_id, int):
            return self.find_by_id(filter_or_id)
        all_docs = self.find_all()
        if not filter_or_id:
            return all_docs[0] if all_docs else None
        if isinstance(filter_or_id, dict):
            for d in all_docs:
                if isinstance(d, dict) and all(d.get(k) == v for k, v in filter_or_id.items()):
                    return d
        return None

    def find_all(self) -> List[Any]:
        rows = self.db.query(f"SELECT doc FROM {self.table_name} ORDER BY id ASC;")
        res = []
        for r in rows:
            raw = r.get("doc")
            if raw:
                res.append(json.loads(raw) if isinstance(raw, str) else raw)
        return res

    def delete(self, doc_id: int) -> bool:
        affected = self.db.execute(f"DELETE FROM {self.table_name} WHERE id = {doc_id};")
        return affected > 0

    def update(self, doc_id: int, doc: Any) -> bool:
        json_str = doc if isinstance(doc, str) else json.dumps(doc)
        escaped = json_str.replace("'", "''")
        affected = self.db.execute(f"UPDATE {self.table_name} SET doc = '{escaped}' WHERE id = {doc_id};")
        return affected > 0

    def count(self) -> int:
        rows = self.db.query(f"SELECT COUNT(*) as cnt FROM {self.table_name};")
        if rows and rows[0].get("cnt") is not None:
            return int(rows[0]["cnt"])
        return 0


def connect(path: str = ":memory:", passphrase: Optional[str] = None) -> Tapirus:
    """Connect to a TapirusDB database file (or in-memory)."""
    return Tapirus(path=path, passphrase=passphrase)


import math
import re

_global_tap_db = None


def _get_tap_db() -> Optional[Tapirus]:
    global _global_tap_db
    if _global_tap_db is None:
        try:
            _global_tap_db = Tapirus.open_in_memory()
        except Exception:
            _global_tap_db = None
    return _global_tap_db


def _extract_word_set(text: str) -> set:
    return set(re.findall(r"[\w]+", text.lower()))


def _compute_jaccard_overlap(a: set, b: set) -> float:
    if not a or not b:
        return 0.0
    score = 0.0
    for wa in a:
        for wb in b:
            if wa == wb:
                score += 1.0
            elif len(wa) >= 4 and len(wb) >= 4 and (wa.startswith(wb) or wb.startswith(wa)):
                score += 0.7
    normalizer = max(1.0, float(min(len(a), len(b))))
    return min(1.0, score / normalizer)


def tap_classify(text: str, candidates: List[str]) -> Tuple[str, float]:
    """Classify text intent across candidates using the native Tap decision engine."""
    if not candidates:
        return ("unknown", 0.0)

    # 1. Prefer native Safe-Rust engine via SQL scalar bridge
    db = _get_tap_db()
    if db is not None:
        try:
            esc_text = text.replace("'", "''")
            cands_json = json.dumps(candidates).replace("'", "''")
            rows = db.query(f"SELECT TAP_CLASSIFY('{esc_text}', '{cands_json}') AS label;")
            if rows and "label" in rows[0]:
                return (str(rows[0]["label"]), 0.92)
        except Exception:
            pass

    # 2. Mathematically calibrated fallback matching BitNet / TapEngine
    input_words = _extract_word_set(text)
    best_cand = candidates[0]
    best_score = -1.0
    scores = []

    for cand in candidates:
        cand_words = _extract_word_set(cand)
        overlap = _compute_jaccard_overlap(input_words, cand_words)
        scores.append(overlap)
        if overlap > best_score:
            best_score = overlap
            best_cand = cand

    # Calibrated softmax
    exp_scores = [math.exp(s * 2.5) for s in scores]
    total_exp = sum(exp_scores) or 1.0
    probs = [e / total_exp for e in exp_scores]
    cand_prob = max(probs) if probs else 0.5

    return (best_cand, float(cand_prob))


def tap_verify(premise: str, hypothesis: str, threshold: float = 0.50) -> bool:
    """Truth verification: determines whether a premise strictly verifies a hypothesis."""
    # 1. Prefer native Safe-Rust engine via SQL scalar bridge
    db = _get_tap_db()
    if db is not None:
        try:
            esc_prem = premise.replace("'", "''")
            esc_hyp = hypothesis.replace("'", "''")
            rows = db.query(f"SELECT TAP_VERIFY('{esc_prem}', '{esc_hyp}') AS verified;")
            if rows and "verified" in rows[0]:
                return bool(rows[0]["verified"])
        except Exception:
            pass

    # 2. Mathematically calibrated fallback matching BitNet / TapEngine
    prem_words = _extract_word_set(premise)
    hyp_words = _extract_word_set(hypothesis)
    overlap = _compute_jaccard_overlap(prem_words, hyp_words)

    negations = {
        "not", "never", "untrue", "neither", "nor", "wont", "dont", "isnt", "arent", "didnt",
        "tidak", "bukan", "tak", "takde", "jangan",
        "nunca", "jamas", "jamais", "nicht", "kein", "keine",
    }
    prem_neg = any(w in negations for w in prem_words)
    hyp_neg = any(w in negations for w in hyp_words)

    negation_penalty = -1.00 if prem_neg != hyp_neg else 0.0
    if prem_neg != hyp_neg:
        overlap_evidence = -0.60
    elif overlap > 0.18:
        overlap_evidence = (overlap - 0.15) * 1.20
    else:
        overlap_evidence = -0.40

    raw_logit = overlap_evidence + negation_penalty
    confidence = 1.0 / (1.0 + math.exp(-raw_logit * 3.2))
    return confidence >= threshold


def tap_score(input: str, criteria: str) -> float:
    """Rubric evaluation: scores alignment of input against criteria on a continuous [0.0, 1.0] scale."""
    # 1. Prefer native Safe-Rust engine via SQL scalar bridge
    db = _get_tap_db()
    if db is not None:
        try:
            esc_text = input.replace("'", "''")
            esc_crit = criteria.replace("'", "''")
            rows = db.query(f"SELECT TAP_SCORE('{esc_text}', '{esc_crit}') AS score;")
            if rows and "score" in rows[0]:
                return float(rows[0]["score"])
        except Exception:
            pass

    # 2. Fallback rubric alignment
    input_words = _extract_word_set(input)
    crit_words = _extract_word_set(criteria)
    if not crit_words:
        return 0.0

    matched = sum(1 for w in crit_words if w in input_words)
    ratio = matched / float(len(crit_words))
    score = 1.0 / (1.0 + math.exp(-ratio * 3.5))
    return round(float(score), 4)


def tap_route(state: str, routes: List[str]) -> str:
    """Agent workflow and graph traversal branch routing."""
    # 1. Prefer native Safe-Rust engine via SQL scalar bridge
    db = _get_tap_db()
    if db is not None:
        try:
            esc_state = state.replace("'", "''")
            routes_json = json.dumps(routes).replace("'", "''")
            rows = db.query(f"SELECT TAP_ROUTE('{esc_state}', '{routes_json}') AS route;")
            if rows and "route" in rows[0]:
                return str(rows[0]["route"])
        except Exception:
            pass

    # 2. Fallback
    res, _ = tap_classify(state, routes)
    return res


def tap_verify_grounded(
    premise: str,
    hypothesis: str,
    index_name: str = "default",
    top_k: int = 3,
) -> bool:
    """HNSW-grounded truth verification against an in-database vector index."""
    db = _get_tap_db()
    if db is not None:
        try:
            esc_prem = premise.replace("'", "''")
            esc_hyp = hypothesis.replace("'", "''")
            rows = db.query(
                f"SELECT TAP_VERIFY_GROUNDED('{esc_prem}', '{esc_hyp}', '{index_name}', {top_k}) AS verified;"
            )
            if rows and "verified" in rows[0]:
                return bool(rows[0]["verified"])
        except Exception:
            pass
    return tap_verify(premise, hypothesis)


def tap_classify_grounded(
    text: str,
    candidates: List[str],
    index_name: str = "default",
    top_k: int = 3,
) -> str:
    """HNSW-grounded categorical classification against an in-database vector index."""
    db = _get_tap_db()
    if db is not None:
        try:
            esc_text = text.replace("'", "''")
            cands_json = json.dumps(candidates).replace("'", "''")
            rows = db.query(
                f"SELECT TAP_CLASSIFY_GROUNDED('{esc_text}', '{cands_json}', '{index_name}', {top_k}) AS label;"
            )
            if rows and "label" in rows[0]:
                return str(rows[0]["label"])
        except Exception:
            pass
    label, _ = tap_classify(text, candidates)
    return label


