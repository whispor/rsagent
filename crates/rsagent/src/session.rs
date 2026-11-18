//! Session persistence: the transcript, saved to a file and resumed later.
//!
//! Two encodings, chosen by file extension:
//!
//! * **JSON envelope** (default) — one versioned object; rewritten in full on every save.
//! * **JSONL** (`.jsonl`) — a header line followed by one message per line, **appended**
//!   to rather than rewritten, which suits long sessions (`docs/EXTENSIBILITY.md` §5).
//!
//! Both are written atomically (`docs/EXTENSIBILITY.md` §4.2 trap 3): a temp file beside
//! the target, then a rename, so a crash never leaves half a document behind. JSONL
//! append writes can additionally leave a truncated *last* line if a crash interrupts
//! them; [`Session::load`] tolerates that by dropping the incomplete tail.

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

/// A session file: where it lives, how it is encoded, and how much is already written.
pub(crate) struct Session {
    path: PathBuf,
    format: Format,
    /// Messages already persisted (JSONL append mode); always `0` for the JSON envelope.
    written: usize,
    /// Set when the on-disk file cannot be appended to — a truncated tail was dropped, so
    /// its content is no longer a clean prefix of the transcript — forcing a full rewrite.
    rewrite: bool,
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
            rewrite: false,
        }
    }

    /// The file this session is stored in.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Read the persisted transcript, if the file exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed, so the caller
    /// can warn and start fresh instead of pretending the session was never there.
    pub(crate) fn load(&mut self) -> anyhow::Result<Option<Context>> {
        if !self.path.exists() {
            return Ok(None);
        }
        match self.format {
            Format::Json => load_json(&self.path),
            Format::Jsonl => {
                let (context, written, truncated) = load_jsonl(&self.path)?;
                self.written = written;
                self.rewrite = truncated;
                Ok(context)
            }
        }
    }

    /// Write the agent's transcript.
    ///
    /// A write failure is reported but not fatal: losing the persisted session must never
    /// abort an otherwise successful run.
    pub(crate) fn save(&mut self, agent: &Agent, palette: Palette) {
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

    /// Append only the new tail when the transcript has only grown; otherwise rewrite.
    ///
    /// Rewriting is needed when the transcript shrank (a `/clear`, or context compaction
    /// dropping old turns), when the file is missing, or when a truncated tail was dropped
    /// on load — an append cannot remove lines that are already on disk.
    fn save_jsonl(&mut self, context: &Context) -> anyhow::Result<()> {
        let messages = &context.messages;
        if self.path.exists() && !self.rewrite && self.written <= messages.len() {
            let mut appended = String::new();
            for message in &messages[self.written..] {
                appended.push_str(&serde_json::to_string(message)?);
                appended.push('\n');
            }
            if !appended.is_empty() {
                let mut file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
                file.write_all(appended.as_bytes())?;
            }
            self.written = messages.len();
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
        self.written = messages.len();
        self.rewrite = false;
        Ok(())
    }
}

/// Read a JSON-envelope session.
fn load_json(path: &Path) -> anyhow::Result<Option<Context>> {
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

/// Read a JSONL session, returning the transcript, its message count, and whether a
/// truncated tail was dropped.
///
/// A malformed **last** line is dropped rather than reported: it is the signature of an
/// append that a crash interrupted, and the messages before it are still valid. The
/// dropped-tail flag tells [`Session::save`] to rewrite rather than append, so the bad
/// bytes do not end up stranded in the middle of the file.
fn load_jsonl(path: &Path) -> anyhow::Result<(Option<Context>, usize, bool)> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return Ok((None, 0, false));
    };
    let header: JsonlHeader =
        serde_json::from_str(header_line).map_err(|error| anyhow::anyhow!("header: {error}"))?;
    if header.version > SESSION_VERSION {
        anyhow::bail!(
            "written by a newer rsagent (format v{}); this build understands up to v{}",
            header.version,
            SESSION_VERSION
        );
    }

    let rest: Vec<&str> = lines.collect();
    let mut messages = Vec::new();
    let mut truncated = false;
    for (index, line) in rest.iter().enumerate() {
        match serde_json::from_str::<Message>(line) {
            Ok(message) => messages.push(message),
            Err(_) if index + 1 == rest.len() => {
                truncated = true;
                break;
            }
            Err(error) => anyhow::bail!("line {}: {error}", index + 2),
        }
    }
    let count = messages.len();
    Ok((
        Some(Context {
            system_prompt: header.system_prompt,
            messages,
            tools: Vec::new(),
        }),
        count,
        truncated,
    ))
}
