//! HTTP helpers shared by the streaming providers.
//!
//! Both the OpenAI-compatible and Anthropic providers send a JSON body and read an
//! SSE response, so the retry/backoff policy lives here once instead of twice.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// The longest a single retry backoff may wait, whatever the server hints.
///
/// A `Retry-After` far in the future (or a buggy one) must not stall a turn indefinitely.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Mixed into the jitter seed so two retries landing in the same nanosecond still differ.
static JITTER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether an HTTP status is worth retrying (rate limiting, timeouts, server errors).
///
/// `409 Conflict` is deliberately *not* here: it is a deterministic answer to the request,
/// so retrying it only wastes an attempt.
pub(super) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500..=599)
}

/// Send a JSON request, retrying transient failures with exponential backoff.
///
/// `headers` must already carry authentication (for example `authorization: Bearer …`
/// or `x-api-key: …`); `content-type` is set here. A response that arrives with a
/// non-retryable error status is returned as an `Err` carrying the status and body;
/// the caller turns it into a terminal stream event.
///
/// `client` is the provider's shared HTTP client, so the connection pool is reused
/// across requests rather than rebuilt on every call.
pub(super) async fn send_with_retry(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    max_retries: u32,
) -> Result<reqwest::Response, String> {
    let max_attempts = max_retries.saturating_add(1);

    for attempt in 0..max_attempts {
        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .json(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }

        let last_attempt = attempt + 1 >= max_attempts;
        match request.send().await {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) if is_retryable_status(resp.status()) && !last_attempt => {
                let hint = retry_after(resp.headers());
                let _ = resp.text().await;
                backoff(attempt, hint).await;
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("HTTP {status}: {}", text.trim()));
            }
            Err(_) if !last_attempt => backoff(attempt, None).await,
            Err(error) => return Err(format!("request failed: {error}")),
        }
    }

    Err("request failed after retries".to_string())
}

/// Wait before retrying `attempt` (0-based), honouring a server hint when there is one.
async fn backoff(attempt: u32, hint: Option<Duration>) {
    tokio::time::sleep(retry_delay(attempt, hint, jitter_seed())).await;
}

/// The delay before retry `attempt` (0-based).
///
/// Exponential backoff, jittered to spread clients out, floored by the server's `Retry-After`
/// hint when present, and capped at [`MAX_BACKOFF`]. Pure, so the policy is unit-testable
/// without sleeping.
fn retry_delay(attempt: u32, hint: Option<Duration>, seed: u64) -> Duration {
    let base = Duration::from_millis(500u64.saturating_mul(1u64 << attempt.min(4)).min(8_000));
    let jittered = jitter(base, seed);
    hint.map_or(jittered, |hint| hint.max(jittered))
        .min(MAX_BACKOFF)
}

/// Spread a backoff across `[base/2, base]`, so many clients do not retry in lockstep.
fn jitter(base: Duration, seed: u64) -> Duration {
    let base_millis = u64::try_from(base.as_millis()).unwrap_or(0);
    if base_millis == 0 {
        return base;
    }
    let half = base_millis / 2;
    let extra = seed % (base_millis - half + 1);
    Duration::from_millis(half + extra)
}

/// A cheap pseudo-random seed for jitter: the sub-second clock, mixed with a counter so
/// concurrent retries still draw different delays.
fn jitter_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::from(elapsed.subsec_nanos()));
    nanos ^ JITTER_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// The delay a `Retry-After` response header asks for, if it carries one we understand.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    parse_retry_after(value, SystemTime::now())
}

/// Parse a `Retry-After` value relative to `now`.
///
/// Accepts both forms RFC 9110 allows: delta-seconds (`120`) and an IMF-fixdate
/// (`Wed, 21 Oct 2015 07:28:00 GMT`). Anything else — including a date already in the past —
/// yields `None`, and the caller falls back to plain exponential backoff.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    parse_http_date(value)?.duration_since(now).ok()
}

/// Parse an IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) into a [`SystemTime`].
///
/// Only the IMF-fixdate form is accepted — the obsolete RFC 850 and asctime forms RFC 9110
/// also lists are not. Real servers send IMF-fixdate, and this keeps the parser dependency-free.
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let rest = value.strip_suffix(" GMT")?;
    let (_, rest) = rest.split_once(", ")?;
    let mut fields = rest.split(' ');
    let day: u32 = fields.next()?.parse().ok()?;
    let month = month_from_name(fields.next()?)?;
    let year: i64 = fields.next()?.parse().ok()?;
    let clock = fields.next()?;
    if fields.next().is_some() || !(1970..=9999).contains(&year) {
        return None;
    }
    let mut time = clock.split(':');
    let hour: u32 = time.next()?.parse().ok()?;
    let minute: u32 = time.next()?.parse().ok()?;
    let second: u32 = time.next()?.parse().ok()?;
    if time.next().is_some() {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3_600
        + i64::from(minute) * 60
        + i64::from(second);
    let seconds = u64::try_from(seconds).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

/// The month number for a three-letter English month name.
fn month_from_name(name: &str) -> Option<u32> {
    match name {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

/// Days from `1970-01-01` to `year-month-day` (proleptic Gregorian).
///
/// Howard Hinnant's `days_from_civil`, so the HTTP-date form needs no date dependency.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400; // [0, 399]
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_status_covers_transient_failures_only() {
        use reqwest::StatusCode;

        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(is_retryable_status(status), "{status} should be retried");
        }

        for status in [
            StatusCode::OK,
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
            StatusCode::CONFLICT,
        ] {
            assert!(
                !is_retryable_status(status),
                "{status} should not be retried"
            );
        }
    }

    #[test]
    fn retry_after_accepts_seconds_and_a_date() {
        let epoch = SystemTime::UNIX_EPOCH;
        // Delta-seconds form, tolerant of surrounding whitespace.
        assert_eq!(
            parse_retry_after("120", epoch),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after(" 30 ", epoch),
            Some(Duration::from_secs(30))
        );

        // IMF-fixdate form, relative to `now`.
        let now = epoch + Duration::from_secs(100);
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:01:40 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:03:20 GMT", now),
            Some(Duration::from_secs(100))
        );
        // A date already in the past is no hint.
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:00:10 GMT", now),
            None
        );
        // Nonsense is no hint.
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn jitter_stays_within_the_backoff_range() {
        let base = Duration::from_millis(500);
        for seed in 0..1_000u64 {
            let delay = jitter(base, seed);
            assert!(delay >= base / 2, "{delay:?} is below half of {base:?}");
            assert!(delay <= base, "{delay:?} is above {base:?}");
        }
        // A zero base is returned unchanged rather than dividing by zero.
        assert_eq!(jitter(Duration::ZERO, 7), Duration::ZERO);
    }

    #[test]
    fn retry_delay_honours_a_hint_and_caps_it() {
        // No hint: exponential backoff, jittered within [half, full].
        let bare = retry_delay(0, None, 3);
        assert!(
            bare >= Duration::from_millis(250) && bare <= Duration::from_millis(500),
            "{bare:?} is outside the jitter range"
        );

        // A hint larger than the jittered backoff becomes the floor.
        assert_eq!(
            retry_delay(0, Some(Duration::from_secs(30)), 0),
            Duration::from_secs(30)
        );

        // A hint beyond the cap is clamped, so a turn cannot stall for long.
        assert_eq!(
            retry_delay(0, Some(Duration::from_secs(3_600)), 0),
            MAX_BACKOFF
        );
    }
}
