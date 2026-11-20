//! Integrity checks for the documentation that is part of the contract.
//!
//! `cargo doc` only verifies Rust intra-doc links; it says nothing about links *between*
//! markdown files, and nothing about whether the prose in `STABILITY.md` still describes
//! what the code actually exposes. Both have silently drifted before, so both are pinned
//! by a test rather than by a reviewer's memory.

use std::collections::BTreeSet;
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

/// The names a crate root re-exports as contract: every `pub use` except those marked
/// `#[doc(hidden)]`, which stay reachable but are not promised.
fn contract_exports(lib_rs: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = lib_rs;
    while let Some(at) = rest.find("pub use ") {
        let hidden = rest[..at]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .rev()
            .take_while(|line| line.starts_with("#["))
            .any(|line| line == "#[doc(hidden)]");
        let statement = rest[at + "pub use ".len()..]
            .split(';')
            .next()
            .unwrap_or_default();
        if !hidden {
            // Either `mod::{A, B}` or `mod::Name`.
            let tail = statement.split_once('{').map_or_else(
                || statement.rsplit("::").next().unwrap_or_default(),
                |(_, inner)| inner,
            );
            for token in tail.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if !token.is_empty() {
                    names.insert(token.to_owned());
                }
            }
        }
        rest = &rest[at + "pub use ".len() + statement.len()..];
    }
    names
}

/// The names listed in `STABILITY.md`'s public-API table for one crate: every backticked
/// token in a row whose `Where` column is `crate root`.
fn table_exports(stability_md: &str, crate_name: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in stability_md.lines() {
        if !line.contains("| crate root |") || !line.contains(crate_name) {
            continue;
        }
        let surface = line.split('|').nth(1).unwrap_or_default();
        for (index, chunk) in surface.split('`').enumerate() {
            let name = chunk.trim();
            if index % 2 == 1 && !name.is_empty() && name != crate_name {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

/// Fails if `STABILITY.md`'s public-API table and the crate roots disagree. The table is
/// the contract; if it drifts from the `pub use` statements, a reader gets a wrong answer
/// about what they may rely on.
#[test]
fn the_public_api_table_matches_the_crate_roots() {
    let root = repo_root();
    let stability = fs::read_to_string(root.join("STABILITY.md")).expect("read STABILITY.md");
    for (crate_name, lib_path) in [
        ("rsagent-ai", "crates/rsagent-ai/src/lib.rs"),
        ("rsagent-agent", "crates/rsagent-agent/src/lib.rs"),
    ] {
        let lib = fs::read_to_string(root.join(lib_path)).expect("read a crate root");
        let exports = contract_exports(&lib);
        let table = table_exports(&stability, crate_name);
        let unlisted: Vec<_> = exports.difference(&table).collect();
        let phantom: Vec<_> = table.difference(&exports).collect();
        assert!(
            unlisted.is_empty() && phantom.is_empty(),
            "{crate_name}: STABILITY.md and the crate root disagree\n  \
             exported but not in the table: {unlisted:?}\n  \
             in the table but not exported: {phantom:?}"
        );
    }
}
