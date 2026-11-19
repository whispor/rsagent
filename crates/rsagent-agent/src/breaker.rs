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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What a breaker is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BreakerState {
    /// Normal: calls pass through.
    Closed,
    /// Tripped: calls are refused until the cooldown elapses.
    Open,
    /// Cooldown elapsed: exactly one probe call is allowed to test the source.
    HalfOpen,
}

/// A serializable snapshot of a breaker, for persistence and status display.
///
/// The live breaker measures time with [`Instant`] (monotonic, but meaningless across
/// processes). A report therefore records wall-clock seconds, so the CLI can persist it
/// and rebuild the breaker in a later run (`rsagent tools list` / `doctor` / `reset`,
/// `docs/EXTENSIBILITY.md` §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakerReport {
    /// The effective state at snapshot time.
    pub state: BreakerState,
    /// Consecutive connection-class failures recorded while closed.
    pub failures: u32,
    /// Unix seconds when the breaker last opened, if it ever did.
    pub opened_at_unix: Option<u64>,
    /// The cooldown in force at snapshot time, in seconds.
    pub cooldown_secs: u64,
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
    /// When the in-flight half-open probe was granted, if one is outstanding.
    ///
    /// Used to bound how long a probe may take: a probe whose future is dropped (a
    /// cancelled turn, an aborted task) would otherwise wedge the breaker in `HalfOpen`
    /// forever, because only an outcome leaves that state.
    probe_started_at: Option<Instant>,
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
                probe_started_at: None,
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
    ///
    /// A probe that never reports back is not allowed to wedge the breaker: once the
    /// probe window (one cooldown) has elapsed with no outcome, the breaker returns to
    /// [`Open`](BreakerState::Open) with a fresh cooldown, so the source can be probed
    /// again instead of staying unusable forever.
    pub fn allows(&self, now: Instant) -> bool {
        let mut inner = self.lock();
        match inner.state {
            BreakerState::Closed => true,
            BreakerState::HalfOpen => {
                if elapsed_since(inner.probe_started_at, now) >= inner.cooldown {
                    // The probe was abandoned (its future dropped). Re-arm so the source
                    // heals after another cooldown rather than staying half-open forever.
                    inner.state = BreakerState::Open;
                    inner.opened_at = Some(now);
                    inner.probe_started_at = None;
                }
                // Either way no call passes now: a probe is in flight, or just expired.
                false
            }
            BreakerState::Open => {
                if elapsed_since(inner.opened_at, now) >= inner.cooldown {
                    // Cooldown is over: consume the probe and let this one call through.
                    inner.state = BreakerState::HalfOpen;
                    inner.probe_started_at = Some(now);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful round-trip: the source is reachable again.
    ///
    /// A success only resets a breaker that is [`Closed`](BreakerState::Closed) or
    /// [`HalfOpen`](BreakerState::HalfOpen). When the breaker is already
    /// [`Open`](BreakerState::Open) the success belongs to a request issued *before* the
    /// trip (a concurrent source's late reply); honouring it would close a breaker that
    /// just tripped and defeat the isolation.
    pub fn record_success(&self) {
        let mut inner = self.lock();
        match inner.state {
            BreakerState::Closed | BreakerState::HalfOpen => close(&mut inner, self.policy),
            BreakerState::Open => {}
        }
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
                inner.probe_started_at = None;
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
        let mut inner = self.lock();
        close(&mut inner, self.policy);
    }

    /// Snapshot the breaker as a serializable [`BreakerReport`].
    ///
    /// Reading never consumes a half-open probe, so this is safe to call from a status
    /// display or a persistence step.
    pub fn report(&self) -> BreakerReport {
        let now = Instant::now();
        let inner = self.lock();
        BreakerReport {
            state: effective(&inner, now),
            failures: inner.failures,
            opened_at_unix: inner
                .opened_at
                .map(|at| unix_now().saturating_sub(now.saturating_duration_since(at).as_secs())),
            cooldown_secs: inner.cooldown.as_secs(),
        }
    }

    /// Rebuild the breaker from a persisted [`BreakerReport`].
    ///
    /// The cooldown is measured from the report's wall-clock timestamp, so a breaker that
    /// was open when the process exited is still open (or, once its cooldown has passed,
    /// half-open) in the next run. A report that no longer fits the current uptime simply
    /// re-arms a full cooldown rather than panicking.
    pub fn restore(&self, report: &BreakerReport) {
        let now = Instant::now();
        let mut inner = self.lock();
        inner.failures = report.failures;
        inner.cooldown = Duration::from_secs(report.cooldown_secs);
        match report.state {
            BreakerState::Closed => {
                inner.state = BreakerState::Closed;
                inner.opened_at = None;
            }
            // Both open and half-open restore as "open at the recorded time"; `effective`
            // then decides whether the cooldown has already elapsed (half-open) or not.
            BreakerState::Open | BreakerState::HalfOpen => {
                let opened_at_unix = report.opened_at_unix.unwrap_or_else(unix_now);
                let elapsed = unix_now().saturating_sub(opened_at_unix);
                inner.opened_at = now.checked_sub(Duration::from_secs(elapsed));
                inner.state = BreakerState::Open;
            }
        }
        inner.probe_started_at = None;
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
            if elapsed_since(inner.opened_at, now) >= inner.cooldown {
                BreakerState::HalfOpen
            } else {
                BreakerState::Open
            }
        }
        // A probe that never reported back stops counting as half-open once its window
        // elapses; `allows` then re-opens the breaker on the next call.
        BreakerState::HalfOpen => {
            if elapsed_since(inner.probe_started_at, now) >= inner.cooldown {
                BreakerState::Open
            } else {
                BreakerState::HalfOpen
            }
        }
        BreakerState::Closed => BreakerState::Closed,
    }
}

/// How long has passed since `at`, or zero when `at` is `None`.
fn elapsed_since(at: Option<Instant>, now: Instant) -> Duration {
    at.map_or(Duration::ZERO, |at| now.saturating_duration_since(at))
}

/// Force `inner` back to a healthy [`Closed`](BreakerState::Closed) state.
fn close(inner: &mut Inner, policy: BreakerPolicy) {
    inner.state = BreakerState::Closed;
    inner.failures = 0;
    inner.cooldown = policy.base_cooldown;
    inner.opened_at = None;
    inner.probe_started_at = None;
}

/// Wall-clock seconds since the Unix epoch, for persisted reports and memory timestamps.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::{unix_now, BreakerPolicy, BreakerReport, BreakerState, CircuitBreaker};
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

    #[test]
    fn a_closed_breaker_reports_closed_with_no_open_time() {
        let breaker = CircuitBreaker::new(policy());
        let report = breaker.report();
        assert_eq!(report.state, BreakerState::Closed);
        assert_eq!(report.failures, 0);
        assert_eq!(report.opened_at_unix, None);
        assert_eq!(report.cooldown_secs, 60);
    }

    #[test]
    fn a_tripped_breaker_survives_a_report_round_trip() {
        // Persisting and rebuilding a breaker in a later process must not lose the trip:
        // the source stays refused instead of being hammered again from a fresh start.
        let breaker = CircuitBreaker::new(policy());
        let now = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(now);
        }
        let report = breaker.report();
        assert_eq!(report.state, BreakerState::Open);
        assert!(report.opened_at_unix.is_some());

        let restored = CircuitBreaker::new(policy());
        restored.restore(&report);
        assert_eq!(restored.state(Instant::now()), BreakerState::Open);
        assert!(!restored.allows(Instant::now()));
    }

    #[test]
    fn an_expired_report_restores_half_open() {
        // A cooldown that elapsed while the process was down must restore as half-open,
        // so the next run gets to probe the source exactly once.
        let breaker = CircuitBreaker::new(policy());
        let report = BreakerReport {
            state: BreakerState::Open,
            failures: 3,
            // Opened an hour ago; the 60s cooldown is long over.
            opened_at_unix: Some(unix_now().saturating_sub(3600)),
            cooldown_secs: 60,
        };
        breaker.restore(&report);

        let now = Instant::now();
        assert_eq!(breaker.state(now), BreakerState::HalfOpen);
        assert!(breaker.allows(now), "the probe is granted");
        assert!(!breaker.allows(now), "only one probe");
    }

    #[test]
    fn restoring_a_closed_report_clears_a_live_trip() {
        // The manual reset path: a persisted `closed` report overwrites a live trip.
        let breaker = CircuitBreaker::new(policy());
        let now = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(now);
        }
        assert!(!breaker.allows(now));

        breaker.restore(&BreakerReport {
            state: BreakerState::Closed,
            failures: 0,
            opened_at_unix: None,
            cooldown_secs: 60,
        });
        assert_eq!(breaker.state(Instant::now()), BreakerState::Closed);
        assert!(breaker.allows(Instant::now()));
    }

    #[test]
    fn a_late_success_does_not_close_an_open_breaker() {
        // A concurrent source can deliver a success for a request issued before the trip.
        // Honouring it would close a breaker that just opened and defeat the isolation.
        let breaker = CircuitBreaker::new(policy());
        let now = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(now);
        }
        assert_eq!(breaker.state(now), BreakerState::Open);

        breaker.record_success();
        assert_eq!(breaker.state(now), BreakerState::Open, "the trip stands");
        assert!(!breaker.allows(now));
    }

    #[test]
    fn a_dropped_probe_returns_the_breaker_to_open() {
        // A half-open probe whose future is dropped never reports back. Without a
        // deadline the breaker would stay half-open forever and refuse every call.
        let breaker = CircuitBreaker::new(policy());
        let start = Instant::now();
        for _ in 0..3 {
            breaker.record_failure(start);
        }

        // Cooldown elapses; the probe is granted but its future is dropped.
        let probe_at = start + Duration::from_secs(60);
        assert!(breaker.allows(probe_at));

        // The probe window (one cooldown) expires with no outcome: back to open.
        let after_probe = probe_at + Duration::from_secs(60);
        assert_eq!(breaker.state(after_probe), BreakerState::Open);
        assert!(!breaker.allows(after_probe), "still cooling down");

        // Once the fresh cooldown elapses, the source gets another chance to heal.
        let retry_at = after_probe + Duration::from_secs(60);
        assert!(breaker.allows(retry_at), "the source can be probed again");
    }
}
