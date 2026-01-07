use serde::{Deserialize, Serialize};

/// Response of `query-status`.
///
/// This type models only the most stable fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryStatus {
    /// Whether the VM is running.
    pub running: bool,

    /// Single-step mode.
    #[serde(default)]
    pub singlestep: bool,

    /// Human-readable status string (e.g. "running", "paused").
    pub status: String,
}

/// Response of `query-version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryVersion {
    /// QEMU version numbers.
    pub qemu: crate::types::QmpVersionNumber,

    /// Package string.
    #[serde(default)]
    pub package: String,
}
