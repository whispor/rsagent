//! Integrity checks for the documentation that is part of the contract.
//!
//! `cargo doc` only verifies Rust intra-doc links; it says nothing about links *between*
//! markdown files, and nothing about whether the prose in `STABILITY.md` still describes
//! what the code actually exposes. Both have silently drifted before, so both are pinned
//! by a test rather than by a reviewer's memory.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, two levels above this crate (`crates/rsagent`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every markdown file under `root`, skipping build output and VCS metadata.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name();
                if name != OsStr::new("target") && name != OsStr::new(".git") {
                    walk(&path, found);
                }
            } else if path.extension() == Some(OsStr::new("md")) {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found
}

/// Relative link targets in a markdown document. Fenced code blocks are skipped (they may
/// contain link-like text), as are external URLs, `mailto:` links, and bare `#anchors`.
fn relative_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find("](") {
            let after = &rest[open + 2..];
            let Some(close) = after.find(')') else {
                break;
            };
            // `](target)`, `](target "title")` and `](<target>)` all resolve to `target`.
            let target = after[..close].split_whitespace().next().unwrap_or_default();
            let target = target.trim_matches(|c| c == '<' || c == '>');
            rest = &after[close + 1..];
            if target.is_empty()
                || target.starts_with('#')
                || target.starts_with("mailto:")
                || target.contains("://")
            {
                continue;
            }
            links.push(target.to_owned());
        }
    }
    links
}

/// Fails if any relative markdown link points at a file that does not exist. The 4 broken
/// links in `STABILITY.md`'s contract table were exactly this kind of rot.
#[test]
fn every_relative_markdown_link_resolves() {
    let root = repo_root();
    let mut broken = Vec::new();
    for file in markdown_files(&root) {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let dir = file.parent().unwrap_or(&root);
        for link in relative_links(&text) {
            let path = link.split('#').next().unwrap_or_default();
            if path.is_empty() || dir.join(path).exists() {
                continue;
            }
            broken.push(format!(
                "{}: {link}",
                file.strip_prefix(&root).unwrap_or(&file).display()
            ));
        }
    }
    assert!(
        broken.is_empty(),
        "broken relative markdown links:\n  {}",
        broken.join("\n  ")
    );
}
