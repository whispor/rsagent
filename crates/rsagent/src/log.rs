//! A durable trace of the agent event stream, for post-mortem diagnosis.
//!
//! `--log <FILE>` appends every [`AgentEvent`] to `FILE` as one JSON object per line —
//! the same shape `--json` writes to stdout (both reuse the `Serialize` impl from
//! `rsagent-agent`, decision D10). Unlike `--json`, the log is written in *every* output
//! mode, so an interactive or unattended run still leaves a machine-readable trace.
//!
//! The file is append-only, so a trace accumulates across runs. A half-written last line
//! (from a crash mid-write) is tolerated by readers, exactly as with JSONL sessions.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use rsagent_agent::AgentEvent;

/// An append-only JSONL sink for the agent event stream.
///
/// With no file it is a no-op, so the CLI can hold one unconditionally.
#[derive(Debug)]
pub(crate) struct EventLog {
    file: Option<File>,
    /// Set after the first write failure, so the warning is printed once, not per event.
    warned: bool,
}

impl EventLog {
    /// A log that writes nowhere (no `--log` was passed).
    pub(crate) fn disabled() -> Self {
        EventLog {
            file: None,
            warned: false,
        }
    }

    /// Open `path` for appending, creating it if needed.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the file cannot be opened or created.
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(EventLog {
            file: Some(file),
            warned: false,
        })
    }

    /// Append one event as a JSON line. A write failure is reported once, then swallowed:
    /// losing a trace line must never fail an otherwise-successful run.
    pub(crate) fn append(&mut self, event: &AgentEvent) {
        let Some(file) = &mut self.file else {
            return;
        };
        let line = match serde_json::to_string(event) {
            Ok(line) => line,
            Err(error) => {
                self.warn_once(&format!("could not serialize event: {error}"));
                return;
            }
        };
        if let Err(error) = writeln!(file, "{line}") {
            self.warn_once(&format!("could not write to the log: {error}"));
        }
    }

    /// Print a warning on stderr at most once per run.
    fn warn_once(&mut self, message: &str) {
        if !self.warned {
            self.warned = true;
            eprintln!("· --log: {message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EventLog;

    #[test]
    fn a_disabled_log_writes_nothing() {
        let mut log = EventLog::disabled();
        // Must not panic and must not create anything.
        log.append(&rsagent_agent::AgentEvent::AgentStart);
    }

    #[test]
    fn events_are_appended_as_json_lines() {
        let dir = std::env::temp_dir().join(format!("rsagent-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("trace.jsonl");

        {
            let mut log = EventLog::open(&path).expect("open log");
            log.append(&rsagent_agent::AgentEvent::AgentStart);
            log.append(&rsagent_agent::AgentEvent::TurnStart);
        }

        let text = std::fs::read_to_string(&path).expect("read log");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"agentStart\""), "{}", lines[0]);
        assert!(lines[1].contains("\"turnStart\""), "{}", lines[1]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
