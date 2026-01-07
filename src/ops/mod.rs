//! "Safe ops" layer.
//!
//! The core [`crate::Client`] is a thin, low-level QMP transport.
//!
//! [`OpsClient`] builds on top of it and adds:
//! - allow-list policies (command white-list)
//! - per-command mutex / idempotent execution
//! - retries with exponential backoff (and jitter)
//! - structured errors and observability hooks

mod policy;
mod retry;
mod state;
mod types;

pub use policy::{CommandMode, CommandRule, Policy, PolicyBuilder, Profile};
pub use retry::{RetryPolicy, RetrySleep};
pub use types::{QueryStatus, QueryVersion};

use serde::Serialize;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

use crate::{
    cancel::CancelToken,
    client::{CallOptions, Client},
    error::{Error, Result},
};

/// Options for a single operation call through [`OpsClient`].
#[derive(Debug, Clone, Default)]
pub struct OpsCallOptions {
    /// Override timeout for this call.
    pub timeout: Option<Duration>,

    /// Cancellation token.
    pub cancel: Option<CancelToken>,
}

/// A higher-level client intended for "production operations".
///
/// This wrapper intentionally focuses on safety and reliability:
/// - explicit allow-list
/// - idempotency/mutex
/// - retries/backoff
/// - structured error mapping
#[derive(Clone, Debug)]
pub struct OpsClient {
    client: Client,
    policy: Policy,
    locks: Arc<state::LockTable>,
    idempotency: Arc<state::IdempotencyStore>,
}

impl OpsClient {
    /// Create an ops client with the given policy.
    #[must_use]
    pub fn new(client: Client, policy: Policy) -> Self {
        Self {
            client,
            policy,
            locks: Arc::new(state::LockTable::default()),
            idempotency: Arc::new(state::IdempotencyStore::default()),
        }
    }

    /// Convenience: build an ops client from a predefined profile.
    #[must_use]
    pub fn from_profile(client: Client, profile: Profile) -> Self {
        let policy = PolicyBuilder::new().profile(profile).build();
        Self::new(client, policy)
    }

    /// Access the underlying low-level client.
    #[must_use]
    pub fn raw(&self) -> &Client {
        &self.client
    }

    /// Execute an allow-listed command and return raw JSON.
    pub async fn call_json<A>(
        &self,
        command: &str,
        args: Option<A>,
        options: OpsCallOptions,
    ) -> Result<Value>
    where
        A: Serialize,
    {
        let rule = self.policy.rule_for(command)?;

        // Per-command mutual exclusion.
        let _guard = match &rule.mode {
            CommandMode::Mutex { key } => Some(self.locks.lock(key).await),
            CommandMode::Idempotent {
                mutex_key: Some(k), ..
            } => Some(self.locks.lock(k).await),
            _ => None,
        };

        // Serialize args once for idempotency key and transport.
        let args_value: Option<Value> = match args {
            Some(a) => Some(serde_json::to_value(a).map_err(Error::from)?),
            None => None,
        };

        let call_opts = CallOptions {
            timeout: options.timeout.or(rule.timeout),
            cancel: options.cancel,
        };

        match &rule.mode {
            CommandMode::Idempotent { key, ttl, .. } => {
                let k = key.clone().unwrap_or_else(|| {
                    state::default_idempotency_key(command, args_value.as_ref())
                });

                self.idempotency
                    .run(k, *ttl, || async {
                        self.retry_call_json(
                            command,
                            args_value.clone(),
                            call_opts.clone(),
                            rule.retry.clone(),
                        )
                        .await
                    })
                    .await
            }
            _ => {
                self.retry_call_json(command, args_value, call_opts, rule.retry.clone())
                    .await
            }
        }
    }

    /// Execute a command and deserialize the `return` value into `R`.
    pub async fn call<A, R>(
        &self,
        command: &str,
        args: Option<A>,
        options: OpsCallOptions,
    ) -> Result<R>
    where
        A: Serialize,
        R: serde::de::DeserializeOwned,
    {
        let v = self.call_json(command, args, options).await?;
        serde_json::from_value::<R>(v).map_err(Error::from)
    }

    async fn retry_call_json(
        &self,
        command: &str,
        args_value: Option<Value>,
        call_opts: CallOptions,
        retry: RetryPolicy,
    ) -> Result<Value> {
        let mut attempt: usize = 0;

        loop {
            attempt = attempt.saturating_add(1);

            let res = self
                .client
                .execute_raw(command, args_value.clone(), call_opts.clone())
                .await;

            match res {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if attempt >= retry.max_attempts {
                        return Err(e);
                    }

                    if !retry.should_retry(&e) {
                        return Err(e);
                    }

                    let delay = retry.delay(attempt);

                    #[cfg(feature = "tracing")]
                    tracing::warn!(
                        command = command,
                        attempt = attempt,
                        max_attempts = retry.max_attempts,
                        delay_ms = delay.as_millis(),
                        error = %e,
                        "retrying QMP command"
                    );

                    retry.sleep(delay).await;
                }
            }
        }
    }

    /// A typed helper: `query-status`.
    pub async fn query_status(&self) -> Result<QueryStatus> {
        self.call::<(), QueryStatus>("query-status", None, OpsCallOptions::default())
            .await
    }

    /// A typed helper: `query-version`.
    pub async fn query_version(&self) -> Result<QueryVersion> {
        self.call::<(), QueryVersion>("query-version", None, OpsCallOptions::default())
            .await
    }
}
