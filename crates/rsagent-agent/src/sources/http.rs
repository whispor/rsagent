//! Reading an HTTP response body with a size cap.
//!
//! Three callers turn a response body into a string — the `web` and `mcp-http` sources and
//! the built-in `webfetch` tool. A body is small in every real case, but nothing stops a
//! server from streaming gigabytes, so all three read through [`read_body`], which stops at
//! a caller-chosen cap.

use reqwest::Response;

/// Cap on how many bytes of a source's response body are buffered.
///
/// A tool's output reaches the model truncated to its context budget anyway, so a
/// multi-megabyte body is already wasted work; the cap keeps one runaway response from
/// setting the process's memory peak. `webfetch` uses its own (smaller) configurable cap.
pub(crate) const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Read a response body, stopping once it would exceed `max_bytes`.
///
/// Returns the decoded body and whether it was cut short. Decoding is lossy, in case a
/// multi-byte character straddles the cut.
///
/// # Errors
///
/// Returns a message when a chunk of the body cannot be read.
pub(crate) async fn read_body(
    mut response: Response,
    max_bytes: usize,
) -> Result<(String, bool), String> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("failed to read response body: {error}"))?
    {
        let room = max_bytes.saturating_sub(bytes.len());
        if chunk.len() > room {
            bytes.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}
