### Builder stage (Nightly for Edition 2024 support)
FROM rustlang/rust:nightly-slim AS builder
WORKDIR /usr/src/app

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev build-essential \
 && rm -rf /var/lib/apt/lists/*

COPY . .
RUN cargo build --release && strip target/release/smpp-perf || true

### Runtime stage (matches GLIBC from builder)
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /usr/src/app/target/release/smpp-perf /usr/local/bin/ilinaia-smpp-perf
COPY config.example.toml /config/config.toml

ENTRYPOINT ["/usr/local/bin/ilinaia-smpp-perf"]
CMD ["--config", "/config/config.toml"]

