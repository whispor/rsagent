//! A per-source circuit breaker.
//!
//! A source that cannot be reached should stop being called for a while, instead of
//! burning a timeout on every tool call. This is the "自动排除调用故障及修复" half of tool
//! extensibility (see `docs/EXTENSIBILITY.md` §3.5).
//!
//! Only **connection-class** failures count: unreachable, protocol error, timeout. A tool
//! that runs fine and returns a business error is *working* — counting those would let one
//! flaky tool trip its whole source and hide the real problem (decision D4).
//!
//! Time is passed in as an [`Instant`] rather than read from the clock inside, so the
//! whole state machine is tested deterministically without sleeping.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What a breaker is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal: calls pass through.
    Closed,
    /// Tripped: calls are refused until the cooldown elapses.
    Open,
    /// Cooldown elapsed: exactly one probe call is allowed to test the source.
    HalfOpen,
}

/// When to trip, and how long to stay tripped.
#[derive(Debug, Clone, Copy)]
pub struct BreakerPolicy {
    /// Consecutive connection failures that trip the breaker.
    pub threshold: u32,
    /// Cooldown after the first trip.
    pub base_cooldown: Duration,
    /// Ceiling for the cooldown, which doubles on each failed probe.
    pub max_cooldown: Duration,
}

impl Default for BreakerPolicy {
    fn default() -> Self {
        BreakerPolicy {
            threshold: 3,
            base_cooldown: Duration::from_secs(60),
            max_cooldown: Duration::from_secs(600),
        }
    }
}

/// The mutable part, behind one mutex so the breaker is `Sync`.
#[derive(Debug)]
struct Inner {
    state: BreakerState,
    failures: u32,
    cooldown: Duration,
    opened_at: Option<Instant>,
}

/// A circuit breaker guarding one [`ToolSource`](crate::ToolSource).
///
/// Share it with `Arc`; every method takes `&self`.
#[derive(Debug)]
pub struct CircuitBreaker {
    policy: BreakerPolicy,
    inner: Mutex<Inner>,
}

impl CircuitBreaker {
    /// A breaker that starts [`Closed`](BreakerState::Closed).
    pub fn new(policy: BreakerPolicy) -> Self {
        CircuitBreaker {
            policy,
            inner: Mutex::new(Inner {
                state: BreakerState::Closed,
                failures: 0,
                cooldown: policy.base_cooldown,
                opened_at: None,
            }),
        }
    }

    /// The policy this breaker was built with.
    pub fn policy(&self) -> BreakerPolicy {
        self.policy
    }

    /// The state at `now`, applying the cooldown transition without recording a probe.
    ///
    /// Reading the state never consumes the half-open probe; only [`allows`](Self::allows)
    /// does. This makes `state` safe to call from a status display.
    pub fn state(&self, now: Instant) -> BreakerState {
        let inner = self.lock();
        effective(&inner, now)
    }

    /// Whether a call may proceed at `now`.
    ///
    /// Returning `true` while [`HalfOpen`](BreakerState::HalfOpen) *grants the single
    /// probe*: the next call is let through and every call after it is refused until the
    /// probe reports back with [`record_success`](Self::record_success) or
    /// [`record_failure`](Self::record_failure).
    pub fn allows(&self, now: Instant) -> bool {
        let mut inner = self.lock();
        match inner.state {
            BreakerState::Closed => true,
            // A probe is already in flight; wait for its outcome.
            BreakerState::HalfOpen => false,
            BreakerState::Open => {
                let elapsed = inner
                    .opened_at
                    .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
                if elapsed >= inner.cooldown {
                    // Cooldown is over: consume the probe and let this one call through.
                    inner.state = BreakerState::HalfOpen;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful round-trip: the source is reachable again.
    pub fn record_success(&self) {
        let mut inner = self.lock();
        inner.state = BreakerState::Closed;
        inner.failures = 0;
        inner.cooldown = self.policy.base_cooldown;
        inner.opened_at = None;
    }

    /// Record a connection-class failure at `now`.
    ///
    /// Business failures must not be reported here — see the module docs.
    pub fn record_failure(&self, now: Instant) {
        let mut inner = self.lock();
        match inner.state {
            // A failed probe means the source is still down; back off twice as long.
            BreakerState::HalfOpen => {
                inner.cooldown = (inner.cooldown * 2).min(self.policy.max_cooldown);
                inner.state = BreakerState::Open;
                inner.opened_at = Some(now);
            }
            // Already open: refresh the window so the cooldown starts from the last failure.
            BreakerState::Open => {
                inner.opened_at = Some(now);
            }
            BreakerState::Closed => {
                inner.failures += 1;
                if inner.failures >= self.policy.threshold {
                    inner.state = BreakerState::Open;
                    inner.cooldown = self.policy.base_cooldown;
                    inner.opened_at = Some(now);
                }
            }
        }
    }

    /// How long until the next probe is allowed, or `None` when calls pass now.
    pub fn retry_after(&self, now: Instant) -> Option<Duration> {
        let inner = self.lock();
        if effective(&inner, now) != BreakerState::Open {
            return None;
        }
        let elapsed = inner
            .opened_at
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        Some(inner.cooldown.saturating_sub(elapsed))
    }

    /// Force the breaker [`Closed`](BreakerState::Closed) — the manual reset.
    pub fn reset(&self) {
        self.record_success();
    }

    /// Lock the mutex, treating poisoning as "the state is still usable".
    ///
    /// A panic elsewhere must not make a breaker permanently unusable; the worst case of a
    /// half-updated counter is one extra or one missing probe.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The state at `now`, with the cooldown transition applied but not written back.
fn effective(inner: &Inner, now: Instant) -> BreakerState {
    match inner.state {
        BreakerState::Open => {
            let elapsed = inner
                .opened_at
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
            if elapsed >= inner.cooldown {
                BreakerState::HalfOpen
            } else {
                BreakerState::Open
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{BreakerPolicy, BreakerState, CircuitBreaker};
    use std::time::{Duration, Instant};

    /// A fast policy, so tests read in milliseconds rather than minutes.
    fn policy() -> BreakerPolicy {
        BreakerPolicy {
            threshold: 3,
            base_cooldown: Duration::from_secs(60),
            max_cooldown: Duration::from_secs(600),
        }
    }

    #[test]
    fn stays_closed_below_the_threshold() {
        let breaker = CircuitBreaker::new(policy());
        let now = Instant::now();
        breaker.record_failure(now);
        breaker.record_failure(now);
        assert_eq!(breaker.state(now), BreakerState::Closed);
        assert!(breaker.allows(now));
    }

    #[test]
    fn opens_at_the_threshold_and_refuses_calls() {
        let breaker = CircuitBreaker::new(policy());
        let now = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(now);
        }
        assert_eq!(breaker.state(now), BreakerState::Open);
        assert!(!breaker.allows(now));
        assert_eq!(
            breaker.retry_after(now),
            Some(Duration::from_secs(60)),
            "the full cooldown remains"
        );
    }

    #[test]
    fn half_opens_after_the_cooldown_and_grants_exactly_one_probe() {
        let breaker = CircuitBreaker::new(policy());
        let start = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(start);
        }

        let later = start + Duration::from_secs(60);
        assert_eq!(breaker.state(later), BreakerState::HalfOpen);
        assert!(breaker.allows(later), "the probe is granted");
        assert!(!breaker.allows(later), "a second call is refused");
        assert!(!breaker.allows(later + Duration::from_secs(1)));
    }

    #[test]
    fn a_successful_probe_closes_the_breaker() {
        let breaker = CircuitBreaker::new(policy());
        let start = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(start);
        }
        let later = start + Duration::from_secs(60);
        assert!(breaker.allows(later));

        breaker.record_success();
        assert_eq!(breaker.state(later), BreakerState::Closed);
        assert!(breaker.allows(later));
    }

    #[test]
    fn a_failed_probe_doubles_the_cooldown() {
        let breaker = CircuitBreaker::new(policy());
        let start = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(start);
        }
        let later = start + Duration::from_secs(60);
        assert!(breaker.allows(later));
        breaker.record_failure(later);

        assert_eq!(breaker.state(later), BreakerState::Open);
        assert_eq!(
            breaker.retry_after(later),
            Some(Duration::from_secs(120)),
            "60s doubled to 120s"
        );
    }

    #[test]
    fn the_cooldown_is_capped() {
        let breaker = CircuitBreaker::new(policy());
        let mut now = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(now);
        }
        // Keep failing probes; the cooldown doubles until it hits the cap.
        for _ in 0..10 {
            now += Duration::from_secs(3600);
            assert!(breaker.allows(now), "each cooldown grants one probe");
            breaker.record_failure(now);
        }
        now += Duration::from_secs(3600);
        assert!(breaker.allows(now));
        breaker.record_failure(now);
        assert_eq!(
            breaker.retry_after(now),
            Some(Duration::from_secs(600)),
            "capped at max_cooldown"
        );
    }

    #[test]
    fn reset_closes_immediately() {
        let breaker = CircuitBreaker::new(policy());
        let now = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(now);
        }
        assert!(!breaker.allows(now));

        breaker.reset();
        assert_eq!(breaker.state(now), BreakerState::Closed);
        assert!(breaker.allows(now));
        assert_eq!(breaker.retry_after(now), None);
    }
}
