//! An injectable monotonic-ish wall clock.
//!
//! Liveness/expiry (issue #25 part 1) compares "now" against each server's
//! `last_verified` instant. Tests must drive that comparison without
//! `std::thread::sleep`, so the clock is a trait object: production uses
//! [`SystemClock`]; tests use [`TestClock`] and advance it explicitly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A source of the current time, in whole seconds since the Unix epoch.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
}

/// Wall-clock time from the operating system.
#[derive(Debug, Default, Clone)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// A manually advanced clock for deterministic tests (no wall-clock sleeps).
#[derive(Debug, Clone)]
pub struct TestClock {
    now: Arc<AtomicU64>,
}

impl TestClock {
    /// Start the clock at `start` seconds since the epoch.
    pub fn new(start: u64) -> Self {
        Self {
            now: Arc::new(AtomicU64::new(start)),
        }
    }

    /// Advance the clock by `secs` seconds.
    pub fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Clock for TestClock {
    fn now_unix(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}
