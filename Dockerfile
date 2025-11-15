### Builder
FROM rust:1.81 as builder
WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && cargo fetch

# Build release binary
COPY . .
RUN cargo build --release && strip target/release/smpp-perf || true

### Runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app

COPY --from=builder /app/target/release/smpp-perf /usr/local/bin/ilinaia-smpp-perf
COPY config.example.toml /config/config.toml

ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/ilinaia-smpp-perf"]
CMD ["--config", "/config/config.toml"]

