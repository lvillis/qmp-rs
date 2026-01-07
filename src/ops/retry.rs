use std::{fmt, time::Duration};

use crate::error::Error;

/// Abstraction over sleeping, mainly for deterministic tests.
pub trait RetrySleep: Send + Sync {
    /// Sleep for the given duration.
    fn sleep<'a>(
        &'a self,
        duration: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// Default Tokio-based sleep.
#[derive(Debug, Clone, Default)]
pub struct TokioSleep;

impl RetrySleep for TokioSleep {
    fn sleep<'a>(
        &'a self,
        duration: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { tokio::time::sleep(duration).await })
    }
}

/// Retry policy for operations.
#[derive(Clone)]
pub struct RetryPolicy {
    /// Maximum attempts (including the first call).
    pub max_attempts: usize,

    /// Base backoff duration.
    pub base_delay: Duration,

    /// Maximum backoff duration.
    pub max_delay: Duration,

    /// Whether to apply full jitter.
    pub jitter: bool,

    sleep: std::sync::Arc<dyn RetrySleep>,
}

impl RetryPolicy {
    /// No retry.
    #[must_use]
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            base_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(2),
            jitter: true,
            sleep: std::sync::Arc::new(TokioSleep),
        }
    }

    /// A conservative retry policy (good default for transient transport failures).
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            jitter: true,
            sleep: std::sync::Arc::new(TokioSleep),
        }
    }

    /// Replace the sleeping strategy.
    #[must_use]
    pub fn with_sleep(mut self, sleep: std::sync::Arc<dyn RetrySleep>) -> Self {
        self.sleep = sleep;
        self
    }

    /// Whether an error should be retried.
    #[must_use]
    pub fn should_retry(&self, err: &Error) -> bool {
        err.is_retryable()
    }

    /// Compute the delay before the next retry.
    ///
    /// `attempt` is 1-based.
    #[must_use]
    pub fn delay(&self, attempt: usize) -> Duration {
        if attempt <= 1 {
            return Duration::from_secs(0);
        }

        let exp = attempt.saturating_sub(2).min(31);
        let base_ms = self.base_delay.as_millis().min(u128::from(u64::MAX));
        let factor = 1u128.checked_shl(exp as u32).unwrap_or(u128::MAX);
        let mut delay_ms = base_ms.saturating_mul(factor);
        let max_ms = self.max_delay.as_millis().min(u128::from(u64::MAX));
        if delay_ms > max_ms {
            delay_ms = max_ms;
        }

        let delay_ms_u64 = delay_ms as u64;

        let final_ms = if self.jitter && delay_ms_u64 > 0 {
            fastrand::u64(0..=delay_ms_u64)
        } else {
            delay_ms_u64
        };

        Duration::from_millis(final_ms)
    }

    /// Sleep for the given duration.
    pub async fn sleep(&self, duration: Duration) {
        self.sleep.sleep(duration).await
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::conservative()
    }
}

impl fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_attempts", &self.max_attempts)
            .field("base_delay", &self.base_delay)
            .field("max_delay", &self.max_delay)
            .field("jitter", &self.jitter)
            .finish()
    }
}
