# Ilinaia SMPP Performance

Ilinaia SMPP Performance is a high-throughput Rust load generator for benchmarking remote SMPP servers. It orchestrates multiple transceiver binds via [`rusmppc`](https://github.com/Rusmpp/Rusmpp/tree/main/rusmppc), drives deep in-flight `submit_sm` queues, enforces optional TPS ceilings, and renders a live dashboard that exposes per-bind TPS, latency, OK/Err ratios, and the most recent message IDs.

© 2025 ilinaia.com — Alexey Ilinskiy. All rights reserved.

---

## Feature Highlights

- **Configurable load plans**: tune bind counts, TPS ceilings, and outstanding submissions per bind.
- **Elastic throughput**: set `max_tps_per_bind = 0` to blast as fast as the SMSC allows or pin to deterministic TPS.
- **Rich observability**: every bind reports its current TPS, average latency, OK/Err totals, and last message ID returned by the SMSC.
- **Smooth control plane**: `Ctrl+C` propagates a cancellation token, drains in-flight work, and prints a final summary.
- **Ship anywhere**: prebuilt Docker workflow plus native `cargo run` keeps deployment friction low.

---

## Configuration

Create a TOML file (default `config.toml`). A starting point lives in `config.example.toml`:

```
[smpp]
host = "127.0.0.1"
port = 2775
system_id = "NfDfddEKVI0NCxO"
password = "rEZYMq5j"
system_type = ""

[message]
source_addr = "12345"
source_ton = 2
source_npi = 1
destination_addr = "491701234567"
destination_ton = 2
destination_npi = 1
body = "Hi, I’m Alexey. At Vinoc.mx, we provide high-quality wholesale A2P SMS traffic termination"
service_type = ""

[load]
binds = 2
max_tps_per_bind = 100
inflight_per_bind = 64
```

Key knobs:

- `binds`: number of SMPP transceiver sessions to open (minimum `0`).
- `max_tps_per_bind`: per-session throttle. Defaults to `100`; set to `0` to remove throttling and rely on in-flight saturation.
- `inflight_per_bind`: simultaneous `submit_sm` futures per bind. Larger values (512+) are recommended for 10–50k TPS testing.
- `source_*` / `destination_*`: TON/NPI values passed straight to SMPP PDUs.

---

## Architecture (PlantUML)

`docs/architecture.puml` documents the control flow. Render it with any PlantUML-compatible tool:

```
@startuml IlinaiaSMPP
skinparam monochrome true
title Ilinaia SMPP Performance

actor Operator
participant "CLI Runner" as CLI
participant "Bind Manager" as BindMgr
participant "Per-Bind Task" as BindTask
participant "Metrics + Tracker" as Metrics
participant "SMPP Server" as SMSC

Operator -> CLI : cargo run / docker run
CLI -> BindMgr : Load config + spawn binds
loop Each bind
    BindMgr -> BindTask : tokio::spawn
    BindTask -> SMSC : bind_transceiver
    BindTask -> Metrics : state=Bound
    loop submit_sm
        BindTask -> SMSC : submit_sm
        SMSC --> BindTask : submit_sm_resp(message_id)
        BindTask -> Metrics : latency/ok + last message id
    end
end
Operator -> CLI : Ctrl+C
CLI -> BindMgr : cancel token
BindTask -> SMSC : unbind + close
Metrics -> Operator : Live dashboard (TPS, latency, msg IDs)
@enduml
```

---

## Native Run

```bash
cargo run --release -- --config config.toml
```

The dashboard refreshes every 500 ms and shows:

- Global message totals with OK/Err percentages and rolling average latency.
- Aggregate instantaneous TPS.
- Per-bind rows with TPS, average latency, OK/Err counts, bind state, and last message ID.

---

## Docker

### Build

```bash
docker build -t ilinaia-smpp-perf .
```

### Run with mounted config

```bash
docker run --rm \
  -e RUST_LOG=info \
  -v $(pwd)/config.toml:/config/config.toml:ro \
  ilinaia-smpp-perf \
  --config /config/config.toml
```

Mount any writable directory (like `./logs`) if you plan to tee dashboard output to files or capture diagnostics.

---

## Credits

- Powered by [`rusmppc`](https://crates.io/crates/rusmppc) and [`rusmpp`](https://crates.io/crates/rusmpp).
- Inspired by the official `rusmppc_submit_sm` example in the Rusmpp repository.
- Special thanks to Jad K. Haddad, creator of [`Rusmpp/Rusmpp`](https://github.com/Rusmpp/Rusmpp), for the SMPP library stack powering Ilinaia SMPP Performance.

---

## License

Ilinaia SMPP Performance is distributed under the MIT License. You are free to use, copy, modify, merge, publish, distribute, sublicense, or sell copies of the software, provided that the copyright notice is preserved. See [`LICENSE`](./LICENSE) for the full text.

© 2025 ilinaia.com — Alexey Ilinskiy.

