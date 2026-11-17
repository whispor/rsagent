//! Terminal input: one owner of stdin, shared by the REPL and the approval prompt.
//!
//! Both the interactive prompt and the approval prompt need to read a line. Two
//! independent readers over the same terminal can swallow each other's bytes, so a
//! single background thread owns stdin and services requests in turn.
//!
//! When stdin is a terminal the thread uses `rustyline`, giving the REPL line editing
//! and history. When stdin is a pipe (tests, scripts) it falls back to plain line
//! reads, so piping still works exactly as before.

use std::io::{self, IsTerminal, Write};

use tokio::sync::{mpsc, oneshot};

/// Whether an answer is recorded in the editing history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recall {
    /// Record the answer so it can be recalled with the up arrow (REPL prompts).
    Remember,
    /// Do not record the answer (one-off approval answers).
    Forget,
}

/// A request for one line of input, serviced by the input thread.
struct InputRequest {
    /// The prompt to show before reading.
    prompt: String,
    /// Whether to record the answer in the editing history.
    recall: Recall,
    /// Where the answer is delivered.
    reply: oneshot::Sender<io::Result<Option<String>>>,
}

/// A handle to the input thread. Cheap to clone; every clone shares one reader.
#[derive(Debug, Clone)]
pub(crate) struct InputHandle {
    tx: mpsc::UnboundedSender<InputRequest>,
}

impl InputHandle {
    /// Spawn the input thread and return a handle to it.
    pub(crate) fn spawn() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<InputRequest>();
        std::thread::Builder::new()
            .name("rsagent-input".to_string())
            .spawn(move || {
                let mut editor = if io::stdin().is_terminal() {
                    rustyline::DefaultEditor::new().ok()
                } else {
                    None
                };
                while let Some(request) = rx.blocking_recv() {
                    let result = read_one(&mut editor, &request.prompt, request.recall);
                    let _ = request.reply.send(result);
                }
            })
            .expect("spawn the input thread");
        InputHandle { tx }
    }

    /// Read a REPL prompt line, recorded in the editing history.
    pub(crate) async fn read_prompt(&self, prompt: &str) -> io::Result<Option<String>> {
        self.request(prompt, Recall::Remember).await
    }

    /// Read a one-off answer, not recorded in the editing history.
    pub(crate) async fn read_answer(&self, prompt: &str) -> io::Result<Option<String>> {
        self.request(prompt, Recall::Forget).await
    }

    async fn request(&self, prompt: &str, recall: Recall) -> io::Result<Option<String>> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(InputRequest {
                prompt: prompt.to_string(),
                recall,
                reply,
            })
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "the input thread has stopped")
            })?;
        answer.await.map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "the input thread has stopped")
        })?
    }
}

/// Read one line, preferring the editor and falling back to a plain read.
fn read_one(
    editor: &mut Option<rustyline::DefaultEditor>,
    prompt: &str,
    recall: Recall,
) -> io::Result<Option<String>> {
    if let Some(active) = editor.as_mut() {
        match active.readline(prompt) {
            Ok(line) => {
                if recall == Recall::Remember {
                    let _ = active.add_history_entry(line.as_str());
                }
                return Ok(Some(line));
            }
            // End of input: the caller stops the loop.
            Err(rustyline::error::ReadlineError::Eof) => return Ok(None),
            // Ctrl-C at the prompt: hand back an empty line so the REPL just reprompts.
            Err(rustyline::error::ReadlineError::Interrupted) => return Ok(Some(String::new())),
            // Any other editor failure: fall through and read plainly from here on.
            Err(_) => {}
        }
    }
    *editor = None;

    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}
