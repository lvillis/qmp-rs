//! Cooperative cancellation.
//!
//! This crate avoids exposing a Tokio-specific cancellation token in the public API.
//! The implementation is currently backed by `tokio::sync::Notify`.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

/// A clonable cancellation token.
///
/// Clones share the same cancellation state.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
}

impl CancelToken {
    /// Create a new, non-cancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::default()),
        }
    }

    /// Cancel this token.
    pub fn cancel(&self) {
        // A relaxed store is sufficient: we only need to propagate the boolean.
        if !self.inner.cancelled.swap(true, Ordering::Relaxed) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Returns `true` if the token has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Relaxed)
    }

    /// A future that resolves when the token is cancelled.
    ///
    /// The returned future is `Send` and does **not** expose Tokio types.
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let token = self.clone();
        async move {
            if token.is_cancelled() {
                return;
            }

            // Wait until someone calls `cancel()`.
            loop {
                token.inner.notify.notified().await;
                if token.is_cancelled() {
                    return;
                }
            }
        }
    }
}
