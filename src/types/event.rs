use serde::{Deserialize, Serialize};
use serde_json::Value;

/// QMP timestamp, typically included in event messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    /// Seconds since epoch.
    pub seconds: i64,

    /// Microseconds within the second.
    pub microseconds: i64,
}

/// A decoded event.
///
/// This type is intentionally _lossless_:
/// - `name` keeps the original event name.
/// - `data` keeps an arbitrary JSON payload.
///
/// You can deserialize `data` into your own strongly typed struct when needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Event name.
    pub name: String,

    /// Event data payload.
    #[serde(default)]
    pub data: Value,

    /// Optional timestamp.
    pub timestamp: Option<Timestamp>,
}

/// Raw event message as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventMessage {
    /// Event name.
    #[serde(rename = "event")]
    pub name: String,

    /// Event data payload.
    #[serde(default)]
    pub data: Value,

    /// Timestamp.
    pub timestamp: Option<Timestamp>,
}

impl From<EventMessage> for Event {
    fn from(msg: EventMessage) -> Self {
        Self {
            name: msg.name,
            data: msg.data,
            timestamp: msg.timestamp,
        }
    }
}
