//! Mock server and transcript replay helpers.
//!
//! This module is intended for:
//! - unit/integration tests
//! - cross-QEMU-version compatibility tests
//! - CI regression tests using recorded QMP "conversations"
//!
//! It is gated behind `cfg(test)` or the `mock` Cargo feature.

use std::{collections::HashMap, path::Path, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};

#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

use crate::{
    client::Endpoint,
    error::{Error, Result},
    types::{Greeting, QmpInfo, QmpVersion, QmpVersionNumber},
};

/// How a command should be answered by the mock.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MockReply {
    /// Successful `return` payload.
    Return(Value),

    /// Error response.
    Error {
        /// QMP error class.
        class: String,
        /// QMP error description.
        desc: String,
    },
}

impl MockReply {
    fn into_response(self, id: &Value) -> Value {
        match self {
            MockReply::Return(v) => serde_json::json!({"return": v, "id": id}),
            MockReply::Error { class, desc } => serde_json::json!({
                "error": {"class": class, "desc": desc},
                "id": id
            }),
        }
    }
}

/// A simple mock script.
#[derive(Debug, Clone)]
pub struct MockScript {
    /// Greeting to send.
    pub greeting: Greeting,

    /// Map from `execute` command name to reply.
    pub replies: HashMap<String, MockReply>,

    /// Events to send right after handshake.
    pub post_handshake_events: Vec<Value>,
}

impl MockScript {
    /// Create a default greeting matching a recent QEMU.
    #[must_use]
    pub fn default_greeting() -> Greeting {
        Greeting {
            qmp: QmpInfo {
                version: QmpVersion {
                    qemu: QmpVersionNumber {
                        major: 8,
                        minor: 2,
                        micro: 0,
                    },
                    package: "mock".to_string(),
                },
                capabilities: vec!["oob".to_string()],
            },
        }
    }

    /// Create a script with a default greeting.
    #[must_use]
    pub fn new() -> Self {
        Self {
            greeting: Self::default_greeting(),
            replies: HashMap::new(),
            post_handshake_events: Vec::new(),
        }
    }

    /// Add a successful reply.
    #[must_use]
    pub fn reply_return(mut self, command: impl Into<String>, value: Value) -> Self {
        self.replies
            .insert(command.into(), MockReply::Return(value));
        self
    }

    /// Add an error reply.
    #[must_use]
    pub fn reply_error(
        mut self,
        command: impl Into<String>,
        class: impl Into<String>,
        desc: impl Into<String>,
    ) -> Self {
        self.replies.insert(
            command.into(),
            MockReply::Error {
                class: class.into(),
                desc: desc.into(),
            },
        );
        self
    }

    /// Add an event to be sent after handshake.
    #[must_use]
    pub fn post_event(mut self, event: Value) -> Self {
        self.post_handshake_events.push(event);
        self
    }
}

impl Default for MockScript {
    fn default() -> Self {
        Self::new()
    }
}

/// A running mock server.
///
/// Dropping the server will shut it down.
#[derive(Debug, Clone)]
pub struct MockServer {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    endpoint: Endpoint,
    shutdown_tx: mpsc::Sender<()>,
    done_rx: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

impl MockServer {
    /// Start a TCP mock server on 127.0.0.1:0 (ephemeral port).
    pub async fn start_tcp(script: MockScript) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::from)?;
        let addr = listener.local_addr().map_err(Error::from)?;
        let endpoint = Endpoint::tcp(addr.ip().to_string(), addr.port());

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let (done_tx, done_rx) = oneshot::channel();

        tokio::spawn(async move {
            tokio::select! {
                _ = async {
                    match listener.accept().await {
                        Ok((stream, _peer)) => {
                            let _ = handle_connection_tcp(stream, script).await;
                        }
                        Err(_e) => {}
                    }
                } => {}
                _ = shutdown_rx.recv() => {}
            }

            let _ = done_tx.send(());
        });

        Ok(Self {
            inner: Arc::new(Inner {
                endpoint,
                shutdown_tx,
                done_rx: tokio::sync::Mutex::new(Some(done_rx)),
            }),
        })
    }

    /// Start a Unix mock server at the given path.
    #[cfg(unix)]
    pub async fn start_unix(path: impl AsRef<Path>, script: MockScript) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Best effort cleanup.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path).map_err(Error::from)?;
        let endpoint = Endpoint::unix(path);

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let (done_tx, done_rx) = oneshot::channel();

        tokio::spawn(async move {
            tokio::select! {
                _ = async {
                    match listener.accept().await {
                        Ok((stream, _addr)) => {
                            let _ = handle_connection_unix(stream, script).await;
                        }
                        Err(_e) => {}
                    }
                } => {}
                _ = shutdown_rx.recv() => {}
            }

            let _ = done_tx.send(());
        });

        Ok(Self {
            inner: Arc::new(Inner {
                endpoint,
                shutdown_tx,
                done_rx: tokio::sync::Mutex::new(Some(done_rx)),
            }),
        })
    }

    /// Endpoint clients should connect to.
    #[must_use]
    pub fn endpoint(&self) -> Endpoint {
        self.inner.endpoint.clone()
    }

    /// Shut down the server and wait for completion.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown_tx.send(()).await;
        let mut rx = self.inner.done_rx.lock().await;
        if let Some(done) = rx.take() {
            let _ = done.await;
        }
    }
}

async fn handle_connection_tcp(stream: TcpStream, script: MockScript) -> Result<()> {
    handle_connection(stream, script).await
}

#[cfg(unix)]
async fn handle_connection_unix(stream: UnixStream, script: MockScript) -> Result<()> {
    handle_connection(stream, script).await
}
async fn handle_connection<S>(stream: S, mut script: MockScript) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (r, mut w) = tokio::io::split(stream);
    let mut r = BufReader::new(r);

    // Send greeting.
    send_json(
        &mut w,
        &serde_json::to_value(&script.greeting).map_err(Error::from)?,
    )
    .await?;

    // Expect qmp_capabilities.
    let req = recv_json(&mut r).await?;
    let id = req.get("id").cloned().unwrap_or_else(|| Value::from(0));

    let execute = req.get("execute").and_then(|v| v.as_str()).unwrap_or("");
    if execute != "qmp_capabilities" {
        // Respond with a protocol error.
        let err = serde_json::json!({
            "error": {"class": "ProtocolError", "desc": "expected qmp_capabilities"},
            "id": id
        });
        send_json(&mut w, &err).await?;
        return Ok(());
    }

    send_json(&mut w, &serde_json::json!({"return": {}, "id": id})).await?;

    // Serve commands.
    let mut pending_events = std::mem::take(&mut script.post_handshake_events);

    loop {
        let req = match recv_json(&mut r).await {
            Ok(v) => v,
            Err(Error::Disconnected) => return Ok(()),
            Err(e) => return Err(e),
        };

        if !pending_events.is_empty() {
            for ev in pending_events.drain(..) {
                send_json(&mut w, &ev).await?;
            }
        }

        let id = req.get("id").cloned().unwrap_or(Value::from(0));
        let execute = req
            .get("execute")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let reply = script
            .replies
            .get(&execute)
            .cloned()
            .unwrap_or_else(|| MockReply::Error {
                class: "CommandNotFound".to_string(),
                desc: format!("no mock reply for '{execute}'"),
            });

        send_json(&mut w, &reply.into_response(&id)).await?;
    }
}

async fn send_json<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> Result<()> {
    let line = msg.to_string();
    w.write_all(line.as_bytes()).await.map_err(Error::from)?;
    w.write_all(b"\r\n").await.map_err(Error::from)?;
    w.flush().await.map_err(Error::from)?;
    Ok(())
}

async fn recv_json<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Result<Value> {
    let mut line = String::new();
    let n = r.read_line(&mut line).await.map_err(Error::from)?;
    if n == 0 {
        return Err(Error::Disconnected);
    }

    let line = line.trim_end_matches(['\r', '\n']);
    serde_json::from_str(line).map_err(Error::from)
}

/// A transcript step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "dir", rename_all = "lowercase")]
pub enum TranscriptStep {
    /// A message sent by the server.
    Server {
        /// Message payload.
        msg: Value,
    },
    /// A message expected from the client.
    Client {
        /// Message payload.
        msg: Value,
    },
}

/// A JSONL transcript, suitable for replay.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    /// Ordered transcript steps.
    pub steps: Vec<TranscriptStep>,
}

impl Transcript {
    /// Parse from JSON Lines content.
    pub fn from_jsonl_str(s: &str) -> Result<Self> {
        let mut steps = Vec::new();
        for (idx, line) in s.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let step: TranscriptStep = serde_json::from_str(line).map_err(|e| Error::Protocol {
                message: format!("invalid jsonl at line {}: {}", idx + 1, e),
            })?;
            steps.push(step);
        }

        Ok(Self { steps })
    }

    /// Load a transcript from a JSONL file.
    pub fn from_jsonl_file(path: impl AsRef<Path>) -> Result<Self> {
        let data = std::fs::read_to_string(path).map_err(Error::from)?;
        Self::from_jsonl_str(&data)
    }
}

/// Replay a transcript by acting as a QMP server.
///
/// The server sends/receives messages exactly as specified.
#[derive(Debug)]
pub struct ReplayServer {
    server: MockServer,
}

impl ReplayServer {
    /// Start a replay server on TCP.
    pub async fn start_tcp(transcript: Transcript) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::from)?;
        let addr = listener.local_addr().map_err(Error::from)?;
        let endpoint = Endpoint::tcp(addr.ip().to_string(), addr.port());

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let (done_tx, done_rx) = oneshot::channel();

        tokio::spawn(async move {
            tokio::select! {
                _ = async {
                    match listener.accept().await {
                        Ok((stream, _peer)) => {
                            let _ = replay_connection(stream, transcript).await;
                        }
                        Err(_e) => {}
                    }
                } => {}
                _ = shutdown_rx.recv() => {}
            }

            let _ = done_tx.send(());
        });

        Ok(Self {
            server: MockServer {
                inner: Arc::new(Inner {
                    endpoint,
                    shutdown_tx,
                    done_rx: tokio::sync::Mutex::new(Some(done_rx)),
                }),
            },
        })
    }

    /// Endpoint clients should connect to.
    #[must_use]
    pub fn endpoint(&self) -> Endpoint {
        self.server.endpoint()
    }

    /// Shut down the server.
    pub async fn shutdown(&self) {
        self.server.shutdown().await
    }
}

async fn replay_connection<S>(stream: S, transcript: Transcript) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (r, mut w) = tokio::io::split(stream);
    let mut r = BufReader::new(r);

    for step in transcript.steps {
        match step {
            TranscriptStep::Server { msg } => {
                send_json(&mut w, &msg).await?;
            }
            TranscriptStep::Client { msg: expected } => {
                let got = recv_json(&mut r).await?;

                if got != expected {
                    // Best effort diagnostic.
                    let err = Error::Protocol {
                        message: format!("transcript mismatch: expected {}, got {}", expected, got),
                    };
                    return Err(err);
                }
            }
        }
    }

    Ok(())
}
