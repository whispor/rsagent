//! Minimal Server-Sent Events (SSE) handling shared by the HTTP providers.
//!
//! Both the `OpenAI` and Anthropic streams are SSE: a byte stream split into
//! newline-delimited lines where only the `data:` field carries the JSON we care
//! about. The framing (splitting bytes into lines) and the classification
//! (`data:` payload vs. everything else) are identical across providers, so they
//! live here instead of being copied into each one.

/// A single SSE line, already classified.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SseLine<'a> {
    /// A `data:` payload with surrounding whitespace trimmed. What the payload
    /// means — JSON, a `[DONE]` sentinel, ... — is the caller's decision.
    Data(&'a str),
    /// A blank line, a comment, or a non-`data:` field (`event:`, `id:`, ...).
    Skip,
}

/// Pull the next complete line out of `buffer`, if one has arrived.
///
/// SSE lines end in `\n`; a trailing `\r` from CRLF is stripped. The consumed
/// bytes are removed from `buffer`, so a caller can loop until this returns
/// [`None`], which means the buffer holds only a partial line.
///
/// `buffer` holds raw bytes, not a `String`, and only the bytes up to (and
/// including) the newline are decoded. A multi-byte UTF-8 character split across
/// two network chunks therefore stays in `buffer` until its final byte arrives —
/// decoding a lone continuation byte would corrupt it into U+FFFD.
pub(super) fn next_line(buffer: &mut Vec<u8>) -> Option<String> {
    let newline = buffer.iter().position(|&byte| byte == b'\n')?;
    let line = String::from_utf8_lossy(&buffer[..newline])
        .trim_end_matches('\r')
        .to_string();
    buffer.drain(..=newline);
    Some(line)
}

/// Classify one line as either a trimmed `data:` payload or something to skip.
pub(super) fn classify(line: &str) -> SseLine<'_> {
    let Some(data) = line.strip_prefix("data:") else {
        return SseLine::Skip;
    };
    let data = data.trim();
    if data.is_empty() {
        SseLine::Skip
    } else {
        SseLine::Data(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulls_lines_out_of_the_buffer_and_strips_crlf() {
        let mut buffer = b"data: a\r\ndata: b\npartial".to_vec();

        assert_eq!(next_line(&mut buffer).as_deref(), Some("data: a"));
        assert_eq!(next_line(&mut buffer).as_deref(), Some("data: b"));
        // No newline yet, so the partial line stays buffered.
        assert_eq!(next_line(&mut buffer), None);
        assert_eq!(buffer, b"partial");
    }

    #[test]
    fn keeps_a_multibyte_char_split_across_chunks_intact() {
        // "中" is the three bytes E4 B8 AD; the first byte arrives in its own chunk,
        // so a naive per-chunk `from_utf8_lossy` would have replaced it with U+FFFD.
        let mut buffer: Vec<u8> = b"data: ".to_vec();
        buffer.push(0xE4);
        assert_eq!(next_line(&mut buffer), None);
        buffer.extend_from_slice(&[0xB8, 0xAD, b'\n']);
        assert_eq!(next_line(&mut buffer).as_deref(), Some("data: 中"));
    }

    #[test]
    fn classifies_data_lines_and_skips_everything_else() {
        assert_eq!(classify("data: {\"x\":1}"), SseLine::Data("{\"x\":1}"));
        assert_eq!(classify("data:[DONE]"), SseLine::Data("[DONE]"));
        assert_eq!(classify("data:   "), SseLine::Skip);
        assert_eq!(classify("event: message_start"), SseLine::Skip);
        assert_eq!(classify(""), SseLine::Skip);
    }
}
