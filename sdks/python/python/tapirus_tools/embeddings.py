"""Explicit lexical, lexical hashing and Ollama providers; no silent fallback."""
import hashlib
import json
import math
import re
from urllib.parse import urlsplit
from urllib.request import Request, build_opener, HTTPRedirectHandler


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def post_json(base_url, path, payload, timeout=120):
    parts = urlsplit(base_url)
    if parts.scheme not in ("http", "https") or not parts.hostname or parts.username or parts.password or parts.query or parts.fragment:
        raise ValueError("Expected an HTTP(S) service URL without credentials or query")
    request = Request(base_url.rstrip("/") + path,
                      json.dumps(payload, allow_nan=False).encode(),
                      {"Content-Type": "application/json"}, method="POST")
    with build_opener(NoRedirect()).open(request, timeout=timeout) as response:
        if response.status != 200:
            raise ValueError("Provider did not return HTTP 200")
        body = response.read(16 * 1024 * 1024 + 1)
    if len(body) > 16 * 1024 * 1024:
        raise ValueError("Provider response exceeds 16 MiB")
    return json.loads(body)


class Embedder:
    def __init__(self, backend="lexical", model=None, dimensions=384,
                 url="http://127.0.0.1:11434", model_id=None, model_path=None,
                 truncate_model_inputs=False):
        if backend not in ("lexical", "hash", "ollama", "onnx"):
            raise ValueError("Unknown embedding backend")
        if not 16 <= dimensions <= 8192:
            raise ValueError("Dimensions must be between 16 and 8192")
        if backend == "ollama" and (not model or not model_id):
            raise ValueError("Ollama requires --model and --model-id (pin model revision/preprocessing)")
        self.backend, self.model, self.dimensions, self.url = backend, model, dimensions, url
        self.identity = (None if backend == "lexical" else
                         f"tapirus-tools/sha256-word-v1/{dimensions}" if backend == "hash" else model_id)
        self.local = None
        if backend == "onnx":
            if model_path is None or dimensions != 384:
                raise ValueError("ONNX E5 requires --model-path and --dimensions 384")
            from .onnx_e5 import E5
            self.local = E5(model_path, truncate=truncate_model_inputs)
            self.identity = self.local.identity

    def embed(self, texts, query=False):
        if self.backend == "lexical":
            return [None] * len(texts)
        if not texts:
            return []
        if self.local is not None:
            return self.local.embed(texts, query=query)
        if self.backend == "hash":
            result = []
            for text in texts:
                vector = [0.0] * self.dimensions
                words = re.findall(r"\w+", text.lower())
                for word in words + [" ".join(words[i:i + 2]) for i in range(len(words) - 1)]:
                    digest = hashlib.sha256(word.encode()).digest()
                    vector[int.from_bytes(digest[:8], "little") % self.dimensions] += 1 if digest[8] & 1 else -1
                norm = math.sqrt(sum(v * v for v in vector))
                if not norm:
                    raise ValueError("Cannot embed text without features")
                result.append([v / norm for v in vector])
        else:
            result = post_json(self.url, "/api/embed", {
                "model": self.model, "input": list(texts), "truncate": False,
            }).get("embeddings")
        if not isinstance(result, list) or len(result) != len(texts):
            raise ValueError("Embedding provider returned the wrong vector count")
        for vector in result:
            if (not isinstance(vector, list) or len(vector) != self.dimensions
                    or any(type(v) not in (int, float) or not math.isfinite(v) for v in vector)
                    or not any(vector)):
                raise ValueError("Embedding provider returned invalid dimensions/components")
        return result


def add_arguments(parser):
    parser.add_argument("--backend", choices=["lexical", "hash", "ollama", "onnx"], default="lexical")
    parser.add_argument("--model-path", help="Verified local E5 snapshot directory; optional ONNX dependencies required")
    parser.add_argument("--truncate-model-inputs", action="store_true",
                        help="Explicitly truncate local E5 inputs to 512 tokens; changes model identity")
    parser.add_argument("--model")
    parser.add_argument("--model-id", help="Pinned embedding model revision/preprocessing identity")
    parser.add_argument("--dimensions", type=int, default=384)
    parser.add_argument("--url", default="http://127.0.0.1:11434")


def from_arguments(args):
    return Embedder(args.backend, args.model, args.dimensions, args.url, args.model_id,
                    args.model_path, args.truncate_model_inputs)
