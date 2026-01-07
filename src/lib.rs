//! `qmp` - Async-first QEMU/QMP client library.
//!
//! This crate is designed for:
//! - QEMU QMP sockets (Unix / TCP)
//! - PVE / Proxmox scenarios where you want observability and automation
//!   without invasive changes
//!
//! It provides:
//! - socket connection management (Unix/TCP)
//! - handshake + `qmp_capabilities` negotiation
//! - typed JSON (serde) request/response handling
//! - subscribable event stream
//! - per-call timeout and cooperative cancellation
//! - an optional "safe ops" layer with allow-lists, idempotency, mutex, and retry/backoff
//!
//! ## Quick start (Unix socket)
//!
//! ```no_run
//! use qmp::{Client, Endpoint};
//! # async fn demo() -> qmp::Result<()> {
//! let client = Client::connect(Endpoint::unix("/var/run/qemu-server/100.qmp")).await?;
//!
//! // Low-level (generic) call:
//! let status: serde_json::Value = client.execute("query-status", Option::<()>::None).await?;
//! println!("status = {status}");
//!
//! // Subscribe to events:
//! let mut events = client.events();
//! if let Ok(ev) = events.recv().await {
//!     println!("event: {}", ev.name);
//! }
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cancel;
mod client;
mod event_stream;
mod transport;

pub mod error;
pub mod ops;
pub mod types;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use cancel::CancelToken;
pub use client::{CallOptions, Client, ClientBuilder, ConnectOptions, Endpoint};
pub use error::{Error, Result};
pub use event_stream::EventStream;
