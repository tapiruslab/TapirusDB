"""Prepare the official BEIR SciFact test split for TapirusDB retrieval evaluation.

The dataset stays under scratch/ and is not distributed with the project.
"""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
from urllib.request import urlopen
import zipfile


URL = "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/scifact.zip"
OFFICIAL_MD5 = "5f7d1de60b170fc8027bb7898e2efca1"


def lines(archive, name):
    with archive.open(name) as stream:
        for line in stream:
            if line.strip():
                yield json.loads(line)


def hf_file(repository, name, directory):
    api = f"https://huggingface.co/api/datasets/{repository}/revision/main?blobs=true"
    with urlopen(api, timeout=30) as response:
        metadata = json.load(response)
    revision = metadata["sha"]
    entry = next(item for item in metadata["siblings"] if item["rfilename"] == name)
    path = directory / Path(name).name
    expected = entry.get("lfs", {}).get("sha256")
    if not path.is_file() or (expected and hashlib.sha256(path.read_bytes()).hexdigest() != expected):
        url = f"https://huggingface.co/datasets/{repository}/resolve/{revision}/{name}?download=true"
        temporary = path.with_suffix(path.suffix + ".partial")
        with urlopen(url, timeout=60) as response, temporary.open("wb") as output:
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
        temporary.replace(path)
    data = path.read_bytes()
    sha = hashlib.sha256(data).hexdigest()
    if expected and sha != expected:
        raise ValueError(f"Unexpected Hugging Face SHA-256 for {name}")
    if not expected:
        blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
        if blob != entry["blobId"]:
            raise ValueError(f"Unexpected Git blob for {name}")
    return path, {"repository": repository, "revision": revision, "path": name, "sha256": sha}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path("scratch/semantic-eval/beir"))
    parser.add_argument("--source", choices=["official", "hf"], default="official")
    args = parser.parse_args()
    args.directory.mkdir(parents=True, exist_ok=True)
    if args.source == "official":
        archive_path = args.directory / "scifact.zip"
        if not archive_path.is_file():
            temporary = archive_path.with_suffix(".partial")
            with urlopen(URL, timeout=60) as response, temporary.open("wb") as output:
                while chunk := response.read(1024 * 1024):
                    output.write(chunk)
            temporary.replace(archive_path)
        archive_bytes = archive_path.read_bytes()
        md5 = hashlib.md5(archive_bytes).hexdigest()  # Published dataset identity only.
        if md5 != OFFICIAL_MD5:
            raise ValueError(f"Unexpected BEIR SciFact archive MD5: {md5}")
        with zipfile.ZipFile(io.BytesIO(archive_bytes)) as archive:
            corpus = list(lines(archive, "scifact/corpus.jsonl"))
            queries = {item["_id"]: item["text"] for item in lines(archive, "scifact/queries.jsonl")}
            qrels_stream = archive.read("scifact/qrels/test.tsv").decode("utf-8")
        provenance = {"source_url": URL, "archive_md5": md5,
                      "archive_sha256": hashlib.sha256(archive_bytes).hexdigest()}
    else:
        import pyarrow.parquet as parquet  # Optional evaluation dependency.
        corpus_file, corpus_info = hf_file("BeIR/scifact", "corpus/corpus-00000-of-00001.parquet", args.directory)
        query_file, query_info = hf_file("BeIR/scifact", "queries/queries-00000-of-00001.parquet", args.directory)
        qrels_file, qrels_info = hf_file("BeIR/scifact-qrels", "test.tsv", args.directory)
        corpus = parquet.read_table(corpus_file).to_pylist()
        queries = {str(item["_id"]): item["text"] for item in parquet.read_table(query_file).to_pylist()}
        qrels_stream = qrels_file.read_text(encoding="utf-8")
        provenance = {"source": "Hugging Face BEIR mirror", "files": [corpus_info, query_info, qrels_info]}
    reader = csv.DictReader(io.StringIO(qrels_stream), delimiter="\t")
    qrels = {}
    for row in reader:
        if int(row["score"]) > 0:
            qrels.setdefault(row["query-id"], []).append(row["corpus-id"])
    documents = [{"id": str(item["_id"]), "content": " ".join(
        part for part in (item.get("title", ""), item["text"]) if part)} for item in corpus]
    source_ids = {item["id"] for item in documents}
    if len(source_ids) != len(documents):
        raise ValueError("Duplicate corpus IDs")
    test_queries = []
    for qid, relevant in sorted(qrels.items()):
        if qid not in queries or not set(relevant) <= source_ids:
            raise ValueError("Missing query or relevant document")
        test_queries.append({"id": qid, "language": "en", "query": queries[qid],
                             "relevant": sorted(set(relevant))})
    if len(test_queries) != 300:
        raise ValueError(f"Expected 300 labeled test queries, found {len(test_queries)}")
    dataset = {"name": "beir-scifact-test", "scope": "BEIR SciFact test qrels; full corpus. English scientific fact-checking claims. No training or tuning on this split.",
               "provenance": provenance,
               "documents": documents, "queries": test_queries}
    output = args.directory / "scifact-test.json"
    output.write_text(json.dumps(dataset, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    print(json.dumps({"dataset": str(output), "documents": len(documents),
                      "queries": len(test_queries), "provenance": provenance}))


if __name__ == "__main__":
    main()
