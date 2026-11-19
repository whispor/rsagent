//! Session persistence: the transcript, saved to a file and resumed later.
//!
//! Two encodings, chosen by file extension:
//!
//! * **JSON envelope** (default) — one versioned object; rewritten in full on every save.
//! * **JSONL** (`.jsonl`) — a header line followed by one message per line, **appended**
//!   to rather than rewritten, which suits long sessions (`docs/EXTENSIBILITY.md` §5).
//!
//! The JSON envelope is written atomically (`docs/EXTENSIBILITY.md` §4.2 trap 3): a temp
//! file beside the target, then a rename, so a crash never leaves half a document behind.
//! JSONL append writes can additionally leave a truncated *last* line if a crash
//! interrupts them; [`Session::load`] tolerates that by dropping the incomplete tail.
//!
//! # Concurrency
//!
//! Session files are not locked. Two rsagent runs sharing one file will race: the JSON
//! envelope always leaves one whole document (the atomic rename), but the last writer
//! wins; a JSONL append can interleave two runs' lines. Do not point two concurrent runs
//! at the same session file.
//!
//! # Version compatibility
//!
//! A file written by a **newer** rsagent is refused, and the session is marked read-only
//! so this older build never overwrites it with its own (smaller) transcript.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use rsagent_agent::Agent;
use rsagent_ai::{Context, Message};
use serde::{Deserialize, Serialize};

use crate::fs::write_atomic;
use crate::render::Palette;

/// Session files are wrapped in a versioned envelope, so a future format change is
/// detected instead of silently misread.
const SESSION_VERSION: u32 = 1;

/// The session file resumed by `--continue`.
pub(crate) const DEFAULT_SESSION_FILE: &str = ".rsagent-session.json";

/// How a session file is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// A single versioned JSON envelope; rewritten in full on every save.
    Json,
    /// A header line followed by one message per line; appended to, not rewritten.
    Jsonl,
}

/// The session file was written by a newer rsagent, so it must not be rewritten.
#[derive(Debug)]
struct IncompatibleVersion {
    found: u32,
}

impl std::fmt::Display for IncompatibleVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "written by a newer rsagent (format v{}); this build understands up to v{}",
            self.found, SESSION_VERSION
        )
    }
}

impl std::error::Error for IncompatibleVersion {}

/// A session file: where it lives, how it is encoded, and how much is already written.
pub(crate) struct Session {
    path: PathBuf,
    format: Format,
    /// Messages already persisted (JSONL append mode); always `0` for the JSON envelope.
    written: usize,
    /// Fingerprint of the last message written (JSONL append mode), used to confirm the
    /// on-disk file is still a clean prefix before appending to it.
    last_written: Option<u64>,
    /// Set when the on-disk file cannot be appended to — a truncated tail was dropped, or
    /// an empty file had no header — forcing a full rewrite.
    rewrite: bool,
    /// Set when loading dropped a truncated final line, so the caller can say so.
    dropped_tail: bool,
    /// Set when the file is incompatible (newer version): never save over it.
    read_only: bool,
}

/// A session file in the JSON envelope: a versioned wrapper around the [`Context`].
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    /// Format version, so a newer file can be recognized (and rejected) by an older build.
    version: u32,
    /// The persisted transcript and system prompt.
    context: Context,
}

/// The first line of a JSONL session: the format version and the system prompt.
#[derive(Debug, Serialize, Deserialize)]
struct JsonlHeader {
    /// Always `header`; distinguishes this line from the message lines that follow.
    #[serde(rename = "type")]
    kind: String,
    /// Format version, so a newer file can be recognized (and rejected) by an older build.
    version: u32,
    /// The system prompt in effect when the session was written.
    #[serde(
        rename = "systemPrompt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    system_prompt: Option<String>,
}

/// The result of reading a JSONL session.
struct JsonlLoad {
    /// The transcript, or `None` when the file held no header (an empty file).
    context: Option<Context>,
    /// How many messages were parsed.
    written: usize,
    /// The on-disk file is not a clean prefix of the transcript; the next save must rewrite.
    needs_rewrite: bool,
    /// A truncated final line was dropped (a crash interrupted an append).
    dropped_tail: bool,
}

impl Session {
    /// Open a session for `path`, without reading it yet.
    ///
    /// The encoding is chosen from the extension: `.jsonl` (case-insensitive) selects the
    /// append-friendly JSONL format, anything else the JSON envelope.
    pub(crate) fn open(path: &Path) -> Self {
        let format = match path.extension() {
            Some(extension) if extension.eq_ignore_ascii_case("jsonl") => Format::Jsonl,
            _ => Format::Json,
        };
        Session {
            path: path.to_path_buf(),
            format,
            written: 0,
            last_written: None,
            rewrite: false,
            dropped_tail: false,
            read_only: false,
        }
    }

    /// The file this session is stored in.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether loading dropped a truncated final line — a crash left a half-written record.
    ///
    /// Only meaningful after [`load`](Self::load). Surfaced so the caller can tell the user
    /// that a partial record was discarded rather than silently losing it.
    pub(crate) fn dropped_truncated_tail(&self) -> bool {
        self.dropped_tail
    }

    /// Read the persisted transcript, if the file exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed, so the caller
    /// can warn and start fresh instead of pretending the session was never there. A file
    /// from a newer rsagent additionally marks the session read-only, so it is not
    /// overwritten by this build.
    pub(crate) fn load(&mut self) -> anyhow::Result<Option<Context>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let result = match self.format {
            Format::Json => load_json(&self.path),
            Format::Jsonl => load_jsonl(&self.path).map(|loaded| {
                self.written = loaded.written;
                self.rewrite = loaded.needs_rewrite;
                self.dropped_tail = loaded.dropped_tail;
                self.last_written = loaded
                    .context
                    .as_ref()
                    .and_then(|context| context.messages.last())
                    .map(fingerprint);
                loaded.context
            }),
        };
        if let Err(error) = &result {
            if error.downcast_ref::<IncompatibleVersion>().is_some() {
                self.read_only = true;
            }
        }
        result
    }

    /// Write the agent's transcript.
    ///
    /// A write failure is reported but not fatal: losing the persisted session must never
    /// abort an otherwise successful run. A file from a newer rsagent is never touched.
    pub(crate) fn save(&mut self, agent: &Agent, palette: Palette) {
        if self.read_only {
            eprintln!(
                "{}",
                palette.yellow(&format!(
                    "· not saving to {}: it was written by a newer rsagent and is left untouched",
                    self.path.display()
                ))
            );
            return;
        }
        let context = agent.transcript();
        let result = match self.format {
            Format::Json => self.save_json(&context),
            Format::Jsonl => self.save_jsonl(&context),
        };
        if let Err(error) = result {
            eprintln!(
                "{}",
                palette.yellow(&format!(
                    "· could not save session to {}: {error}",
                    self.path.display()
                ))
            );
        }
    }

    /// Rewrite the whole file as one versioned JSON envelope.
    fn save_json(&mut self, context: &Context) -> anyhow::Result<()> {
        let file = SessionFile {
            version: SESSION_VERSION,
            context: context.clone(),
        };
        let json = serde_json::to_string_pretty(&file)?;
        write_atomic(&self.path, &json)?;
        Ok(())
    }

    /// Append only the new tail when the on-disk file is still a clean prefix; else rewrite.
    ///
    /// Rewriting is needed when the transcript shrank (a `/clear`), when context compaction
    /// dropped middle turns (the transcript may still have grown, so a message count is not
    /// enough — the last written message is fingerprinted and re-checked), when the file is
    /// missing or headerless, or when a truncated tail was dropped on load.
    fn save_jsonl(&mut self, context: &Context) -> anyhow::Result<()> {
        let messages = &context.messages;
        if self.can_append(messages) {
            let mut appended = String::new();
            for message in &messages[self.written..] {
                appended.push_str(&serde_json::to_string(message)?);
                appended.push('\n');
            }
            if !appended.is_empty() {
                let mut file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
                file.write_all(appended.as_bytes())?;
            }
            self.record_written(messages);
            return Ok(());
        }

        let header = JsonlHeader {
            kind: "header".to_string(),
            version: SESSION_VERSION,
            system_prompt: context.system_prompt.clone(),
        };
        let mut text = serde_json::to_string(&header)?;
        text.push('\n');
        for message in messages {
            text.push_str(&serde_json::to_string(message)?);
            text.push('\n');
        }
        write_atomic(&self.path, &text)?;
        self.record_written(messages);
        self.rewrite = false;
        Ok(())
    }

    /// Whether the new tail can be appended without rewriting the file.
    fn can_append(&self, messages: &[Message]) -> bool {
        if !self.path.exists() || self.rewrite || self.written > messages.len() {
            return false;
        }
        if self.written == 0 {
            // An existing file with only a header (or no messages yet): appending is fine.
            return true;
        }
        messages.get(self.written - 1).map(fingerprint) == self.last_written
    }

    /// Remember how much was written and what the last message was.
    fn record_written(&mut self, messages: &[Message]) {
        self.written = messages.len();
        self.last_written = messages.last().map(fingerprint);
    }
}

/// A cheap fingerprint of one message, used to confirm the on-disk prefix still matches.
fn fingerprint(message: &Message) -> u64 {
    let mut hasher = DefaultHasher::new();
    serde_json::to_string(message)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// Read a JSON-envelope session.
fn load_json(path: &Path) -> anyhow::Result<Option<Context>> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    if value.get("version").is_some() {
        let file: SessionFile = serde_json::from_value(value)?;
        if file.version > SESSION_VERSION {
            return Err(anyhow::Error::new(IncompatibleVersion {
                found: file.version,
            }));
        }
        Ok(Some(file.context))
    } else {
        // Legacy format: a bare `Context` with no envelope.
        Ok(Some(serde_json::from_value(value)?))
    }
}

/// Read a JSONL session.
///
/// A malformed **last** line is dropped rather than reported: it is the signature of an
/// append that a crash interrupted, and the messages before it are still valid. A file
/// with no header at all (empty, or blank lines only) is reported as needing a rewrite, so
/// the next save writes a proper header instead of stranding headerless message lines.
fn load_jsonl(path: &Path) -> anyhow::Result<JsonlLoad> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return Ok(JsonlLoad {
            context: None,
            written: 0,
            needs_rewrite: true,
            dropped_tail: false,
        });
    };
    let header: JsonlHeader =
        serde_json::from_str(header_line).map_err(|error| anyhow::anyhow!("header: {error}"))?;
    if header.version > SESSION_VERSION {
        return Err(anyhow::Error::new(IncompatibleVersion {
            found: header.version,
        }));
    }

    let rest: Vec<&str> = lines.collect();
    let mut messages = Vec::new();
    let mut dropped_tail = false;
    for (index, line) in rest.iter().enumerate() {
        match serde_json::from_str::<Message>(line) {
            Ok(message) => messages.push(message),
            Err(_) if index + 1 == rest.len() => {
                dropped_tail = true;
                break;
            }
            Err(error) => anyhow::bail!("line {}: {error}", index + 2),
        }
    }
    let count = messages.len();
    Ok(JsonlLoad {
        context: Some(Context {
            system_prompt: header.system_prompt,
            messages,
            tools: Vec::new(),
        }),
        written: count,
        needs_rewrite: dropped_tail,
        dropped_tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsagent_ai::{ContentBlock, UserMessage};

    fn message(text: &str) -> Message {
        Message::User(UserMessage {
            content: vec![ContentBlock::text(text)],
            timestamp: 0,
        })
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsagent-session-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn an_empty_jsonl_file_is_rewritten_with_a_header() {
        let dir = temp_dir("empty");
        let path = dir.join("session.jsonl");
        std::fs::write(&path, "").expect("seed an empty file");

        let mut session = Session::open(&path);
        assert!(session.load().expect("load").is_none());
        assert!(session.rewrite, "an empty file needs a rewrite");
        // The empty file is not a clean prefix: appending must be refused.
        assert!(!session.can_append(&[message("hi")]));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_newer_version_file_is_refused_and_never_overwritten() {
        let dir = temp_dir("newer");
        let path = dir.join("session.json");
        let newer = r#"{"version":999,"context":{"messages":[]}}"#;
        std::fs::write(&path, newer).expect("seed a newer file");

        let mut session = Session::open(&path);
        let error = session.load().expect_err("a newer version is refused");
        assert!(error.to_string().contains("newer rsagent"), "{error}");
        assert!(session.read_only, "the session must become read-only");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_compacted_transcript_rewrites_instead_of_appending() {
        let dir = temp_dir("compaction");
        let path = dir.join("session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"header\",\"version\":1}\n\
             {\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"one\"}],\"timestamp\":0}\n\
             {\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"two\"}],\"timestamp\":0}\n",
        )
        .expect("seed a two-message session");

        let mut session = Session::open(&path);
        let context = session.load().expect("load").expect("a context");
        assert_eq!(context.messages.len(), 2);

        // Compaction drops the middle turn: same length, different first message. A count
        // check alone would append and strand the dropped turn on disk.
        let compacted = vec![message("two"), message("three")];
        assert!(
            !session.can_append(&compacted),
            "a changed prefix must force a rewrite"
        );

        // An unchanged prefix can still be appended to.
        let grown = vec![message("one"), message("two"), message("three")];
        assert!(session.can_append(&grown));

        std::fs::remove_dir_all(&dir).ok();
    }
}
