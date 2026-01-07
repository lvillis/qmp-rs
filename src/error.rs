//! Error model for the `qmp` crate.

use std::time::Duration;

use thiserror::Error;

/// Convenience result type.
pub type Result<T> = std::result::Result<T, Error>;

/// High-level error classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// An I/O level failure (socket, EOF, etc.).
    Io,
    /// JSON encoding/decoding failure.
    Json,
    /// QMP protocol violation or unexpected message.
    Protocol,
    /// QMP returned an error object.
    Qmp,
    /// The connection was closed.
    Disconnected,
    /// The call timed out.
    Timeout,
    /// The call was cancelled.
    Cancelled,
    /// A safety policy rejected an operation.
    Policy,
    /// The event stream receiver fell behind and dropped messages.
    EventLagged,
}

/// Structured error type.
///
/// This type is designed to be:
/// - **diagnosable** (keeps context when available)
/// - **safe by default** (no sensitive data in `Display`)
/// - **extensible** (`#[non_exhaustive]`)
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Socket / file I/O error.
    #[error("I/O error: {source}")]
    Io {
        /// Lower-level error.
        #[from]
        source: std::io::Error,
    },

    /// JSON serialization/deserialization error.
    #[error("JSON error: {source}")]
    Json {
        /// Lower-level error.
        #[from]
        source: serde_json::Error,
    },

    /// The QMP peer sent an unexpected or invalid message.
    #[error("QMP protocol error: {message}")]
    Protocol {
        /// Human readable message.
        message: String,
    },

    /// QMP returned an error for an `execute` request.
    #[error("QMP command failed: {class}: {desc}")]
    Qmp {
        /// Command name.
        command: String,
        /// QMP error class.
        class: String,
        /// QMP error description.
        desc: String,
    },

    /// The connection closed while a request was in-flight.
    #[error("QMP connection closed")]
    Disconnected,

    /// A command call exceeded the configured timeout.
    #[error("QMP command timed out after {timeout:?}")]
    Timeout {
        /// Timeout value.
        timeout: Duration,
    },

    /// A command call was cancelled.
    #[error("QMP command cancelled")]
    Cancelled,

    /// A safety policy rejected an operation.
    #[error("operation rejected by policy: {command}")]
    PolicyViolation {
        /// Command name.
        command: String,
        /// Reason.
        reason: String,
    },

    /// The event receiver lagged behind and dropped events.
    #[error("event stream lagged behind and dropped {missed} events")]
    EventLagged {
        /// How many events were dropped.
        missed: usize,
    },
}

impl Error {
    /// Returns a coarse error classification.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Io { .. } => ErrorKind::Io,
            Self::Json { .. } => ErrorKind::Json,
            Self::Protocol { .. } => ErrorKind::Protocol,
            Self::Qmp { .. } => ErrorKind::Qmp,
            Self::Disconnected => ErrorKind::Disconnected,
            Self::Timeout { .. } => ErrorKind::Timeout,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::PolicyViolation { .. } => ErrorKind::Policy,
            Self::EventLagged { .. } => ErrorKind::EventLagged,
        }
    }

    /// Whether this error is likely retryable at the transport/operation layer.
    ///
    /// Note: QMP command errors are typically **not** retryable, unless you
    /// implement a higher-level policy that treats certain classes as retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind(),
            ErrorKind::Io | ErrorKind::Disconnected | ErrorKind::Timeout
        )
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: message.into(),
        }
    }

    pub(crate) fn qmp(
        command: impl Into<String>,
        class: impl Into<String>,
        desc: impl Into<String>,
    ) -> Self {
        Self::Qmp {
            command: command.into(),
            class: class.into(),
            desc: desc.into(),
        }
    }

    pub(crate) fn policy(command: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::PolicyViolation {
            command: command.into(),
            reason: reason.into(),
        }
    }

    /// Create a safe, owned copy of this error suitable for broadcasting across tasks.
    ///
    /// `Error` is intentionally not `Clone`. Some internal components (such as the
    /// reader loop) need to notify multiple waiters; this helper provides a
    /// best-effort owned copy while keeping the error structure.
    pub(crate) fn clone_for_task(&self) -> Self {
        match self {
            Self::Io { .. } => Self::Disconnected,
            Self::Json { .. } => Self::Disconnected,
            Self::Protocol { message } => Self::Protocol {
                message: message.clone(),
            },
            Self::Qmp {
                command,
                class,
                desc,
            } => Self::Qmp {
                command: command.clone(),
                class: class.clone(),
                desc: desc.clone(),
            },
            Self::Disconnected => Self::Disconnected,
            Self::Timeout { timeout } => Self::Timeout { timeout: *timeout },
            Self::Cancelled => Self::Cancelled,
            Self::PolicyViolation { command, reason } => Self::PolicyViolation {
                command: command.clone(),
                reason: reason.clone(),
            },
            Self::EventLagged { missed } => Self::EventLagged { missed: *missed },
        }
    }
}
