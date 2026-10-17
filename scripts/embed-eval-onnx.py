"""Local E5 ONNX inference, official query/passage prefixes and mean pooling.

Optional evaluation dependencies: numpy, onnxruntime, tokenizers. No model code
is downloaded/executed. Model files must match the recorded download manifest.
"""
import argparse
import hashlib
import importlib.metadata
import json
from pathlib import Path
import time


import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "sdks/python/python"))
from tapirus_tools.onnx_e5 import E5


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dataset", type=Path)
    parser.add_argument("--model", type=Path, default=Path("scratch/semantic-eval/model"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=2)
    args = parser.parse_args()
    started = time.perf_counter()
    model = E5(args.model, args.threads)
    dataset_raw = args.dataset.read_bytes()
    dataset = json.loads(dataset_raw)
    output = {"model_id": model.identity, "dimensions": 384,
              "dataset_sha256": hashlib.sha256(dataset_raw).hexdigest(),
              "runtime": {name: importlib.metadata.version(name) for name in ("numpy", "onnxruntime", "tokenizers")},
              "model_manifest": model.manifest, "documents": {}, "queries": {}}
    loaded = time.perf_counter()
    for key, query in (("documents", False), ("queries", True)):
        items = dataset[key]
        for offset in range(0, len(items), 8):
            batch = items[offset:offset + 8]
            vectors = model.embed([item["query" if query else "content"] for item in batch], query)
            output[key].update({item["id"]: vector for item, vector in zip(batch, vectors)})
    output["model_load_seconds"] = loaded - started
    output["inference_seconds"] = time.perf_counter() - loaded
    output["threads"] = args.threads
    args.output.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: output[key] for key in ("model_id", "runtime", "model_load_seconds", "inference_seconds")}))


if __name__ == "__main__":
    main()
