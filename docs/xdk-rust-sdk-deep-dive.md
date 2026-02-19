# XDK Deep Dive: Why Python Worked and Rust Failed

## What We Compared

- Python path (working): official `xdk` streaming runtime (`xdk/streaming.py`)
- Rust path (failing): custom streaming client in `src/x_api.rs`

## Key Runtime Difference

Python `xdk` stream runtime does **not** use a short global request timeout for streaming sockets.

Rust client previously used:

- `reqwest::Client::builder().timeout(Duration::from_secs(30))`

That global timeout applies to response-body reading too, which is incompatible with long-lived filtered streams. During quiet periods, this can force disconnects, causing reconnect churn and eventually `TooManyConnections` / `429` behavior.

## XDK Patterns Observed

From `xdk/streaming.py` and `xdk/stream/client.py`:

- Connection lifecycle callbacks (`on_connect`, `on_disconnect`, `on_reconnect`, `on_error`)
- Retry classification and backoff around stream connection attempts
- Streaming connection kept open as a long-lived channel, with retries handled externally

## Fix Applied in Rust

Updated `src/x_api.rs`:

1. Removed global request timeout from the shared `reqwest::Client`.
2. Added connection/keepalive tuning for stream stability:
   - `connect_timeout(15s)`
   - `tcp_keepalive(30s)`
3. Added **per-request** timeout (`30s`) only for non-stream APIs:
   - add rule
   - delete rule
   - terminate-all-connections

Result: stream connections are no longer forced closed by client timeout during normal idle periods.

## About Building a Full Rust SDK From `xdk`

`xdk` currently has production generators/templates for Python + TypeScript only (`xdk-gen/src/python`, `xdk-gen/src/typescript`).

A real Rust SDK generation path would require:

1. `xdk-gen/src/rust/generator.rs` using `language!` macro config
2. Rust template set under `xdk-gen/templates/rust/*.j2`
3. Rust-specific filters/type-mapping in generator pipeline
4. Emitted crate layout and tests from `xdk-build`

That is feasible, but is a larger project than a stream-runtime fix.

## Recommended Next Step

Short-term: keep the current Rust app on the fixed runtime (already applied).

Medium-term: if you want parity with official XDK generation flow, implement a dedicated Rust generator in `xdk-gen` and migrate `x-monitor` to consume the generated crate.
