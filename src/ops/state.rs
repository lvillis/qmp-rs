use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{Mutex, OwnedMutexGuard, oneshot};

use crate::error::{Error, Result};

/// A per-key mutex table.
#[derive(Debug, Default)]
pub(crate) struct LockTable {
    inner: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

#[derive(Debug)]
pub(crate) struct LockGuard {
    _guard: OwnedMutexGuard<()>,
}

impl LockTable {
    pub(crate) async fn lock(&self, key: &str) -> LockGuard {
        let arc = {
            let mut map = self.inner.lock().await;
            match map.get(key) {
                Some(m) => m.clone(),
                None => {
                    let m = Arc::new(Mutex::new(()));
                    map.insert(key.to_string(), m.clone());
                    m
                }
            }
        };

        // `OwnedMutexGuard` keeps the Arc alive.
        let g = arc.lock_owned().await;
        LockGuard { _guard: g }
    }
}

/// Idempotency / in-flight de-duplication store.
#[derive(Debug, Default)]
pub(crate) struct IdempotencyStore {
    inner: Mutex<HashMap<String, IdempotencyEntry>>,
}

#[derive(Debug)]
struct IdempotencyEntry {
    state: IdempotencyState,
    ttl: Option<Duration>,
}

#[derive(Debug)]
enum IdempotencyState {
    InFlight {
        waiters: Vec<oneshot::Sender<Result<Value>>>,
    },
    Completed {
        value: Value,
        at: Instant,
    },
}

impl IdempotencyStore {
    pub(crate) async fn run<F, Fut>(
        &self,
        key: String,
        ttl: Option<Duration>,
        f: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Value>>,
    {
        // Fast path: check existing entry.
        {
            let mut map = self.inner.lock().await;
            if let Some(entry) = map.get_mut(&key) {
                // Update TTL if provided by the caller.
                if ttl.is_some() {
                    entry.ttl = ttl;
                }

                match &mut entry.state {
                    IdempotencyState::Completed { value, at } => {
                        if !is_expired(*at, entry.ttl) {
                            return Ok(value.clone());
                        }
                        // Expired -> remove and fall through to start a new execution.
                        map.remove(&key);
                    }
                    IdempotencyState::InFlight { waiters } => {
                        let (tx, rx) = oneshot::channel();
                        waiters.push(tx);
                        drop(map);
                        return match rx.await {
                            Ok(res) => res,
                            Err(_closed) => Err(Error::Disconnected),
                        };
                    }
                }
            }

            // No entry -> install an in-flight marker.
            map.insert(
                key.clone(),
                IdempotencyEntry {
                    state: IdempotencyState::InFlight {
                        waiters: Vec::new(),
                    },
                    ttl,
                },
            );
        }

        // Execute the operation without holding the lock.
        let result = f().await;

        // Publish result to waiters.
        let mut map = self.inner.lock().await;
        let entry = map.get_mut(&key);
        match entry {
            Some(entry) => {
                // Take waiters.
                let waiters = match std::mem::replace(
                    &mut entry.state,
                    IdempotencyState::InFlight {
                        waiters: Vec::new(),
                    },
                ) {
                    IdempotencyState::InFlight { waiters } => waiters,
                    IdempotencyState::Completed { value, at } => {
                        // Unexpected: someone replaced the state while we were executing.
                        // Keep the completed value.
                        entry.state = IdempotencyState::Completed { value, at };
                        Vec::new()
                    }
                };

                match &result {
                    Ok(v) => {
                        entry.state = IdempotencyState::Completed {
                            value: v.clone(),
                            at: Instant::now(),
                        };
                    }
                    Err(_e) => {
                        // On error, remove the entry so the next call can retry.
                        map.remove(&key);
                    }
                }

                for w in waiters {
                    match &result {
                        Ok(v) => {
                            let _ = w.send(Ok(v.clone()));
                        }
                        Err(e) => {
                            let _ = w.send(Err(e.clone_for_task()));
                        }
                    }
                }

                result
            }
            None => {
                // Entry disappeared; return original result.
                result
            }
        }
    }
}

pub(crate) fn default_idempotency_key(command: &str, args: Option<&Value>) -> String {
    match args {
        Some(v) => format!("{command}:{v}"),
        None => command.to_string(),
    }
}

fn is_expired(at: Instant, ttl: Option<Duration>) -> bool {
    match ttl {
        Some(ttl) => at.elapsed() >= ttl,
        None => false,
    }
}
