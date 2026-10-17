"""Optional local E5 ONNX provider. No network or model code execution.

Requires numpy, onnxruntime and tokenizers; download/verify a model snapshot
explicitly with scripts/download-eval-model.py before use.
"""
import hashlib
import json
from pathlib import Path
import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer


class E5:
    def __init__(self, model_path, threads=2, truncate=False):
        self.path = Path(model_path)
        self.manifest = json.loads((self.path / "model-manifest.json").read_text(encoding="utf-8"))
        for entry in self.manifest["files"]:
            if hashlib.sha256((self.path / entry["local"]).read_bytes()).hexdigest() != entry["sha256"]:
                raise ValueError("Model snapshot checksum mismatch")
        self.tokenizer = Tokenizer.from_file(str(self.path / "tokenizer.json"))
        self.tokenizer.enable_padding(pad_id=1, pad_token="<pad>")
        self.truncate = truncate
        self.truncated_inputs = 0
        if truncate:
            self.tokenizer.enable_truncation(max_length=512)
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
        options.inter_op_num_threads = 1
        self.session = ort.InferenceSession(str(self.path / "model_qint8_avx512_vnni.onnx"),
                                            sess_options=options, providers=["CPUExecutionProvider"])
        self.identity = (self.manifest["repository"] + "@" + self.manifest["revision"]
                         + "/qint8-avx512-vnni/e5-query-passage-mean-l2-v1"
                         + ("/truncate512" if truncate else ""))

    def embed(self, texts, query=False):
        if not texts:
            return []
        if len(texts) > 8:
            return [vector for offset in range(0, len(texts), 8)
                    for vector in self.embed(texts[offset:offset + 8], query=query)]
        prefix = "query: " if query else "passage: "
        encoded = self.tokenizer.encode_batch([prefix + text for text in texts])
        if any(len(item.ids) > 512 for item in encoded):
            raise ValueError("E5 input exceeds 512 tokens; chunk sources explicitly")
        self.truncated_inputs += sum(bool(item.overflowing) for item in encoded)
        feed = {"input_ids": np.asarray([item.ids for item in encoded], dtype=np.int64),
                "attention_mask": np.asarray([item.attention_mask for item in encoded], dtype=np.int64)}
        if any(item.name == "token_type_ids" for item in self.session.get_inputs()):
            feed["token_type_ids"] = np.asarray([item.type_ids for item in encoded], dtype=np.int64)
        hidden = self.session.run(None, feed)[0]
        if hidden.ndim != 3 or hidden.shape[2] != 384:
            raise ValueError(f"Unexpected E5 model output: {hidden.shape}")
        mask = feed["attention_mask"][..., None]
        pooled = (hidden * mask).sum(axis=1) / mask.sum(axis=1)
        pooled /= np.linalg.norm(pooled, axis=1, keepdims=True)
        if not np.isfinite(pooled).all():
            raise ValueError("Nonfinite E5 output")
        return pooled.astype(np.float32).tolist()
