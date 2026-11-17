//! A deliberately crude HTML-to-text converter for the `webfetch` tool.
//!
//! It is not a spec-compliant parser and does not try to be. It drops
//! `<script>`/`<style>`/`<noscript>` content and comments, removes tags, decodes a few
//! common entities, and tidies whitespace. That is enough to read a documentation page;
//! it is not enough to preserve layout or `<pre>` indentation.

/// Convert an HTML document to readable plain text.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut text = html.to_string();
    for (open, close) in [
        ("<script", "</script"),
        ("<style", "</style"),
        ("<noscript", "</noscript"),
        ("<!--", "-->"),
    ] {
        text = remove_between(&text, open, close);
    }
    let stripped = strip_tags(&text);
    let decoded = decode_entities(&stripped);
    tidy_whitespace(&decoded)
}

/// Remove everything from each `open` marker through the end of the matching `close` tag.
fn remove_between(html: &str, open: &str, close_prefix: &str) -> String {
    // ASCII-only lowercasing preserves byte offsets, so indices stay valid on `html`.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;

    while let Some(rel) = lower[cursor..].find(open) {
        let start = cursor + rel;
        out.push_str(&html[cursor..start]);

        let after_open = start + open.len();
        match lower[after_open..].find(close_prefix) {
            Some(rel_close) => {
                let close_at = after_open + rel_close;
                // Skip past the closing tag's `>`.
                cursor = match lower[close_at..].find('>') {
                    Some(gt) => close_at + gt + 1,
                    None => html.len(),
                };
            }
            None => cursor = html.len(),
        }
    }
    out.push_str(&html[cursor..]);
    out
}

/// Drop every `<...>` tag, replacing it with a space.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Decode common named entities and numeric character references.
///
/// `&amp;` is decoded last so `&amp;lt;` yields the literal `&lt;`, not `<`.
fn decode_entities(text: &str) -> String {
    let mut out = text.to_string();
    for (name, value) in [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&nbsp;", " "),
    ] {
        out = out.replace(name, value);
    }
    out = decode_numeric_entities(&out);
    out.replace("&amp;", "&")
}

/// Decode `&#123;` and `&#x1F600;` references, leaving anything invalid untouched.
fn decode_numeric_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] == '&' && chars.get(index + 1) == Some(&'#') {
            if let Some((decoded, next)) = decode_numeric_at(&chars, index) {
                out.push(decoded);
                index = next;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// Try to decode a numeric reference starting at `start` (`&`). Returns the decoded
/// character and the index just past the terminating `;`.
fn decode_numeric_at(chars: &[char], start: usize) -> Option<(char, usize)> {
    let mut index = start + 2;
    let radix = if matches!(chars.get(index), Some('x' | 'X')) {
        index += 1;
        16
    } else {
        10
    };
    let digits_start = index;
    while matches!(chars.get(index), Some(c) if c.is_digit(radix)) {
        index += 1;
    }
    if index == digits_start || chars.get(index) != Some(&';') {
        return None;
    }
    let digits: String = chars[digits_start..index].iter().collect();
    let code = u32::from_str_radix(&digits, radix).ok()?;
    let decoded = char::from_u32(code)?;
    Some((decoded, index + 1))
}

/// Trim each line, collapse internal whitespace runs, and drop repeated blank lines.
fn tidy_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;

    for line in text.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&trimmed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    #[test]
    fn drops_script_style_and_comments() {
        let html = "<p>keep</p><script>var x = 1;</script><style>p{}</style><!-- note -->";
        let text = html_to_text(html);
        assert!(text.contains("keep"), "got: {text}");
        assert!(!text.contains("var x"), "script must be dropped: {text}");
        assert!(!text.contains("p{}"), "style must be dropped: {text}");
        assert!(!text.contains("note"), "comments must be dropped: {text}");
    }

    #[test]
    fn strips_tags_and_decodes_entities() {
        let html = "<h1>A &amp; B</h1><p>a &lt; b &#39;q&#39; &#x41;</p>";
        let text = html_to_text(html);
        assert!(text.contains("A & B"), "got: {text}");
        assert!(text.contains("a < b 'q' A"), "got: {text}");
    }

    #[test]
    fn does_not_double_decode_ampersand_entities() {
        assert_eq!(html_to_text("&amp;lt;"), "&lt;");
    }

    #[test]
    fn collapses_blank_lines() {
        let html = "<p>one</p>\n\n\n\n<p>two</p>";
        assert_eq!(html_to_text(html), "one\n\ntwo");
    }
}
