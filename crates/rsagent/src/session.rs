//! Session persistence: the transcript, wrapped in a versioned envelope.
//!
//! The envelope lets a future format change be *detected* instead of silently misread.
//! Writes are atomic (`docs/EXTENSIBILITY.md` §4.2 trap 3): the file is written to a
//! sibling temp file and renamed into place, so a crash or a concurrent run never leaves
//! half a JSON document behind.

use std::path::Path;

use rsagent_agent::Agent;
use rsagent_ai::Context;
use serde::{Deserialize, Serialize};

use crate::render::Palette;

/// Session files are wrapped in a versioned envelope, so a future format change is
/// detected instead of silently misread.
const SESSION_VERSION: u32 = 1;

/// The session file resumed by `--continue`.
pub(crate) const DEFAULT_SESSION_FILE: &str = ".rsagent-session.json";

/// A session file: a versioned envelope around the persisted [`Context`].
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    /// Format version, so a newer file can be recognized (and rejected) by an older build.
    version: u32,
    /// The persisted transcript and system prompt.
    context: Context,
}

/// Read a persisted transcript, if the file exists.
///
/// Both the current versioned envelope and the older bare-`Context` format are
/// accepted; a file written by a newer format version is rejected rather than misread.
///
/// # Errors
///
/// Returns an error when the file exists but cannot be read or parsed, so the caller can
/// warn and start fresh instead of pretending the session was never there.
pub(crate) fn load(path: &Path) -> anyhow::Result<Option<Context>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    if value.get("version").is_some() {
        let file: SessionFile = serde_json::from_value(value)?;
        if file.version > SESSION_VERSION {
            anyhow::bail!(
                "written by a newer rsagent (format v{}); this build understands up to v{}",
                file.version,
                SESSION_VERSION
            );
        }
        Ok(Some(file.context))
    } else {
        // Legacy format: a bare `Context` with no envelope.
        Ok(Some(serde_json::from_value(value)?))
    }
}

/// Write the transcript to `path`, if a session file was requested.
///
/// A write failure is reported but not fatal: losing the persisted session must never
/// abort an otherwise successful run.
pub(crate) fn save(path: Option<&Path>, agent: &Agent, palette: Palette) {
    let Some(path) = path else {
        return;
    };
    let file = SessionFile {
        version: SESSION_VERSION,
        context: agent.transcript(),
    };
    match serde_json::to_string_pretty(&file) {
        Ok(json) => {
            if let Err(error) = write_atomic(path, &json) {
                eprintln!(
                    "{}",
                    palette.yellow(&format!(
                        "· could not save session to {}: {error}",
                        path.display()
                    ))
                );
            }
        }
        Err(error) => eprintln!(
            "{}",
            palette.yellow(&format!("· could not serialize session: {error}"))
        ),
    }
}

/// Write `contents` to `path` atomically: a temp file beside it, then a rename.
///
/// `rename` replaces the destination in a single step on every supported platform, so a
/// reader never observes a partially written file. The temp file lives in the same
/// directory as `path`, because a rename across filesystems is not atomic (and on some
/// platforms not permitted at all).
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, contents)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        // A failed rename must not leave the temp file behind.
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}
