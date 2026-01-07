//! Socket transport.

use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    client::Endpoint,
    error::{Error, Result},
};

/// Trait object representing an async stream that can be used for QMP I/O.
pub trait AsyncQmpStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> AsyncQmpStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

/// A connected QMP stream.
///
/// Internally this is a boxed stream so the rest of the crate does not care
/// whether the underlying connection is Unix or TCP.
pub type QmpStream = Box<dyn AsyncQmpStream>;

/// Connect to a QMP endpoint.
pub async fn connect(endpoint: Endpoint) -> Result<QmpStream> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix { path } => {
            let s = tokio::net::UnixStream::connect(&path)
                .await
                .map_err(Error::from)?;
            Ok(Box::new(s))
        }
        #[cfg(not(unix))]
        Endpoint::Unix { .. } => Err(Error::protocol(
            "unix sockets are not supported on this platform",
        )),
        Endpoint::Tcp { host, port } => {
            let addr = (host.as_str(), port);
            let s = tokio::net::TcpStream::connect(addr)
                .await
                .map_err(Error::from)?;
            // Best effort: disable Nagle for request/response latency.
            let _ = s.set_nodelay(true);
            Ok(Box::new(s))
        }
    }
}
