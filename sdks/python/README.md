# TapirusDB native Python SDK

Python 3.10+ binding to the Rust engine through PyO3. This package opens actual `.tapir` files; it does not require a daemon. `python/` is a separate legacy ctypes integration and does not define this package's API.

```python
import tapirus

with tapirus.connect("agent.tapir") as db:
    db.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
    db.execute("INSERT INTO notes VALUES (?, ?)", [1, "Don't interpolate SQL"])
    print(db.query("SELECT * FROM notes WHERE id = ?", [1]))
    source = db.store_memory({
        "content": "The delivery arrives on Tuesday",
        "embedding": [1.0, 0.0],  # supply your model's actual embedding
        "namespace": "support",
        "session_id": "case-7",
        "tags": ["source:delivery-policy"],
    })
    print(db.search_memories({"embedding": [1.0, 0.0], "namespace": "support"}))
```

`execute`/`query` accept a list or tuple of bound parameters: `None`, bool, signed 64-bit integer, finite float, string, bytes or a finite vector list. Results are lists of dictionaries; integers retain precision and blobs return `bytes`. Literal question marks inside SQL strings are preserved.

`store_memory`, `search_memories` and `forget_memory` use the [core AI memory contract](../../docs/AI_MEMORY.md). Results include source content/IDs and separate lexical, semantic and recency scores. Applications supply embeddings from a consistent model. These calls do not invoke a model or a network service. Namespace/session filters are application filters, not authorization boundaries. TAP classification/verification scores are heuristics, not calibrated probabilities or safety guarantees.

`execute("BEGIN")`, `COMMIT`, `ROLLBACK` and savepoints apply to SQL and AI memory together. The context manager closes the database on exit; it does not automatically commit an explicit transaction. `close()` is idempotent. It invalidates retained collection handles and releases the native connection. Connection and collection objects must stay on their creating Python thread; this module requires the GIL.

Pass a second argument to `connect(path, passphrase)` to open an encrypted native database. Browser snapshots currently use unencrypted files.

## Build and validate the wheel

The wheel also installs `tapirus-migrate`, `tapirus-agent` and `tapirus-eval`.
See [AI and embedded workflows](../../docs/AI_EMBEDDED_WORKFLOWS.md) for source
upsert/batch/TTL/model contracts, sessions, SQLite migration, a runnable agent,
and the optional verified local multilingual E5 ONNX backend. The ONNX runtime,
tokenizer and NumPy dependencies are optional; install the `onnx` extra or its
declared dependencies when using `--backend onnx`.

```sh
python -m pip install 'maturin>=1,<2'
maturin build --manifest-path sdks/python/Cargo.toml --locked --out target/python-wheels
python scripts/test-python-sdk.py --wheel target/python-wheels
maturin sdist --manifest-path sdks/python/Cargo.toml --out target/python-sdist
python scripts/normalize-python-sdist.py --sdist target/python-sdist --check
```

The consumer check installs the wheel offline into a fresh directory, writes in a subprocess, then reopens in a second process. It checks parameters, large integers, blobs, vectors, AI filters/rollback, collections, close and encrypted reopen. Local evidence covers Windows/Python 3.12; CI defines additional host/interpreter checks. Registry publication is a separate release step.

The source-package step prunes lock entries for workspace crates excluded by Maturin. It rejects registry version/checksum changes, retains other archive bytes and compiles the extracted package with `--locked --offline`. Python 3.12+ is required for this packaging check; the SDK itself supports Python 3.10+.

The wrapper retains its existing MIT declaration. The embedded engine uses the repository's BUSL-1.1 terms, included as `CORE_LICENSE`; the combined wheel is labeled `MIT AND BUSL-1.1`. See `NOTICE` for the component boundary.
