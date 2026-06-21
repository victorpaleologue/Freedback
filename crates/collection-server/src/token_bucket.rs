//! A per-host token bucket for polite upstream rate limiting.
//!
//! `refill_per_sec == 0.0` makes the bucket a hard cap (no refill), which is
//! useful both as a strict budget and for deterministic tests.

use std::time::Instant;

/// A classic token bucket.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl TokenBucket {
    /// Create a full bucket with `capacity` tokens refilling at `refill_per_sec`.
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            refill_per_sec,
            last: Instant::now(),
        }
    }

    /// Try to take one token (refilling against the wall clock first).
    pub fn try_acquire(&mut self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    /// Try to take one token, refilling based on `now` (testable).
    pub fn try_acquire_at(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn hard_cap_when_no_refill() {
        let mut b = TokenBucket::new(2.0, 0.0);
        assert!(b.try_acquire());
        assert!(b.try_acquire());
        assert!(!b.try_acquire(), "third acquire exceeds the budget");
        assert!(!b.try_acquire());
    }

    #[test]
    fn refills_over_time() {
        let start = Instant::now();
        let mut b = TokenBucket::new(1.0, 1.0); // 1 token/sec
        assert!(b.try_acquire_at(start));
        assert!(!b.try_acquire_at(start), "empty immediately after");
        // One second later, one token has refilled.
        assert!(b.try_acquire_at(start + Duration::from_secs(1)));
    }

    #[test]
    fn refill_is_capped_at_capacity() {
        let start = Instant::now();
        let mut b = TokenBucket::new(2.0, 100.0);
        // A long gap should not overflow beyond capacity.
        assert!(b.try_acquire_at(start + Duration::from_secs(10)));
        assert!(b.try_acquire_at(start + Duration::from_secs(10)));
        assert!(!b.try_acquire_at(start + Duration::from_secs(10)));
    }
}
