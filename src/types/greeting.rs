use serde::{Deserialize, Serialize};

/// QMP greeting message.
///
/// QMP sends this as the very first JSON object after the socket is connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Greeting {
    /// QMP meta information.
    #[serde(rename = "QMP")]
    pub qmp: QmpInfo,
}

/// `QMP` section in the greeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QmpInfo {
    /// Server version.
    pub version: QmpVersion,

    /// Supported capabilities.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// QEMU version information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QmpVersion {
    /// QEMU package version.
    pub qemu: QmpVersionNumber,

    /// Package string (when available).
    #[serde(default)]
    pub package: String,
}

/// Numeric version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QmpVersionNumber {
    /// Major version.
    pub major: u64,
    /// Minor version.
    pub minor: u64,
    /// Micro version.
    pub micro: u64,
}
