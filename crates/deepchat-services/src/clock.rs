//! Deterministic time source for schema markers and quarantine directory naming.

use std::time::{SystemTime, UNIX_EPOCH};

/// Time source injected by the composition root so tests stay deterministic.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_millis(&self) -> i64;
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now_millis(&self) -> i64 {
        (**self).now_millis()
    }
}

/// Real wall-clock time.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}
