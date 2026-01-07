//! Asynchronous QMP client.

use std::{
    collections::HashMap,
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, broadcast, oneshot},
};

use crate::{
    cancel::CancelToken,
    error::{Error, Result},
    event_stream::EventStream,
    types::{Event, EventMessage, Greeting, QmpResponse},
};

/// QMP endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// Connect via a Unix domain socket.
    Unix {
        /// Socket path.
        path: PathBuf,
    },

    /// Connect via a TCP socket.
    Tcp {
        /// Hostname or IP.
        host: String,
        /// Port.
        port: u16,
    },
}

impl Endpoint {
    /// Create a Unix socket endpoint.
    #[must_use]
    pub fn unix(path: impl Into<PathBuf>) -> Self {
        Self::Unix { path: path.into() }
    }

    /// Create a TCP endpoint.
    #[must_use]
    pub fn tcp(host: impl Into<String>, port: u16) -> Self {
        Self::Tcp {
            host: host.into(),
            port,
        }
    }
}

/// Options controlling how the QMP connection is established.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Capabilities to enable during the `qmp_capabilities` negotiation.
    ///
    /// Leaving this empty performs a plain `qmp_capabilities` without arguments.
    pub enable_capabilities: Vec<String>,

    /// Size of the internal event broadcast buffer.
    ///
    /// If consumers lag behind, events will be dropped and surfaced as
    /// [`Error::EventLagged`].
    pub event_buffer: usize,

    /// Default timeout for command calls.
    ///
    /// Individual calls can override this via [`CallOptions`].
    pub default_timeout: Option<Duration>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            enable_capabilities: Vec::new(),
            event_buffer: 1024,
            default_timeout: Some(Duration::from_secs(30)),
        }
    }
}

/// Options for a single command call.
#[derive(Debug, Clone, Default)]
pub struct CallOptions {
    /// Override the default timeout.
    pub timeout: Option<Duration>,

    /// A cancellation token.
    pub cancel: Option<CancelToken>,
}

/// QMP client builder.
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    endpoint: Endpoint,
    options: ConnectOptions,
}

impl ClientBuilder {
    /// Set capabilities to enable during handshake.
    #[must_use]
    pub fn enable_capabilities(mut self, caps: impl Into<Vec<String>>) -> Self {
        self.options.enable_capabilities = caps.into();
        self
    }

    /// Set internal event buffer size.
    #[must_use]
    pub fn event_buffer(mut self, size: usize) -> Self {
        self.options.event_buffer = size;
        self
    }

    /// Set default command timeout.
    #[must_use]
    pub fn default_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.options.default_timeout = timeout;
        self
    }

    /// Connect and perform handshake.
    pub async fn connect(self) -> Result<Client> {
        Client::connect_with_options(self.endpoint, self.options).await
    }
}

/// An async-first QMP client.
#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    writer: Mutex<Writer>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<QmpResponse>>>>,
    next_id: AtomicU64,
    events_tx: broadcast::Sender<Event>,
    default_timeout: Option<Duration>,
    greeting: Greeting,
}

struct Writer {
    /// The underlying write half.
    w: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
}

impl Writer {
    async fn send_line(&mut self, line: &str) -> Result<()> {
        self.w
            .write_all(line.as_bytes())
            .await
            .map_err(Error::from)?;
        self.w.write_all(b"\r\n").await.map_err(Error::from)?;
        self.w.flush().await.map_err(Error::from)?;
        Ok(())
    }
}

impl fmt::Debug for Writer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer").finish()
    }
}

impl Client {
    /// Create a builder for the given endpoint.
    #[must_use]
    pub fn builder(endpoint: Endpoint) -> ClientBuilder {
        ClientBuilder {
            endpoint,
            options: ConnectOptions::default(),
        }
    }

    /// Connect to an endpoint with default options.
    pub async fn connect(endpoint: Endpoint) -> Result<Client> {
        Self::connect_with_options(endpoint, ConnectOptions::default()).await
    }

    /// Connect to an endpoint and perform QMP handshake.
    pub async fn connect_with_options(
        endpoint: Endpoint,
        options: ConnectOptions,
    ) -> Result<Client> {
        let stream = crate::transport::connect(endpoint).await?;
        let (r, w) = tokio::io::split(stream);

        let mut reader = BufReader::new(r);

        // 1) Greeting
        let greeting = read_json_line::<Greeting>(&mut reader).await?;

        #[cfg(feature = "tracing")]
        tracing::debug!(
            qemu_major = greeting.qmp.version.qemu.major,
            qemu_minor = greeting.qmp.version.qemu.minor,
            qemu_micro = greeting.qmp.version.qemu.micro,
            caps = ?greeting.qmp.capabilities,
            "received QMP greeting"
        );

        // 2) qmp_capabilities
        let mut writer = Writer { w: Box::new(w) };

        let cap_req = build_capabilities_request(0, &options.enable_capabilities);
        writer.send_line(&cap_req).await?;

        let cap_resp = read_json_line::<QmpResponse>(&mut reader).await?;
        validate_capabilities_response(cap_resp)?;

        let (events_tx, _events_rx) = broadcast::channel(options.event_buffer.max(1));

        let client = Client {
            inner: Arc::new(Inner {
                writer: Mutex::new(writer),
                pending: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                events_tx,
                default_timeout: options.default_timeout,
                greeting: greeting.clone(),
            }),
        };

        // Start reader loop.
        client.spawn_reader(reader);

        Ok(client)
    }

    /// The greeting received during handshake.
    #[must_use]
    pub fn greeting(&self) -> Greeting {
        self.inner.greeting.clone()
    }

    /// Subscribe to the QMP event stream.
    #[must_use]
    pub fn events(&self) -> EventStream {
        EventStream::new(self.inner.events_tx.subscribe())
    }

    /// Execute a QMP command.
    ///
    /// `args` will be serialized into the `arguments` field.
    ///
    /// The return value is deserialized from the QMP `return` field.
    pub async fn execute<A, R>(&self, command: &str, args: Option<A>) -> Result<R>
    where
        A: Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.execute_with_options(command, args, CallOptions::default())
            .await
    }

    /// Execute a QMP command with per-call options.
    pub async fn execute_with_options<A, R>(
        &self,
        command: &str,
        args: Option<A>,
        options: CallOptions,
    ) -> Result<R>
    where
        A: Serialize,
        R: serde::de::DeserializeOwned,
    {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);

        let request_json = build_execute_request(id, command, args.as_ref());

        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.inner.pending.lock().await;
            pending.insert(id, tx);
        }

        // Send request.
        {
            let mut writer = self.inner.writer.lock().await;

            #[cfg(feature = "tracing")]
            tracing::trace!(id = id, command = command, "sending QMP request");

            if let Err(e) = writer.send_line(&request_json).await {
                // Fail fast: remove pending entry if we couldn't even send.
                let mut pending = self.inner.pending.lock().await;
                pending.remove(&id);
                return Err(e);
            }
        }

        let timeout = options.timeout.or(self.inner.default_timeout);

        let resp = match (timeout, options.cancel) {
            (Some(t), Some(cancel)) => {
                tokio::select! {
                    biased;
                    r = rx => r,
                    _ = tokio::time::sleep(t) => {
                        self.drop_pending(id).await;
                        return Err(Error::Timeout { timeout: t });
                    }
                    _ = cancel.cancelled() => {
                        self.drop_pending(id).await;
                        return Err(Error::Cancelled);
                    }
                }
            }
            (Some(t), None) => {
                tokio::select! {
                    biased;
                    r = rx => r,
                    _ = tokio::time::sleep(t) => {
                        self.drop_pending(id).await;
                        return Err(Error::Timeout { timeout: t });
                    }
                }
            }
            (None, Some(cancel)) => {
                tokio::select! {
                    biased;
                    r = rx => r,
                    _ = cancel.cancelled() => {
                        self.drop_pending(id).await;
                        return Err(Error::Cancelled);
                    }
                }
            }
            (None, None) => rx.await,
        };

        let resp = match resp {
            Ok(r) => r?,
            Err(_closed) => {
                // Sender dropped: reader task ended.
                return Err(Error::Disconnected);
            }
        };

        #[cfg(feature = "tracing")]
        tracing::trace!(id = id, command = command, "received QMP response");

        decode_execute_response::<R>(command, resp)
    }

    pub(crate) async fn execute_raw<A>(
        &self,
        command: &str,
        args: Option<A>,
        options: CallOptions,
    ) -> Result<Value>
    where
        A: Serialize,
    {
        let v: Value = self.execute_with_options(command, args, options).await?;
        Ok(v)
    }

    async fn drop_pending(&self, id: u64) {
        let mut pending = self.inner.pending.lock().await;
        pending.remove(&id);
    }

    fn spawn_reader(&self, reader: BufReader<tokio::io::ReadHalf<crate::transport::QmpStream>>) {
        let inner = self.inner.clone();

        tokio::spawn(async move {
            let mut reader = reader;
            loop {
                let msg: Value = match read_json_line::<Value>(&mut reader).await {
                    Ok(v) => v,
                    Err(e) => {
                        // Connection likely closed or broken.
                        let mut pending = inner.pending.lock().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Err(e.clone_for_task()));
                        }
                        break;
                    }
                };

                // Event?
                if msg.get("event").is_some() {
                    match serde_json::from_value::<EventMessage>(msg) {
                        Ok(ev) => {
                            #[cfg(feature = "tracing")]
                            tracing::trace!(event = %ev.event, "received QMP event");
                            let _ = inner.events_tx.send(Event::from(ev));
                        }
                        Err(_e) => {
                            // Ignore malformed events.
                        }
                    }
                    continue;
                }

                // Response?
                if msg.get("id").is_some() {
                    match serde_json::from_value::<QmpResponse>(msg) {
                        Ok(resp) => {
                            if let Some(id) = resp.id.as_u64() {
                                let tx = {
                                    let mut pending = inner.pending.lock().await;
                                    pending.remove(&id)
                                };

                                if let Some(tx) = tx {
                                    let _ = tx.send(Ok(resp));
                                }
                            }
                        }
                        Err(_e) => {
                            // Ignore.
                        }
                    }

                    continue;
                }

                // Unhandled message kind.
            }
        });
    }
}

fn validate_capabilities_response(resp: QmpResponse) -> Result<()> {
    if resp.error.is_some() {
        return Err(Error::protocol("qmp_capabilities returned an error"));
    }

    Ok(())
}

fn build_capabilities_request(id: u64, enable: &[String]) -> String {
    if enable.is_empty() {
        serde_json::json!({"execute": "qmp_capabilities", "id": id}).to_string()
    } else {
        serde_json::json!({
            "execute": "qmp_capabilities",
            "arguments": { "enable": enable },
            "id": id
        })
        .to_string()
    }
}

fn build_execute_request<A: Serialize>(id: u64, command: &str, args: Option<&A>) -> String {
    match args {
        Some(a) => serde_json::json!({
            "execute": command,
            "arguments": a,
            "id": id
        })
        .to_string(),
        None => serde_json::json!({"execute": command, "id": id}).to_string(),
    }
}

fn decode_execute_response<R: serde::de::DeserializeOwned>(
    command: &str,
    resp: QmpResponse,
) -> Result<R> {
    if let Some(err) = resp.error {
        return Err(Error::qmp(command, err.class, err.desc));
    }

    let value = resp
        .result
        .ok_or_else(|| Error::protocol("missing 'return' field in response"))?;

    deserialize_value::<R>(value).map_err(|e| Error::Protocol {
        message: format!("failed to decode response for '{command}': {e}"),
    })
}

fn deserialize_value<T: serde::de::DeserializeOwned>(v: Value) -> std::result::Result<T, String> {
    let line = v.to_string();
    let mut deserializer = serde_json::Deserializer::from_str(&line);
    match serde_path_to_error::deserialize(&mut deserializer) {
        Ok(t) => Ok(t),
        Err(e) => Err(e.to_string()),
    }
}

async fn read_json_line<T: serde::de::DeserializeOwned>(
    reader: &mut BufReader<tokio::io::ReadHalf<crate::transport::QmpStream>>,
) -> Result<T> {
    let mut line = String::new();
    let n: usize = reader.read_line(&mut line).await.map_err(Error::from)?;
    if n == 0 {
        return Err(Error::Disconnected);
    }

    // Strip trailing newlines.
    let line = line.trim_end_matches(['\r', '\n']);

    serde_json::from_str::<T>(line).map_err(Error::from)
}
