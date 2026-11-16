//! The stateful agent: a turn loop that calls a model, executes tools, and emits events.

use std::collections::VecDeque;
use std::path::PathBuf;

use futures::future::join_all;
use futures::StreamExt;
use rsagent_ai::{
    now_millis, AssistantMessage, ContentBlock, Context, Message, Model, Models, StopReason,
    StreamEvent, StreamOptions, ToolResultMessage,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::approval::Approver;
use crate::cancel::CancelToken;
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::tool::{Tool, ToolContext};

/// A coding agent driven by an event loop.
///
/// Subscribers receive [`AgentEvent`]s over an unbounded channel, which makes it easy to
/// drive a UI from another task while `prompt()` runs to completion.
///
/// The loop mirrors pi's `pi-agent-core`:
/// * a turn is one model call plus its tool executions;
/// * tool calls are preflighted sequentially, then executed in parallel unless a tool
///   declares itself sequential;
/// * [`steer`](Agent::steer) injects messages mid-run, [`follow_up`](Agent::follow_up)
///   queues work for after the agent would otherwise stop;
/// * before each request the transcript is trimmed by whole turns if it would exceed
///   the model's context window, keeping the first and most recent turns;
/// * [`abort`](Agent::abort) cancels the current run.
pub struct Agent {
    models: Models,
    model: Model,
    system_prompt: String,
    messages: Vec<Message>,
    tools: Vec<Box<dyn Tool>>,
    approver: Option<Box<dyn Approver>>,
    subscribers: Vec<UnboundedSender<AgentEvent>>,
    options: StreamOptions,
    cwd: PathBuf,
    max_turns: usize,
    cancel: CancelToken,
    steering: VecDeque<Message>,
    follow_up: VecDeque<Message>,
    is_streaming: bool,
    last_error: Option<String>,
    pending_tool_calls: Vec<String>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("models", &self.models)
            .field("model", &self.model)
            .field("system_prompt", &self.system_prompt)
            .field("messages", &self.messages.len())
            .field("tools", &self.tools.len())
            .field("approver", &self.approver.is_some())
            .field("subscribers", &self.subscribers.len())
            .field("options", &self.options)
            .field("cwd", &self.cwd)
            .field("max_turns", &self.max_turns)
            .field("is_streaming", &self.is_streaming)
            .field("steering", &self.steering.len())
            .field("follow_up", &self.follow_up.len())
            .field("cancel", &self.cancel.is_cancelled())
            .field("last_error", &self.last_error)
            .field("pending_tool_calls", &self.pending_tool_calls.len())
            .finish()
    }
}

impl Agent {
    /// Create an agent that targets `model` and starts with `system_prompt`.
    ///
    /// `models` is a cheap-to-clone registry handle; pass it by value.
    pub fn new(models: Models, model: Model, system_prompt: impl Into<String>) -> Self {
        Agent {
            models,
            model,
            system_prompt: system_prompt.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            approver: None,
            subscribers: Vec::new(),
            options: StreamOptions::default(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            max_turns: 50,
            cancel: CancelToken::new(),
            steering: VecDeque::new(),
            follow_up: VecDeque::new(),
            is_streaming: false,
            last_error: None,
            pending_tool_calls: Vec::new(),
        }
    }

    /// Replace the tool set the agent may call.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    /// Set the approval policy consulted before tools that require approval.
    ///
    /// Without one, every tool runs unconditionally.
    #[must_use]
    pub fn with_approver(mut self, approver: impl Approver + 'static) -> Self {
        self.approver = Some(Box::new(approver));
        self
    }

    /// Set the working directory used to resolve tool paths.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Set the per-request stream options.
    #[must_use]
    pub fn with_options(mut self, options: StreamOptions) -> Self {
        self.options = options;
        self
    }

    /// Set the session id used for provider-side prompt caching.
    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.options.session_id = Some(session_id.into());
        self
    }

    /// Cap how many model turns a single run may take.
    #[must_use]
    pub fn with_max_turns(mut self, max_turns: usize) -> Self {
        self.max_turns = max_turns;
        self
    }

    /// Register an event subscriber.
    pub fn subscribe(&mut self, sender: UnboundedSender<AgentEvent>) {
        self.subscribers.push(sender);
    }

    /// The model this agent currently targets.
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Swap the model used for subsequent turns.
    pub fn set_model(&mut self, model: Model) {
        self.model = model;
    }

    /// The transcript so far.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Replace the transcript, e.g. to resume a persisted session.
    ///
    /// This does not emit events; call it before the first [`prompt`](Agent::prompt).
    pub fn load_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Replace the system prompt used for subsequent turns.
    ///
    /// Used when resuming a persisted session, where the saved prompt should win over
    /// the CLI default.
    pub fn set_system_prompt(&mut self, system_prompt: impl Into<String>) {
        self.system_prompt = system_prompt.into();
    }

    /// Snapshot the transcript and system prompt as a serializable [`Context`].
    pub fn transcript(&self) -> Context {
        Context {
            system_prompt: Some(self.system_prompt.clone()),
            messages: self.messages.clone(),
            tools: Vec::new(),
        }
    }

    /// Whether a run is currently in progress.
    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// The most recent model error message, if the last turn failed.
    pub fn error_message(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Ids of tool calls currently executing.
    pub fn pending_tool_calls(&self) -> Vec<String> {
        self.pending_tool_calls.clone()
    }

    /// Cancel the current run; tools observing the token stop promptly.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// A clone of the cancellation token, usable to abort from another task.
    ///
    /// The flag is cleared at the start of every run, so a stale clone is harmless.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Queue a steering message, injected before the next model request.
    pub fn steer(&mut self, input: impl Into<String>) {
        self.steering.push_back(Message::user(input));
    }

    /// Queue a follow-up message, injected only after the agent would otherwise stop.
    pub fn follow_up(&mut self, input: impl Into<String>) {
        self.follow_up.push_back(Message::user(input));
    }

    /// Drop all queued steering messages.
    pub fn clear_steering_queue(&mut self) {
        self.steering.clear();
    }

    /// Drop all queued follow-up messages.
    pub fn clear_follow_up_queue(&mut self) {
        self.follow_up.clear();
    }

    /// Send a user prompt and run the agent loop to completion.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::NoResult`] if the model stream ends without a result,
    /// or [`AgentError::MaxTurnsExceeded`] if the agent hits its turn limit.
    pub async fn prompt(&mut self, input: impl Into<String>) -> Result<(), AgentError> {
        let user_message = Message::user(input);
        self.messages.push(user_message.clone());

        self.cancel.reset();
        self.emit(&AgentEvent::AgentStart);
        self.emit(&AgentEvent::MessageStart {
            message: user_message.clone(),
        });
        self.emit(&AgentEvent::MessageEnd {
            message: user_message,
        });

        self.run_loop().await
    }

    /// Resume the loop from the existing transcript without appending a new prompt.
    ///
    /// # Errors
    ///
    /// Same as [`prompt`](Agent::prompt).
    pub async fn continue_run(&mut self) -> Result<(), AgentError> {
        self.cancel.reset();
        self.emit(&AgentEvent::AgentStart);
        self.run_loop().await
    }

    /// Run turns until the agent stops, is cancelled, or hits its turn limit.
    async fn run_loop(&mut self) -> Result<(), AgentError> {
        self.is_streaming = true;
        self.last_error = None;
        let result = self.run_turns().await;
        self.is_streaming = false;
        self.pending_tool_calls.clear();
        self.emit(&AgentEvent::AgentEnd {
            messages: self.messages.clone(),
        });
        result
    }

    async fn run_turns(&mut self) -> Result<(), AgentError> {
        for _ in 0..self.max_turns {
            if self.cancel.is_cancelled() {
                return Ok(());
            }
            self.inject_steering();
            self.compact_context();

            self.emit(&AgentEvent::TurnStart);

            let assistant = match self.stream_turn().await? {
                StreamOutcome::Message(message) => message,
                StreamOutcome::Cancelled => return Ok(()),
                StreamOutcome::NoResult => return Err(AgentError::NoResult),
            };

            // Read everything needed off the assistant before moving it into the message,
            // so the message is built without an extra deep clone.
            let stop_reason = assistant.stop_reason;
            let error_message = assistant.error_message.clone();
            let calls: Vec<(String, String, Value)> = assistant
                .tool_calls()
                .into_iter()
                .map(|(id, name, args)| (id.to_string(), name.to_string(), args.clone()))
                .collect();

            let assistant_message = Message::Assistant(assistant);
            self.messages.push(assistant_message.clone());
            self.emit(&AgentEvent::MessageEnd {
                message: assistant_message.clone(),
            });

            // A hard error or abort ends the run.
            if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                self.last_error = error_message;
                self.emit(&AgentEvent::TurnEnd {
                    message: assistant_message,
                    tool_results: Vec::new(),
                });
                return Ok(());
            }

            if stop_reason == StopReason::ToolUse && !calls.is_empty() {
                let (tool_results, all_terminate) = self.run_tool_calls(calls).await;

                self.emit(&AgentEvent::TurnEnd {
                    message: assistant_message,
                    tool_results,
                });
                // pi: stop early only when every finalized result requests termination.
                if all_terminate {
                    return Ok(());
                }
                continue;
            }

            self.emit(&AgentEvent::TurnEnd {
                message: assistant_message,
                tool_results: Vec::new(),
            });

            // No tool calls: a queued follow-up restarts the loop instead of stopping.
            if self.inject_follow_up() {
                continue;
            }
            return Ok(());
        }

        Err(AgentError::MaxTurnsExceeded {
            max_turns: self.max_turns,
        })
    }

    /// Stream one model response, returning the final assistant message.
    async fn stream_turn(&mut self) -> Result<StreamOutcome, AgentError> {
        let context = self.build_context();
        let mut stream = self
            .models
            .stream(&self.model, context, self.options.clone());

        let mut final_message: Option<AssistantMessage> = None;
        let mut announced_start = false;

        loop {
            let next = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Ok(StreamOutcome::Cancelled),
                item = stream.next() => item,
            };
            let Some(event) = next else {
                break;
            };

            if let StreamEvent::Start { partial } = &event {
                announced_start = true;
                self.emit(&AgentEvent::MessageStart {
                    message: Message::Assistant(partial.clone()),
                });
            }
            if let Some(message) = event.terminal_message() {
                final_message = Some(message);
            }
            self.emit(&AgentEvent::MessageUpdate { event });
        }

        let Some(assistant) = final_message else {
            return Ok(StreamOutcome::NoResult);
        };
        if !announced_start {
            self.emit(&AgentEvent::MessageStart {
                message: Message::Assistant(assistant.clone()),
            });
        }
        Ok(StreamOutcome::Message(assistant))
    }

    /// Execute a batch of tool calls, returning the result messages and whether every
    /// result requested termination.
    async fn run_tool_calls(
        &mut self,
        calls: Vec<(String, String, Value)>,
    ) -> (Vec<Message>, bool) {
        // Preflight: consult approval in order, then announce the calls that will run.
        // Approval comes first so the prompt is never preceded by a misleading "start".
        let mut prepared: Vec<PreparedCall> = Vec::with_capacity(calls.len());
        for (call_id, name, args) in calls {
            self.pending_tool_calls.push(call_id.clone());

            if self.check_approval(&call_id, &name, &args).await {
                self.emit(&AgentEvent::ToolExecutionStart {
                    tool_call_id: call_id.clone(),
                    tool_name: name.clone(),
                    label: self.tool_label(&name),
                    args: args.clone(),
                });
                prepared.push(PreparedCall::Run {
                    call_id,
                    name,
                    args,
                });
            } else {
                prepared.push(PreparedCall::Blocked {
                    message: denied_message(&call_id, &name),
                });
            }
        }

        // If any tool in the batch is sequential, the whole batch runs sequentially.
        let sequential = prepared.iter().any(|call| match call {
            PreparedCall::Run { name, .. } => self.tool_is_sequential(name),
            PreparedCall::Blocked { .. } => false,
        });

        let executed: Vec<Executed> = if sequential {
            let mut out = Vec::with_capacity(prepared.len());
            for call in &prepared {
                out.push(self.run_one(call).await);
            }
            out
        } else {
            join_all(prepared.iter().map(|call| self.run_one(call))).await
        };

        let mut results = Vec::with_capacity(executed.len());
        let mut all_terminate = !executed.is_empty();
        for item in executed {
            let mut result = item.result;
            truncate_result(&mut result);

            self.emit(&AgentEvent::ToolExecutionEnd {
                tool_call_id: result.tool_call_id.clone(),
                tool_name: result.tool_name.clone(),
                is_error: result.is_error,
                content: result
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .collect::<Vec<_>>()
                    .join(""),
                details: item.details,
            });
            self.pending_tool_calls
                .retain(|id| id != &result.tool_call_id);

            let message = Message::ToolResult(result);
            self.messages.push(message.clone());
            self.emit(&AgentEvent::MessageStart {
                message: message.clone(),
            });
            self.emit(&AgentEvent::MessageEnd {
                message: message.clone(),
            });
            results.push(message);
            all_terminate &= item.terminate;
        }

        (results, all_terminate)
    }

    /// Execute (or surface as blocked) one preflighted call.
    async fn run_one(&self, call: &PreparedCall) -> Executed {
        match call {
            PreparedCall::Blocked { message } => Executed {
                result: match message {
                    Message::ToolResult(result) => result.clone(),
                    _ => unreachable!("blocked calls always carry a tool result"),
                },
                details: None,
                terminate: false,
            },
            PreparedCall::Run {
                call_id,
                name,
                args,
            } => self.execute_one(call_id, name, args.clone()).await,
        }
    }

    async fn execute_one(&self, call_id: &str, name: &str, args: Value) -> Executed {
        let Some(tool) = self.tools.iter().find(|tool| tool.spec().name == name) else {
            return Executed::error(call_id, name, format!("unknown tool: {name}"));
        };

        let context = ToolContext::new(self.cwd.clone())
            .with_cancel(self.cancel.clone())
            .with_progress(self.progress_sender(call_id));

        match tool.execute(call_id, args, &context).await {
            Ok(result) => Executed {
                result: ToolResultMessage {
                    tool_call_id: call_id.to_string(),
                    tool_name: name.to_string(),
                    content: result.content,
                    is_error: false,
                    timestamp: now_millis(),
                },
                details: result.details,
                terminate: result.terminate,
            },
            Err(message) => Executed::error(call_id, name, message),
        }
    }

    /// Build a progress sink whose updates are forwarded as tool-progress events.
    fn progress_sender(&self, tool_call_id: &str) -> UnboundedSender<String> {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
        if self.subscribers.is_empty() {
            return sender;
        }

        let subscribers = self.subscribers.clone();
        let id = tool_call_id.to_string();
        tokio::spawn(async move {
            while let Some(content) = receiver.recv().await {
                let event = AgentEvent::ToolExecutionUpdate {
                    tool_call_id: id.clone(),
                    content,
                };
                for subscriber in &subscribers {
                    let _ = subscriber.send(event.clone());
                }
            }
        });
        sender
    }

    /// Consult the approval policy for a tool call, emitting request/response events.
    async fn check_approval(&self, call_id: &str, tool_name: &str, args: &Value) -> bool {
        let requires = self
            .tools
            .iter()
            .any(|tool| tool.spec().name == tool_name && tool.requires_approval());
        let Some(approver) = &self.approver else {
            return true;
        };
        if !requires {
            return true;
        }

        // Only announce a request the UI should actually prompt for; an approver that
        // decides on its own (allow-all, deny-all, "stop asking") skips the prompt.
        if approver.needs_prompt(tool_name) {
            self.emit(&AgentEvent::ToolApprovalRequest {
                tool_call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
                args: args.clone(),
            });
        }
        let allowed = approver.approve(tool_name, args).await;
        self.emit(&AgentEvent::ToolApprovalResponse {
            tool_call_id: call_id.to_string(),
            approved: allowed,
        });
        allowed
    }

    /// Inject queued steering messages into the transcript. Returns whether any were.
    fn inject_steering(&mut self) -> bool {
        self.drain_queue(QueueKind::Steering)
    }

    /// Inject queued follow-up messages into the transcript. Returns whether any were.
    fn inject_follow_up(&mut self) -> bool {
        self.drain_queue(QueueKind::FollowUp)
    }

    fn drain_queue(&mut self, kind: QueueKind) -> bool {
        let queue = match kind {
            QueueKind::Steering => &mut self.steering,
            QueueKind::FollowUp => &mut self.follow_up,
        };
        if queue.is_empty() {
            return false;
        }

        let drained: Vec<Message> = queue.drain(..).collect();
        for message in drained {
            self.messages.push(message.clone());
            self.emit(&AgentEvent::MessageStart {
                message: message.clone(),
            });
            self.emit(&AgentEvent::MessageEnd { message });
        }
        true
    }

    /// Drop the oldest whole turns so the next request fits the model's context window.
    ///
    /// The first turn and the most recent turn are always kept — the original task and
    /// the latest exchange survive — while older middle turns are dropped oldest-first.
    /// Turns are dropped whole so a tool call is never separated from its result.
    /// Returns how many messages were dropped.
    fn compact_context(&mut self) -> usize {
        let budget = self.context_budget();
        if budget == u64::MAX {
            return 0;
        }
        let ranges = turn_ranges(&self.messages);
        if ranges.len() < 2 {
            return 0;
        }

        let mut total = estimate_tokens(&self.system_prompt, &self.messages);
        if total <= budget {
            return 0;
        }

        let mut keep = vec![true; ranges.len()];
        let last = ranges.len() - 1;
        let mut index = 1;
        while total > budget && index < last {
            let (start, end) = ranges[index];
            total = total.saturating_sub(estimate_tokens("", &self.messages[start..end]));
            keep[index] = false;
            index += 1;
        }

        let mut rebuilt = Vec::with_capacity(self.messages.len());
        for (idx, (start, end)) in ranges.iter().enumerate() {
            if keep[idx] {
                rebuilt.extend_from_slice(&self.messages[*start..*end]);
            }
        }

        let dropped = self.messages.len() - rebuilt.len();
        if dropped == 0 {
            return 0;
        }
        self.messages = rebuilt;
        self.emit(&AgentEvent::ContextCompacted {
            dropped_messages: dropped,
        });
        dropped
    }

    /// The token budget for one request: three quarters of the model's context window,
    /// leaving room for the response. `u64::MAX` when the window is unknown.
    fn context_budget(&self) -> u64 {
        if self.model.context_window == 0 {
            return u64::MAX;
        }
        self.model.context_window / 4 * 3
    }

    fn tool_label(&self, name: &str) -> String {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .map_or_else(|| name.to_string(), |tool| tool.label())
    }

    fn tool_is_sequential(&self, name: &str) -> bool {
        self.tools
            .iter()
            .any(|tool| tool.spec().name == name && tool.is_sequential())
    }

    fn build_context(&self) -> Context {
        Context {
            system_prompt: Some(self.system_prompt.clone()),
            messages: self.messages.clone(),
            tools: self.tools.iter().map(|tool| tool.spec()).collect(),
        }
    }

    fn emit(&self, event: &AgentEvent) {
        for subscriber in &self.subscribers {
            let _ = subscriber.send(event.clone());
        }
    }
}

/// The result of streaming one turn.
enum StreamOutcome {
    Message(AssistantMessage),
    NoResult,
    Cancelled,
}

/// Which message queue to drain.
#[derive(Clone, Copy)]
enum QueueKind {
    Steering,
    FollowUp,
}

/// A preflighted tool call.
enum PreparedCall {
    Run {
        call_id: String,
        name: String,
        args: Value,
    },
    Blocked {
        message: Message,
    },
}

/// The outcome of executing one tool call.
struct Executed {
    result: ToolResultMessage,
    details: Option<Value>,
    terminate: bool,
}

impl Executed {
    fn error(call_id: &str, name: &str, message: impl Into<String>) -> Self {
        Executed {
            result: ToolResultMessage {
                tool_call_id: call_id.to_string(),
                tool_name: name.to_string(),
                content: vec![ContentBlock::text(message)],
                is_error: true,
                timestamp: now_millis(),
            },
            details: None,
            terminate: false,
        }
    }
}

/// The tool result produced when the approval policy denies a call.
fn denied_message(call_id: &str, name: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: call_id.to_string(),
        tool_name: name.to_string(),
        content: vec![ContentBlock::text(
            "tool call denied by the approval policy",
        )],
        is_error: true,
        timestamp: now_millis(),
    })
}

/// Cap on the characters of a single tool result fed back to the model.
const MAX_TOOL_RESULT_CHARS: usize = 32_000;

/// Truncate an oversized tool result so it cannot blow up the model's context window.
fn truncate_result(result: &mut ToolResultMessage) {
    let total: usize = result
        .content
        .iter()
        .filter_map(ContentBlock::as_text)
        .map(|text| text.chars().count())
        .sum();
    if total <= MAX_TOOL_RESULT_CHARS {
        return;
    }

    let combined: String = result
        .content
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect();
    let kept: String = combined.chars().take(MAX_TOOL_RESULT_CHARS).collect();
    result.content = vec![ContentBlock::text(format!(
        "{kept}\n\n… [truncated: showing {MAX_TOOL_RESULT_CHARS} of {total} characters]"
    ))];
}

/// Split the transcript into turn ranges, each starting at a user message.
///
/// A turn spans from a user message up to (but not including) the next one, so tool
/// calls and their results always live inside the same range and are dropped together.
fn turn_ranges(messages: &[Message]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, message) in messages.iter().enumerate() {
        if index > 0 && matches!(message, Message::User(_)) {
            ranges.push((start, index));
            start = index;
        }
    }
    if start < messages.len() {
        ranges.push((start, messages.len()));
    }
    ranges
}

/// A rough token estimate for a transcript slice plus an optional prefix.
///
/// Four characters per token is close enough for a trimming decision and needs no
/// tokenizer dependency.
fn estimate_tokens(prefix: &str, messages: &[Message]) -> u64 {
    let chars: usize = prefix.chars().count() + messages.iter().map(message_chars).sum::<usize>();
    u64::try_from(chars / 4).unwrap_or(u64::MAX)
}

/// The serialized size of one message, in characters.
fn message_chars(message: &Message) -> usize {
    serde_json::to_string(message).map_or(0, |json| json.chars().count())
}
