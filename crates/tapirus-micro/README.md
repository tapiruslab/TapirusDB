# Tapirus Micro

An initial bare-metal component: `no_std`, no allocator, no dependencies, and
`forbid(unsafe_code)`. It stores bounded byte records in numbered flash slots and
provides an `i8` dot product for application-managed quantized vectors.

This component has a separate `TPMIC001` format. It does not read native `.tapir`
files and does not implement SQL, HNSW, graphs or a language model.

## Storage contract

Implement `Flash` for the board's driver. Give each slot two dedicated erase
blocks. The driver must enforce aligned writes and durable `sync`; failed
erase/program operations may have partially modified their target block.

```rust
# fn example<F: tapirus_micro::Flash>(flash: F) -> Result<(), tapirus_micro::Error<F::Error>> {
let mut store = tapirus_micro::Store::<_, 4096>::new(flash, 8)?;
store.put(0, b"robot calibration v1")?;
let mut value = [0u8; 128];
let length = store.get(0, &mut value)?;
# let _ = length;
# Ok(())
# }
```

Updates write the inactive block and commit after syncing its checksum/payload.
Failed operations have uncertain outcomes; reopen and read before retrying.
CRC detects accidental corruption, not malicious changes. The library fails
closed on a committed block with invalid CRC/header, including an interrupted
erase of an older committed bank. Recovery of such a bank requires a board or
application recovery procedure; it will not silently return potentially stale
data. There is no cross-slot atomicity, encryption, wear levelling or ECC.

Operations use a block-sized stack scratch buffer. Each slot consumes two erase
blocks and one block erase per replacement. Verify stack limits, endurance,
power-failure behavior and the driver's sync semantics on the actual board.

Host fault simulation and target compilation are engineering evidence; physical
hardware qualification remains required.
