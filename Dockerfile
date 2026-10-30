# ==============================================================================
# TapirusDB Linux container image
# Multi-stage build; measure the final size instead of assuming a fixed limit.
# ==============================================================================

# Stage 1: Build binary and shared library
FROM rust:alpine AS builder

RUN apk add --no-cache musl-dev gcc git

WORKDIR /app
COPY . .

RUN cargo build --locked --release --bin tapirus

# Stage 2: Minimal runtime image
FROM alpine:3.24

WORKDIR /data

RUN addgroup -S tapirus && adduser -S -G tapirus tapirus \
    && chown tapirus:tapirus /data

COPY --from=builder /app/target/release/tapirus /usr/local/bin/tapirus
COPY --from=builder /app/include/tapirus.h /usr/local/include/tapirus.h

VOLUME ["/data"]

USER tapirus

ENTRYPOINT ["/usr/local/bin/tapirus"]
CMD ["--help"]
