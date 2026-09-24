# ==============================================================================
# TapirusDB Multi-Arch Container Image
# Multi-stage build producing an ultra-lightweight < 15MB container
# ==============================================================================

# Stage 1: Build binary and shared library
FROM rust:1.88-alpine AS builder

RUN apk add --no-cache musl-dev gcc git

WORKDIR /app
COPY . .

RUN cargo build --release --workspace

# Stage 2: Minimal runtime image
FROM alpine:3.20

RUN apk add --no-cache libgcc

WORKDIR /data

COPY --from=builder /app/target/release/tapirus /usr/local/bin/tapirus
COPY --from=builder /app/target/release/libtapirus.so /usr/local/lib/libtapirus.so
COPY --from=builder /app/include/tapirus.h /usr/local/include/tapirus.h

ENV LD_LIBRARY_PATH=/usr/local/lib:$LD_LIBRARY_PATH

VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/tapirus"]
CMD ["--help"]
