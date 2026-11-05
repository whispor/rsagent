//! HTTP helpers shared by the streaming providers.
//!
//! Both the OpenAI-compatible and Anthropic providers send a JSON body and read an
//! SSE response, so the retry/backoff policy lives here once instead of twice.

use std::time::Duration;

use serde_json::Value;

/// Whether an HTTP status is worth retrying (rate limiting, timeouts, server errors).
pub(super) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500..=599)
}

/// Send a JSON request, retrying transient failures with exponential backoff.
///
/// `headers` must already carry authentication (for example `authorization: Bearer …`
/// or `x-api-key: …`); `content-type` is set here. A response that arrives with a
/// non-retryable error status is returned as an `Err` carrying the status and body;
/// the caller turns it into a terminal stream event.
pub(super) async fn send_with_retry(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    max_retries: u32,
) -> Result<reqwest::Response, String> {
    let client = reqwest::Client::new();
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
                let _ = resp.text().await;
                backoff(attempt).await;
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("HTTP {status}: {}", text.trim()));
            }
            Err(_) if !last_attempt => backoff(attempt).await,
            Err(error) => return Err(format!("request failed: {error}")),
        }
    }

    Err("request failed after retries".to_string())
}

/// Exponential backoff between retry attempts, capped at a few seconds.
async fn backoff(attempt: u32) {
    let millis = 500u64.saturating_mul(1u64 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(millis.min(8_000))).await;
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
        ] {
            assert!(
                !is_retryable_status(status),
                "{status} should not be retried"
            );
        }
    }
}
