"""Reproducible source retrieval evaluation; no generated claims of AI quality."""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import platform
import statistics
import tempfile
import time
from .embeddings import add_arguments, from_arguments


def peak_rss_bytes():
    if platform.system() == "Windows":
        import ctypes
        from ctypes import wintypes
        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in (
                    "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
                    "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage",
                    "PagefileUsage", "PeakPagefileUsage")]
        info = Counters()
        info.cb = ctypes.sizeof(info)
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = wintypes.HANDLE
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        if not psapi.GetProcessMemoryInfo(kernel.GetCurrentProcess(), ctypes.byref(info), info.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return info.PeakWorkingSetSize
    import resource
    value = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return value if platform.system() == "Darwin" else value * 1024


def percentile(values, quantile):
    return sorted(values)[min(len(values) - 1, math.ceil(quantile * len(values)) - 1)]


def evaluate(dataset_path, output, embedder, k=5, embeddings_path=None, retrieval_mode="hybrid"):
    import tapirus
    if not 1 <= k <= 128:
        raise ValueError("Require 1 <= k <= 128")
    if retrieval_mode not in ("hybrid", "vector", "lexical"):
        raise ValueError("Invalid retrieval mode")
    raw = Path(dataset_path).read_bytes()
    dataset = json.loads(raw)
    documents, queries = dataset["documents"], dataset["queries"]
    if not documents or not queries:
        raise ValueError("Evaluation requires documents and queries")
    ids = {d["id"] for d in documents}
    if len(ids) != len(documents) or len({q["id"] for q in queries}) != len(queries):
        raise ValueError("Duplicate document or query IDs")
    for query in queries:
        if not query["relevant"] or not set(query["relevant"]).issubset(ids):
            raise ValueError("Every query requires known relevant source IDs")
    started = time.perf_counter()
    external_sha = None
    if embeddings_path:
        external_raw = Path(embeddings_path).read_bytes()
        external_sha = hashlib.sha256(external_raw).hexdigest()
        external = json.loads(external_raw)
        if external.get("dataset_sha256", hashlib.sha256(raw).hexdigest()) != hashlib.sha256(raw).hexdigest():
            raise ValueError("External embeddings were generated for a different dataset snapshot")
        model_id = external["model_id"]
        document_vectors = [external["documents"][d["id"]] for d in documents]
        query_vectors = [external["queries"][q["id"]] for q in queries]
        dim = external["dimensions"]
        if not model_id or not 1 <= dim <= 4096:
            raise ValueError("Invalid external model identity/dimensions")
        for vector in document_vectors + query_vectors:
            if (len(vector) != dim or any(type(v) not in (int, float) or not math.isfinite(v) for v in vector)
                    or not any(vector)):
                raise ValueError("Invalid external embedding")
        backend = "external"
    else:
        model_id, backend = embedder.identity, embedder.backend
        document_vectors, query_vectors = [], []
        for destination, items in ((document_vectors, documents), (query_vectors, queries)):
            for offset in range(0, len(items), 64):
                destination.extend(embedder.embed([item["content" if destination is document_vectors else "query"]
                                                  for item in items[offset:offset + 64]], query=destination is query_vectors))
    embedding_seconds = time.perf_counter() - started
    samples, by_language = [], defaultdict(list)
    with tempfile.TemporaryDirectory(prefix="tapirus-eval-") as directory:
        database = Path(directory) / "eval.tapir"
        started = time.perf_counter()
        with tapirus.connect(str(database)) as connection:
            for offset in range(0, len(documents), 256):
                batch = []
                for doc, vector in zip(documents[offset:offset + 256], document_vectors[offset:offset + 256]):
                    item = {"external_id": doc["id"], "content": doc["content"], "namespace": "eval"}
                    if vector is not None:
                        item.update(embedding=vector, embedding_model=model_id)
                    batch.append(item)
                connection.upsert_memory_batch(batch)
        ingestion_seconds = time.perf_counter() - started
        started = time.perf_counter()
        with tapirus.connect(str(database)) as connection:
            reopen_seconds = time.perf_counter() - started
            started = time.perf_counter()
            stats = connection.prepare_memory_indexes()
            index_seconds = time.perf_counter() - started
            for query, vector in zip(queries, query_vectors):
                request = {"query": query["query"], "namespace": "eval", "limit": k,
                           "recency_weight": 0, "importance_weight": 0}
                if vector is not None:
                    request.update(embedding=vector, embedding_model=model_id)
                if retrieval_mode == "vector":
                    if vector is None:
                        raise ValueError("Vector retrieval requires an embedding backend or external vectors")
                    request.pop("query")
                    request.update(vector_weight=1, lexical_weight=0)
                elif retrieval_mode == "lexical":
                    request.pop("embedding", None)
                    request.pop("embedding_model", None)
                started = time.perf_counter_ns()
                response = connection.search_memories_detailed(request)
                latency_ms = (time.perf_counter_ns() - started) / 1e6
                ranked = [r["entry"]["external_id"] for r in response["results"]]
                relevant = set(query["relevant"])
                hits = [i + 1 for i, source in enumerate(ranked) if source in relevant]
                dcg = sum(1 / math.log2(rank + 1) for rank in hits)
                ideal = sum(1 / math.log2(i + 2) for i in range(min(k, len(relevant))))
                sample = {"id": query["id"], "language": query.get("language", "unspecified"),
                          "ranked_ids": ranked, "recall_at_k": len(hits) / len(relevant),
                          "reciprocal_rank": 1 / hits[0] if hits else 0,
                          "ndcg_at_k": dcg / ideal, "latency_ms": latency_ms,
                          "diagnostics": response["diagnostics"]}
                samples.append(sample)
                by_language[sample["language"]].append(sample)
            size = database.stat().st_size
    def aggregate(items):
        return {"queries": len(items), "recall_at_k": statistics.mean(s["recall_at_k"] for s in items),
                "mrr": statistics.mean(s["reciprocal_rank"] for s in items),
                "ndcg_at_k": statistics.mean(s["ndcg_at_k"] for s in items),
                "misses": [s["id"] for s in items if s["reciprocal_rank"] == 0]}
    report = {"dataset": dataset.get("name"), "dataset_scope": dataset.get("scope"),
              "dataset_sha256": hashlib.sha256(raw).hexdigest(), "embeddings_sha256": external_sha,
              "engine_version": tapirus.__version__, "backend": backend, "embedding_model": model_id,
              "retrieval_mode": retrieval_mode,
              "k": k, "documents": len(documents), "metrics": aggregate(samples),
              "by_language": {lang: aggregate(items) for lang, items in by_language.items()},
              "latency_ms": {f"p{int(q * 100)}": percentile([s["latency_ms"] for s in samples], q)
                             for q in (0.5, 0.95, 0.99)},
              "embedding_seconds": embedding_seconds,
              "truncated_model_inputs": getattr(embedder.local, "truncated_inputs", 0),
              "ingestion_seconds": ingestion_seconds,
              "reopen_seconds": reopen_seconds, "index_build_seconds": index_seconds,
              "peak_process_rss_bytes": peak_rss_bytes(), "database_bytes": size, "memory_stats": stats,
              "host": {"system": platform.system(), "machine": platform.machine(), "python": platform.python_version()},
              "samples": samples}
    Path(output).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dataset", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--k", type=int, default=5)
    parser.add_argument("--embeddings", type=Path, help="External model_id/dimensions/documents/queries JSON")
    parser.add_argument("--retrieval-mode", choices=["hybrid", "vector", "lexical"], default="hybrid")
    add_arguments(parser)
    args = parser.parse_args(argv)
    report = evaluate(args.dataset, args.output, from_arguments(args), args.k, args.embeddings, args.retrieval_mode)
    print(json.dumps({key: report[key] for key in ("backend", "embedding_model", "metrics", "by_language", "latency_ms")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
