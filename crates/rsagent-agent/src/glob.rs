//! A tiny glob matcher for the `glob` tool, with no external dependency.
//!
//! The supported syntax is deliberately small:
//! * `*` matches any run of characters inside a single path segment;
//! * `?` matches exactly one character inside a segment;
//! * `**` matches zero or more whole path segments.
//!
//! Matching is performed on `/`-separated paths, so callers must normalize the
//! platform separator before calling.

/// Match a `/`-separated `path` against a `/`-separated `pattern`.
///
/// A pattern that contains no `/` is matched against the path's final segment, so
/// `*.rs` finds `src/main.rs` as well as `main.rs`. A leading `./` on either side is
/// ignored.
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let pattern = pattern.trim_end_matches('/');
    let path = path.trim_start_matches("./");

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if pattern.contains('/') {
        let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        match_segments(&pat, &segments)
    } else {
        // No slash: match the file name only, wherever the file lives.
        segments
            .last()
            .is_some_and(|name| match_segment(pattern, name))
    }
}

/// Match a sequence of pattern segments against a sequence of path segments.
///
/// `**` may absorb any number of segments; every other segment is matched literally
/// (with `*`/`?` inside it).
fn match_segments(pat: &[&str], seg: &[&str]) -> bool {
    match pat.split_first() {
        None => seg.is_empty(),
        Some((&"**", rest)) => (0..=seg.len()).any(|skip| match_segments(rest, &seg[skip..])),
        Some((head, rest)) => match seg.split_first() {
            Some((first, tail)) => match_segment(head, first) && match_segments(rest, tail),
            None => false,
        },
    }
}

/// Wildcard match within a single path segment: `*` and `?`.
fn match_segment(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();

    // Classic linear wildcard match with backtracking on the last `*`.
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn star_stays_within_a_segment() {
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/deep/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/deep/main.rs"));
    }

    #[test]
    fn double_star_spans_segments() {
        assert!(glob_match("**/*.rs", "main.rs"));
        assert!(glob_match("**/*.rs", "src/deep/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b/c.rs"));
        assert!(glob_match("src/**", "src/a/b/c.rs"));
        assert!(!glob_match("src/**/*.rs", "lib/a.rs"));
    }

    #[test]
    fn question_matches_one_character() {
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("a?c", "abbc"));
    }

    #[test]
    fn a_pattern_without_a_slash_matches_the_file_name() {
        assert!(glob_match("*.rs", "deep/nested/main.rs"));
        assert!(glob_match("main.rs", "deep/main.rs"));
    }

    #[test]
    fn leading_dot_slash_is_ignored() {
        assert!(glob_match("./**/*.rs", "./src/main.rs"));
    }
}
