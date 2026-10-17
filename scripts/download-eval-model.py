"""Download a pinned public model snapshot for reproducible local evaluation."""
import argparse
import hashlib
import json
from pathlib import Path
from urllib.request import urlopen


def fetch(url):
    with urlopen(url, timeout=60) as response:
        return response.read()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=Path("scratch/semantic-eval/model"))
    parser.add_argument("--revision", default="main")
    args = parser.parse_args()
    repo = "intfloat/multilingual-e5-small"
    metadata = json.loads(fetch(f"https://huggingface.co/api/models/{repo}/revision/{args.revision}?blobs=true"))
    revision = metadata["sha"]
    files = {entry["rfilename"]: entry for entry in metadata["siblings"]}
    paths = ["onnx/model_qint8_avx512_vnni.onnx", "onnx/tokenizer.json", "onnx/config.json", "README.md"]
    args.destination.mkdir(parents=True, exist_ok=True)
    manifest = {"repository": repo, "revision": revision, "files": [], "source": f"https://huggingface.co/{repo}/tree/{revision}"}
    for name in paths:
        entry = files[name]
        expected = entry.get("lfs", {}).get("sha256")
        destination = args.destination / Path(name).name
        url = f"https://huggingface.co/{repo}/resolve/{revision}/{name}?download=true"
        if not destination.exists() or (expected and hashlib.sha256(destination.read_bytes()).hexdigest() != expected):
            temporary = destination.with_suffix(destination.suffix + ".partial")
            print(f"Downloading {name}", flush=True)
            with urlopen(url, timeout=60) as response, temporary.open("wb") as output:
                while chunk := response.read(1024 * 1024):
                    output.write(chunk)
            temporary.replace(destination)
        data = destination.read_bytes()
        actual = hashlib.sha256(data).hexdigest()
        if expected and actual != expected:
            raise ValueError(f"LFS checksum mismatch: {name}")
        if not expected:
            blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
            if blob != entry["blobId"]:
                raise ValueError(f"Git blob mismatch: {name}")
        manifest["files"].append({"path": name, "local": destination.name, "bytes": len(data),
                                  "sha256": actual, "hub_lfs_sha256": expected})
    (args.destination / "model-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
