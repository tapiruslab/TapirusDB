"""Exercise a real local model through the installed agent and native database."""
import argparse
import json
from pathlib import Path
import tempfile
import tapirus
from tapirus_tools.agent import ingest, answer
from tapirus_tools.embeddings import Embedder


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    args = parser.parse_args()
    embedder = Embedder("onnx", dimensions=384, model_path=args.model)
    with tempfile.TemporaryDirectory(prefix="tapirus-onnx-agent-") as directory:
        root = Path(directory)
        sources = {
            "filter.txt": "Penapis habuk perlu dibersihkan setiap 40 jam operasi. Matikan kuasa sebelum membuka penutup penapis.",
            "battery.txt": "Charge the rover battery at the blue dock until its indicator turns green.",
            "radio.txt": "Hold the amber button for six seconds to reconnect the wireless controller.",
        }
        for name, content in sources.items():
            (root / name).write_text(content, encoding="utf-8")
        database = root / "agent.tapir"
        with tapirus.connect(str(database)) as connection:
            inserted = ingest(connection, [root / name for name in sources], embedder)
            assert inserted["chunks"] == 3
        with tapirus.connect(str(database)) as connection:
            result = answer(connection, "How often must the dust filter be cleaned?", embedder,
                            limit=1, retrieval_mode="vector")
            assert len(result["sources"]) == 1 and "40 jam" in result["sources"][0]["content"], result
            original_id = result["sources"][0]["id"]
            (root / "filter.txt").write_text(sources["filter.txt"].replace("40 jam", "20 jam"), encoding="utf-8")
            ingest(connection, [root / "filter.txt"], embedder)
            updated = answer(connection, "How often must the dust filter be cleaned?", embedder,
                             limit=1, retrieval_mode="vector")
            assert updated["sources"][0]["id"] == original_id
            assert "20 jam" in updated["sources"][0]["content"]
            assert connection.memory_stats()["documents"] == 3
        print(json.dumps({"engine": tapirus.__version__, "model": embedder.identity,
                          "checks": "local neural document/query prefixes, multilingual retrieval, persisted reopen, source citations and stable ID after source update",
                          "retrieved_source": updated["sources"][0]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
