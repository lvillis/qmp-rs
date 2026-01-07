use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `error` object returned by QMP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QmpError {
    /// Error class.
    pub class: String,
    /// Error description.
    pub desc: String,
}

/// QMP command response.
///
/// QMP response objects may contain either `return` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QmpResponse {
    /// Request id, echoed by the server.
    pub id: Value,

    /// Success payload.
    #[serde(rename = "return", default)]
    pub result: Option<Value>,

    /// Error payload.
    #[serde(default)]
    pub error: Option<QmpError>,
}
