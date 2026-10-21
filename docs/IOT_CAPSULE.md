# Offline IoT capsule bridge

Tapirus Micro can export one committed flash slot as a small versioned frame.
The full TapirusDB Quad engine can import that frame into local AI memory. No
external database or network service is required. This is a **source-build
capability**, not a newly published release artifact or an MCU hardware claim.

## Device side: bounded and offline

`tapirus-micro` remains `no_std`, allocation-free and independent of the full
`.tapir` format. A board application implements the `Flash` trait, writes a
text value to a slot, then exports its committed state:

```rust
use tapirus_micro::{ExportError, Flash, Store};

fn send_latest<F: Flash>(flash: F) -> Result<(), ExportError<F::Error>> {
    let mut store = Store::<_, 4096>::new(flash, 8).map_err(ExportError::Store)?;
    store.put(0, b"pump bearing temperature 68 C").map_err(ExportError::Store)?;
    let mut output = [0u8; 4096];
    let device_id = [7u8; 16]; // Example only: provision a unique ID per device.
    if let Some(length) = store.export_capsule(0, device_id, &mut output)? {
        let frame = &output[..length];
        // Send exactly `frame` over an application-chosen channel.
        let _ = frame;
    }
    Ok(())
}
```

The transport frame is not the native `.tapir` file. Its fixed 48-byte header
contains the `TPIOCAP1` magic, version 1, deletion flag, 16-byte device ID,
32-bit slot, 64-bit **per-slot** sequence, payload length and CRC32. Payloads
are bounded to 64 KiB. The CRC covers the header fields and payload and detects
accidental corruption; it does **not** prove origin or prevent intentional
tampering. An all-zero device ID is rejected. A deletion exports a tombstone
with no payload. If device flash is factory-reset and its sequence restarts,
provision a new device ID (or migrate the gateway head deliberately); reusing
the old identity will make new low-sequence frames stale.

## Quad side: local AI memory

Build from the current checkout:

```sh
cargo build --release -p tapirus --bin tapirus-iot
tapirus-iot inspect reading.cap
tapirus-iot import gateway.tapir reading.cap
tapirus-iot search gateway.tapir DEVICE_ID_HEX "bearing temperature"
```

Replace `DEVICE_ID_HEX` with the 32-character lowercase device ID shown by
`inspect` or `import`. On Windows PowerShell, replace `tapirus-iot` with
`./target/release/tapirus-iot.exe`. The CLI reads a capsule file; it does not
create a transport or embed a neural model. `search` runs built-in lexical
memory retrieval filtered to one device; it does not produce an LLM answer.
Applications can instead call:

```rust
use tapirus::{SessionDatabase, iot::import_micro_capsule};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read("reading.cap")?;
    let db = SessionDatabase::open("gateway.tapir")?;
    let session = db.session()?;
    let receipt = import_micro_capsule(&session, &bytes)?;
    println!("{receipt:?}");
    session.close()?;
    db.close()?;
    Ok(())
}
```

Import uses an owned session transaction. The head record and AI memory source
change together. An exact replay is a no-op; an older sequence or conflicting
frame at the same sequence is rejected. A newer nondeleted slot replaces the
previous memory source. A tombstone removes that source. Device IDs are kept in
`iot/<device-id-hex>` namespaces with an exact `slot/<number>` external ID and
source-version sequence. Namespaces are retrieval filters, **not** access
control. The payload must be nonempty UTF-8 text for AI memory; binary sensor
records need an application encoding before import. Imported text is searchable
lexically. A gateway application can call
`import_micro_capsule_with_provider(&session, &bytes, &provider)` to generate
an embedding **before** the transaction using a local, pinned-model
`EmbeddingProvider`. The existing memory search API can then use query vectors
from that same model space. `DeterministicHashEmbedder` is a feature-hash
baseline, not a neural model. No neural model, LLM answer or automatic fact
verification is bundled or claimed by this bridge. Treat device-supplied text
as untrusted input when using it with an LLM.

## Next evidence gates

1. Run on a named board and flash driver; measure firmware size, RAM/stack,
   erase cycles, interrupted writes and actual `sync` behavior. Host simulation
   and cross-compilation are not physical power-cut qualification.
2. Add authenticated, optionally encrypted transport and provisioning. A CRC
   and application-provided device ID do not establish provenance or secrecy.
3. Add an append-only event path if historical telemetry is needed. The current
   bridge preserves **latest state per slot**, not every overwritten reading.
4. Benchmark end-to-end sensor write → transfer → import → local retrieval and
   measure cold reopen/resource use before claiming an IoT or AI performance win.
