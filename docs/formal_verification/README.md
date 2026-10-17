# WAL TLA+ specification: scope and status

This directory contains a TLA+ model for selected WAL and recovery transitions. The model is a design aid and is not evidence that the Rust implementation has been formally verified.

No TLC execution output, checked state count, invariant result, or refinement proof connecting this model to the implementation is included in this repository. Do not describe the engine as formally verified based on these files alone.

To run the model locally, install the TLA+ tools and execute:

```bash
java -cp tla2tools.jar tlc2.TLC TAPIRUS_WAL.tla -config TAPIRUS_WAL.cfg
```

A successful run should be recorded with the TLC version, exact command, model configuration, explored state count, and invariant results. It would verify only the bounded model that was checked; it would not prove the Rust implementation correct.