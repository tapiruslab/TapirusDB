# Chatbot prototype

This directory contains a prototype application that combines a web interface, local inference integrations, and TapirusDB calls. It is not a validated truth classifier, a production chatbot, or a benchmark. Inference, retrieval, and persistence depend on the selected backend and code path.

The server's public and authenticated routes have different purposes; configure authentication and bind addresses for the environment. Do not treat displayed scores, timings, or retrieved text as calibrated confidence or factual verification.

Use the current [project README](../../README.md) and [security notes](../../SECURITY.md) for database and server limitations. Review the example source before running it.
