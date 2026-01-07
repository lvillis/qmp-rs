use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::error::{Error, Result};

/// Predefined safety profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Profile {
    /// Only read-only, query-style commands.
    ReadOnly,

    /// Read-only + basic VM lifecycle operations (pause/continue/powerdown/reset).
    VmControl,

    /// No allow-list enforcement (dangerous, mostly for testing / power users).
    Unrestricted,
}

/// How a command should be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandMode {
    /// Execute normally.
    Plain,

    /// Ensure only one command with the same `key` runs at a time.
    Mutex {
        /// Mutex key.
        key: String,
    },

    /// Treat identical calls as idempotent within a time window.
    ///
    /// This mode deduplicates concurrent calls and optionally caches the last
    /// successful result (TTL based).
    Idempotent {
        /// Optional explicit key.
        ///
        /// When `None`, a key is derived from `{command}+{arguments}`.
        key: Option<String>,

        /// Optional mutual-exclusion key (in addition to idempotency).
        mutex_key: Option<String>,

        /// Cache TTL for successful results.
        ttl: Option<Duration>,
    },
}

/// Per-command policy.
#[derive(Debug, Clone)]
pub struct CommandRule {
    /// Execution mode.
    pub mode: CommandMode,

    /// Retry policy.
    pub retry: crate::ops::RetryPolicy,

    /// Optional timeout override.
    pub timeout: Option<Duration>,
}

impl Default for CommandRule {
    fn default() -> Self {
        Self {
            mode: CommandMode::Plain,
            retry: crate::ops::RetryPolicy::none(),
            timeout: None,
        }
    }
}

/// Runtime policy.
#[derive(Debug, Clone)]
pub struct Policy {
    pub(crate) enforce_allow_list: bool,
    allowed: HashSet<String>,
    rules: HashMap<String, CommandRule>,
    default_rule: CommandRule,
}

impl Policy {
    /// Whether a command is allowed by the policy.
    #[must_use]
    pub fn is_allowed(&self, command: &str) -> bool {
        self.allowed.contains(command)
    }

    /// Get the rule for a command.
    ///
    /// This also performs basic validation such as known profile restrictions.
    pub(crate) fn rule_for(&self, command: &str) -> Result<CommandRule> {
        if self.enforce_allow_list && !self.is_allowed(command) {
            return Err(Error::policy(command, "command is not allowed"));
        }

        Ok(self
            .rules
            .get(command)
            .cloned()
            .unwrap_or_else(|| self.default_rule.clone()))
    }
}

/// Builder for [`Policy`].
#[derive(Debug, Clone)]
pub struct PolicyBuilder {
    profile: Profile,
    enforce_allow_list: bool,
    allowed: HashSet<String>,
    rules: HashMap<String, CommandRule>,
    default_rule: CommandRule,
}

impl PolicyBuilder {
    /// Create a new policy builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            profile: Profile::ReadOnly,
            enforce_allow_list: true,
            allowed: HashSet::new(),
            rules: HashMap::new(),
            default_rule: CommandRule::default(),
        }
    }

    /// Use a predefined profile.
    #[must_use]
    pub fn profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }

    /// Enable / disable allow-list enforcement.
    ///
    /// Disabling this is **dangerous**.
    #[must_use]
    pub fn enforce_allow_list(mut self, enabled: bool) -> Self {
        self.enforce_allow_list = enabled;
        self
    }

    /// Explicitly allow a command.
    #[must_use]
    pub fn allow(mut self, command: impl Into<String>) -> Self {
        self.allowed.insert(command.into());
        self
    }

    /// Set a per-command rule.
    #[must_use]
    pub fn rule(mut self, command: impl Into<String>, rule: CommandRule) -> Self {
        self.rules.insert(command.into(), rule);
        self
    }

    /// Set the default rule (used when no per-command rule exists).
    #[must_use]
    pub fn default_rule(mut self, rule: CommandRule) -> Self {
        self.default_rule = rule;
        self
    }

    /// Build the policy.
    #[must_use]
    pub fn build(mut self) -> Policy {
        // Apply profile defaults.
        match self.profile {
            Profile::ReadOnly => {
                self.allowed
                    .extend(read_only_allow_list().into_iter().map(|s| s.to_string()));
            }
            Profile::VmControl => {
                self.allowed
                    .extend(read_only_allow_list().into_iter().map(|s| s.to_string()));
                self.allowed
                    .extend(vm_control_allow_list().into_iter().map(|s| s.to_string()));

                // Pause/resume should be mutually exclusive with itself.
                self.rules
                    .entry("stop".to_string())
                    .or_insert_with(|| CommandRule {
                        mode: CommandMode::Mutex {
                            key: "vm-control".to_string(),
                        },
                        retry: crate::ops::RetryPolicy::conservative(),
                        timeout: Some(Duration::from_secs(10)),
                    });

                self.rules
                    .entry("cont".to_string())
                    .or_insert_with(|| CommandRule {
                        mode: CommandMode::Mutex {
                            key: "vm-control".to_string(),
                        },
                        retry: crate::ops::RetryPolicy::conservative(),
                        timeout: Some(Duration::from_secs(10)),
                    });
            }
            Profile::Unrestricted => {
                self.enforce_allow_list = false;
            }
        }

        Policy {
            enforce_allow_list: self.enforce_allow_list,
            allowed: self.allowed,
            rules: self.rules,
            default_rule: self.default_rule,
        }
    }
}

impl Default for PolicyBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn read_only_allow_list() -> [&'static str; 6] {
    [
        "query-status",
        "query-version",
        "query-kvm",
        "query-machines",
        "query-cpus-fast",
        "query-block",
    ]
}

fn vm_control_allow_list() -> [&'static str; 4] {
    ["stop", "cont", "system_powerdown", "system_reset"]
}
