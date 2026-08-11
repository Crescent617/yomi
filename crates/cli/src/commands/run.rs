//! `yomi run` — headless one-shot run: send a prompt, wait for the agent to
//! finish, print the result, and exit with a status code.
//!
//! Kernel selection is the global three-way (`--bg` / `--fg` / auto)
//! shared with `tui` — see `crate::daemon::select_kernel`.
//!
//! Termination: the run ends when, after our own user-message echo, the agent
//! emits `Lifecycle Stopped` (or a non-recoverable `Error`, whose code path
//! recovers to Idle without emitting a Stopped). The echo fires when the
//! agent *consumes* the message, so on a busy daemon session everything
//! before it — including the in-flight run's `Stopped` — belongs to another
//! run and is ignored. Permission / `ask_user` requests are always answered
//! immediately (deny / empty answers, approve only with `--yolo`) so they
//! never stall on the kernel's 2-minute response timeout — note this also
//! answers requests raised by a *previous* in-flight run on a busy shared
//! session, so prefer dedicated sessions for `run`.

use crate::args::GlobalArgs;
use crate::session::SessionArg;
use crate::storage::AppStorage;
use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use kernel::client::KernelApi;
use kernel::event::{AgentEvent, AgentStatus, Event, ModelEvent, StopReason, ToolEvent, UserEvent};
use kernel::permission::Level;
use kernel::tools::AskUserResponse;
use kernel::types::{ContentBlock, SessionId};
use kernel::utils::strs;
use std::io::Write as _;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// `send_message` retries (transient IPC errors, daemon restart restore).
const SEND_MAX_RETRIES: u32 = 10;

/// Run a single prompt headlessly: send it, wait for the agent to finish,
/// print the result, and exit non-zero on failure.
#[derive(Parser)]
#[allow(clippy::struct_excessive_bools)] // clap flags are naturally bools
pub struct RunArgs {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(flatten)]
    mode: crate::args::KernelModeArgs,

    /// Prompt text (reads from stdin when omitted)
    prompt: Vec<String>,

    /// Model key to use for this session (overrides default model)
    #[arg(short, long, value_name = "MODEL_KEY")]
    model: Option<String>,

    /// Resume a specific session
    #[arg(short, long, value_name = "SESSION_ID", conflicts_with_all = ["last", "fork", "fork_last"])]
    resume: Option<String>,

    /// Resume the current directory's last session
    #[arg(long, conflicts_with_all = ["fork", "fork_last"])]
    last: bool,

    /// Fork a specific session and run in the copy
    #[arg(short, long, value_name = "SESSION_ID", conflicts_with = "fork_last")]
    fork: Option<String>,

    /// Fork the current directory's last session and run in the copy
    #[arg(long)]
    fork_last: bool,

    /// Output format
    #[arg(long, value_name = "FORMAT", default_value = "text")]
    format: OutputFormat,

    /// Skip all confirmations (approve every tool call)
    #[arg(short, long, conflicts_with = "auto_approve")]
    yolo: bool,

    /// Auto-approve threshold: safe, caution, or dangerous
    #[arg(long, value_name = "LEVEL")]
    auto_approve: Option<String>,

    /// Wall-clock timeout in seconds; cancels the run on expiry (exit 124)
    #[arg(long, value_name = "SECONDS")]
    timeout: Option<u64>,

    /// Do not record this session as the directory's last session
    #[arg(long)]
    ephemeral: bool,

    /// Print tool calls and retries to stderr
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum OutputFormat {
    /// Final assistant message only
    Text,
    /// Single JSON object printed when the run finishes
    Json,
    /// NDJSON event stream (same shape as `yomi events`) plus a final result line
    StreamJson,
}

/// How the run ended; maps to both the JSON `status` field and the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunStatus {
    Completed,
    Failed,
    MaxIterations,
    Cancelled,
    Timeout,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::MaxIterations => "max_iterations",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Completed => 0,
            Self::Failed => 2,
            Self::MaxIterations => 3,
            Self::Cancelled => 130,
            Self::Timeout => 124,
        }
    }
}

/// Tokens consumed across all model requests of the run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

/// Final result of a run, ready for formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RunOutcome {
    status: RunStatus,
    result_text: String,
    num_turns: usize,
    usage: Usage,
    error: Option<String>,
}

impl RunOutcome {
    fn to_json(
        &self,
        session_id: &str,
        model: Option<&str>,
        duration_ms: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "session_id": session_id,
            "status": self.status.as_str(),
            "result": self.result_text,
            "model": model,
            "num_turns": self.num_turns,
            "duration_ms": duration_ms,
            "usage": {
                "prompt_tokens": self.usage.prompt_tokens,
                "completion_tokens": self.usage.completion_tokens,
                "total_tokens": self.usage.total_tokens,
            },
            "error": self.error,
        })
    }
}

/// Side effects the event loop must perform (answered via the kernel).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Effect {
    RespondPermission { req_id: String, approved: bool },
    RespondAskUser { req_id: String },
}

enum Step {
    Continue(Vec<Effect>),
    Done(RunOutcome),
}

/// Pure run state machine: fed every session event, decides when the run is
/// finished and what to answer to pending requests. Kept IO-free for tests.
struct RunState {
    /// Exact text we sent; identifies our message echo.
    sent_text: String,
    /// yolo / dangerous mode: approve permission requests instead of denying.
    approve_all: bool,
    /// True once our own user-message echo has been seen.
    armed: bool,
    /// Last non-empty assistant text (the run's result).
    result_text: String,
    /// Assistant messages seen (one per model turn).
    num_turns: usize,
    usage: Usage,
}

impl RunState {
    fn new(sent_text: String, approve_all: bool) -> Self {
        Self {
            sent_text,
            approve_all,
            armed: false,
            result_text: String::new(),
            num_turns: 0,
            usage: Usage::default(),
        }
    }

    fn finish(&self, status: RunStatus, error: Option<String>) -> RunOutcome {
        RunOutcome {
            status,
            result_text: self.result_text.clone(),
            num_turns: self.num_turns,
            usage: self.usage,
            error,
        }
    }

    fn on_event(&mut self, event: &Event) -> Step {
        match event {
            // Our message echo marks the start of OUR run; it fires at
            // consumption time, so anything before it (a previous run's
            // events on a busy session) must not affect us.
            Event::User(UserEvent::Message { content, .. }) if !self.armed => {
                if blocks_text(content) == self.sent_text {
                    self.armed = true;
                }
                Step::Continue(vec![])
            }
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped { reason },
            }) if self.armed => {
                let (status, error) = match reason {
                    StopReason::Completed { .. } => (RunStatus::Completed, None),
                    StopReason::Failed { error } => (RunStatus::Failed, Some(error.clone())),
                    StopReason::MaxIterations { .. } => (RunStatus::MaxIterations, None),
                    StopReason::Cancelled { .. } => (RunStatus::Cancelled, None),
                };
                Step::Done(self.finish(status, error))
            }
            // The main-loop error path recovers to Idle WITHOUT a Stopped
            // event — treat it as terminal so the run can never hang.
            Event::Agent(AgentEvent::Error {
                error,
                is_recoverable: false,
                ..
            }) if self.armed => Step::Done(self.finish(RunStatus::Failed, Some(error.clone()))),
            Event::Model(ModelEvent::End { content, .. }) if self.armed => {
                self.num_turns += 1;
                let text = blocks_text(content);
                if !text.trim().is_empty() {
                    self.result_text = text;
                }
                Step::Continue(vec![])
            }
            Event::Model(ModelEvent::TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                ..
            }) if self.armed => {
                self.usage.prompt_tokens += u64::from(*prompt_tokens);
                self.usage.completion_tokens += u64::from(*completion_tokens);
                self.usage.total_tokens += u64::from(*total_tokens);
                Step::Continue(vec![])
            }
            // Headless: never leave a request hanging on the kernel's
            // 2-minute response timeout.
            Event::Agent(AgentEvent::PermissionRequest { req_id, .. }) => {
                Step::Continue(vec![Effect::RespondPermission {
                    req_id: req_id.clone(),
                    approved: self.approve_all,
                }])
            }
            Event::Agent(AgentEvent::AskUserQuestion { req_id, .. }) => {
                Step::Continue(vec![Effect::RespondAskUser {
                    req_id: req_id.clone(),
                }])
            }
            _ => Step::Continue(vec![]),
        }
    }
}

/// Concatenated text of all text blocks (thinking/images ignored).
fn blocks_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prompt from positional args (joined with spaces) and/or piped stdin.
fn prompt_from_parts(args: &[String], stdin: Option<String>) -> Result<String> {
    let prompt = if args.is_empty() {
        None
    } else {
        Some(args.join(" "))
    };
    match crate::utils::combine_prompt_stdin(prompt, stdin) {
        Some(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        _ => anyhow::bail!("No prompt provided. Pass it as an argument or pipe it via stdin."),
    }
}

async fn apply_effect(kernel: &dyn KernelApi, session_id: &SessionId, effect: Effect) {
    match effect {
        Effect::RespondPermission { req_id, approved } => {
            if let Err(e) = kernel
                .send_permission_response(session_id, &req_id, approved, false)
                .await
            {
                tracing::warn!("Failed to answer permission request {req_id}: {e}");
            }
        }
        Effect::RespondAskUser { req_id } => {
            let response = AskUserResponse {
                answers: Default::default(),
            };
            if let Err(e) = kernel
                .send_ask_user_response(session_id, &req_id, response)
                .await
            {
                tracing::warn!("Failed to answer ask_user request {req_id}: {e}");
            }
        }
    }
}

/// Progress lines for --verbose (stderr only, stdout stays clean).
fn print_verbose(event: &Event, approve_all: bool) {
    match event {
        Event::Tool(ToolEvent::Start {
            tool_name,
            arguments,
            ..
        }) => {
            let args = arguments.as_deref().unwrap_or("");
            eprintln!(
                "[tool] {tool_name} {}",
                strs::truncate_with_suffix(args, 80, "...")
            );
        }
        Event::Tool(ToolEvent::End {
            tool_name,
            elapsed_ms,
            is_error,
            ..
        }) => {
            eprintln!(
                "[tool] {tool_name} done in {elapsed_ms}ms{}",
                if *is_error { " (error)" } else { "" }
            );
        }
        Event::Agent(AgentEvent::Retrying {
            attempt,
            max_attempts,
            reason,
            wait_ms,
        }) => {
            eprintln!("[retry] {attempt}/{max_attempts}: {reason} (in {wait_ms}ms)");
        }
        Event::Agent(AgentEvent::Error {
            phase,
            error,
            is_recoverable,
        }) => {
            eprintln!("[error] {phase:?} (recoverable={is_recoverable}): {error}");
        }
        Event::Agent(AgentEvent::PermissionRequest {
            tool_name,
            tool_args,
            ..
        }) => {
            if approve_all {
                eprintln!("[permission] approved: {tool_name}");
            } else {
                eprintln!("[permission] denied (headless): {tool_name} {tool_args}");
            }
        }
        Event::Agent(AgentEvent::AskUserQuestion { .. }) => {
            eprintln!("[ask_user] answered with empty answers (headless)");
        }
        Event::Model(ModelEvent::Compacting { active: true }) => {
            eprintln!("[compact] compacting context...");
        }
        _ => {}
    }
}

/// Drive the event loop until the run finishes, answering permission /
/// `ask_user` requests on the way. Stdout IO errors (e.g. closed pipe in
/// stream-json mode) propagate.
async fn drive_event_loop(
    kernel: &dyn KernelApi,
    session_id: &SessionId,
    events: &mut kernel::comms::EventBusSubscriber,
    state: &mut RunState,
    args: &RunArgs,
) -> Result<RunOutcome> {
    let deadline = args
        .timeout
        .map(|secs| tokio::time::Instant::now() + Duration::from_secs(secs));
    // A lost daemon also kills the server-side subscription and the bridge
    // receiver may just go quiet — poll liveness so we don't hang forever.
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));

    let outcome = loop {
        let timeout = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                let _ = kernel.cancel(session_id).await;
                break state.finish(RunStatus::Cancelled, None);
            }
            () = timeout => {
                let _ = kernel.cancel(session_id).await;
                break state.finish(RunStatus::Timeout, None);
            }
            _ = watchdog.tick() => {
                if !kernel.is_connected().await {
                    break state.finish(
                        RunStatus::Failed,
                        Some("Lost connection to daemon".to_string()),
                    );
                }
            }
            item = events.recv() => {
                let Some((_sid, envelope)) = item else {
                    break state.finish(
                        RunStatus::Failed,
                        Some("Event stream closed before the run finished".to_string()),
                    );
                };
                if args.format == OutputFormat::StreamJson {
                    let stdout = std::io::stdout();
                    let mut out = stdout.lock();
                    writeln!(out, "{}", serde_json::to_string(&envelope)?)?;
                    out.flush()?;
                }
                if args.verbose {
                    print_verbose(&envelope.event, state.approve_all);
                }
                match state.on_event(&envelope.event) {
                    Step::Done(outcome) => break outcome,
                    Step::Continue(effects) => {
                        for effect in effects {
                            apply_effect(kernel, session_id, effect).await;
                        }
                    }
                }
            }
        }
    };
    Ok(outcome)
}

pub async fn run(args: RunArgs) -> Result<()> {
    let working_dir = crate::utils::resolve_working_dir(&args.global)?;
    let mut config = crate::utils::load_config(args.global.config.as_ref())?;

    if args.yolo {
        config.auto_approve = Level::Dangerous;
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    } else if let Some(level) = &args.auto_approve {
        config.auto_approve = Level::from_str(level).map_err(|_| {
            anyhow::anyhow!("Unknown level '{level}' (expected safe, caution, or dangerous)")
        })?;
    }
    let approve_all = config.auto_approve == Level::Dangerous;

    tokio::fs::create_dir_all(&config.data_dir).await?;
    let app_storage = Arc::new(AppStorage::new(config.data_dir.clone())?);
    let _log_guard = kernel::utils::logging::init_logging(&config, "run", false)?;

    let prompt = prompt_from_parts(&args.prompt, crate::utils::read_piped_stdin().await)?;

    let (kernel, _daemon_mode) = crate::daemon::select_kernel(&args.mode, &config).await?;

    let session_arg = if let Some(fork) = &args.fork {
        SessionArg::ForkSpecific(fork.clone())
    } else if args.fork_last {
        SessionArg::ForkLast
    } else if args.last {
        SessionArg::Last
    } else if let Some(resume) = &args.resume {
        SessionArg::Specific(resume.clone())
    } else {
        SessionArg::New
    };

    // Fail fast on a typo'd --resume=<id>: resolve_session would silently
    // fall back to a fresh session — fine for the interactive TUI, but in
    // scripts you want to know the session you asked for doesn't exist.
    if let SessionArg::Specific(id) = &session_arg {
        kernel
            .get_session(&SessionId::from(id.clone()))
            .await
            .with_context(|| format!("Session {id} not found"))?;
    }

    let session_id = crate::session::resolve_session(
        &session_arg,
        true,
        kernel.as_ref(),
        &app_storage,
        &working_dir,
        config.auto_approve,
        args.model.clone(),
    )
    .await?;

    // Subscribe BEFORE sending so no event of our run can be missed.
    let mut events = kernel.subscribe_session_events(&session_id, None).await?;

    let blocks = vec![ContentBlock::Text {
        text: prompt.clone(),
    }];
    crate::session::send_with_retry(kernel.as_ref(), &session_id, blocks, SEND_MAX_RETRIES)
        .await
        .with_context(|| format!("Failed to send message to session {}", session_id.0))?;

    let started = std::time::Instant::now();
    let mut state = RunState::new(prompt, approve_all);
    let outcome =
        drive_event_loop(kernel.as_ref(), &session_id, &mut events, &mut state, &args).await?;

    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let model = kernel.get_session_model(&session_id).await.ok();

    if !args.ephemeral {
        if let Err(e) = app_storage.save_session(&working_dir, &session_id.0).await {
            tracing::warn!("Failed to save session: {e}");
        }
    }
    kernel.stop();

    match args.format {
        OutputFormat::Text => {
            if !outcome.result_text.is_empty() {
                println!("{}", outcome.result_text);
            }
            if let Some(error) = &outcome.error {
                eprintln!("Error: {error}");
            } else if outcome.result_text.is_empty() && outcome.status != RunStatus::Completed {
                eprintln!("Run ended with status: {}", outcome.status.as_str());
            }
        }
        OutputFormat::Json => {
            let json = outcome.to_json(&session_id.0, model.as_deref(), duration_ms);
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        OutputFormat::StreamJson => {
            let mut json = outcome.to_json(&session_id.0, model.as_deref(), duration_ms);
            json["type"] = serde_json::Value::from("result");
            println!("{}", serde_json::to_string(&json)?);
        }
    }
    std::io::stdout().flush()?;

    let code = outcome.status.exit_code();
    if code == 0 {
        Ok(())
    } else {
        std::process::exit(code);
    }
}

#[cfg(test)]
#[path = "run_test.rs"]
mod tests;
