# Python integration source

This is the legacy `tapirus-ctypes` source distribution. New applications should
use the native [`tapirus` wheel](../sdks/python/README.md). Both integrations
import as `tapirus`, so install them in separate environments. The legacy
distribution does not bundle the required C shared library.

This repository contains Python integration code. Its API, native-loading behavior, packaging, and persistence semantics must be checked against the exact package artifact in use; no registry availability or support guarantee is asserted here.

The audit identified confidence values and packaging behavior that must not be treated as calibrated or persistent without verification. Inspect [`tapirus/`](tapirus/) and test write, close, process restart, and reopen using disposable data.

See the [project README](../README.md) for core limits and the [SDK source index](../sdks/README.md) for the other language bindings.
