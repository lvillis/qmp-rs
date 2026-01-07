# qmp

**QEMU QMP** client library.

Goals:

- **Pure Rust**, async-first.
- **Unix/TCP** socket support.
- Correct **greeting + `qmp_capabilities`** negotiation.
- JSON request/response encoding/decoding with **serde**.
- **Event stream** (subscribe, multi-consumer).
- Command calls with **timeout** and cooperative **cancellation**.
- Optional, higher-level **safe ops** layer:
  - allow-list policies (white-list)
  - per-command mutex / idempotency
  - retry/backoff with jitter
  - structured errors
  - (optional) `tracing` observability

## Quick start

### Low-level client

```rust
use qmp::{Client, Endpoint};

# async fn demo() -> qmp::Result<()> {
let client = Client::connect(Endpoint::unix("/var/run/qemu-server/100.qmp")).await?;

let status: serde_json::Value = client.execute("query-status", Option::<()>::None).await?;
println!("status = {status}");

let mut events = client.events();
while let Ok(ev) = events.recv().await {
    println!("event: {} data={}", ev.name, ev.data);
}
# Ok(()) }
```

### Safe ops layer

```rust
use qmp::{Client, Endpoint};
use qmp::ops::{OpsClient, Profile};

# async fn demo() -> qmp::Result<()> {
let client = Client::connect(Endpoint::unix("/var/run/qemu-server/100.qmp")).await?;

// Read-only allow-list by default.
let ops = OpsClient::from_profile(client, Profile::ReadOnly);

let v = ops.query_version().await?;
println!("qemu {}.{}.{} ({})", v.qemu.major, v.qemu.minor, v.qemu.micro, v.package);
# Ok(()) }
```
