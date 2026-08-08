//! Supervises runner-owned agent loops without reimplementing them.
//!
//! Each active turn has one worker that exclusively owns its child process and
//! stdin. Tauri commands communicate with that worker over channels, so no
//! process or pending-request lock is ever held across `wait`, channel delivery
//! or a blocking write. Normalised events commit to SQLite before the UI sees
//! them; runner JSON itself never crosses the persistence boundary.

mod claude;
mod codex;
mod sanitize;

pub use crate::providers::Runner;

use crate::store::sessions::{
    self, ChatSession, ChatTurn, FailureKind, NewSession, NewTurn, PermissionPromptData,
    PermissionQuestion, QueuedTurn, SessionDetail, SessionEvent, SessionSummary, StoredEvent,
    TurnStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use uuid::Uuid;

const MAX_PROTOCOL_LINE_BYTES: usize = 1024 * 1024;
const PROCESS_POLL: Duration = Duration::from_millis(25);
const INTERRUPT_GRACE: Duration = Duration::from_millis(750);
const POST_EXIT_DRAIN: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub interactive_approvals: bool,
    pub dangerous: bool,
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyCapabilities {
    pub runner: Runner,
    pub available: bool,
    pub protocol: String,
    pub default_option_id: Option<String>,
    pub options: Vec<SafetyOption>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionReply {
    pub decision: PermissionDecision,
    #[serde(default)]
    pub updated_input: Option<Value>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub answers: Option<Value>,
    #[serde(default)]
    pub content: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionDecision {
    AllowOnce,
    AllowSession,
    Deny,
    Cancel,
    Submit,
}

impl PermissionDecision {
    fn persisted(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow-once",
            Self::AllowSession => "allow-session",
            Self::Deny => "deny",
            Self::Cancel => "cancel",
            Self::Submit => "submit",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineEvent {
    pub session_id: String,
    pub stored: StoredEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Durable start acknowledgement. `turn` is the queued row created before the
/// worker starts; terminal state arrives through `EngineEvent` and rehydration.
pub struct RunReceipt {
    pub session: ChatSession,
    pub turn: ChatTurn,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub safety_option_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<SupervisorInner>,
}

struct SupervisorInner {
    active: Mutex<HashMap<String, ActiveRegistration>>,
    pending: Mutex<HashMap<String, PendingRegistration>>,
    safety_cache: Mutex<BTreeMap<Runner, SafetyCapabilities>>,
    shutting_down: AtomicBool,
}

#[derive(Clone)]
struct ActiveRegistration {
    id: Uuid,
    control: mpsc::Sender<ControlMessage>,
    cancel_requested: Arc<AtomicBool>,
}

struct RegisteredTurn {
    id: Uuid,
    control: mpsc::Sender<ControlMessage>,
    receiver: mpsc::Receiver<ControlMessage>,
    cancel_requested: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveSignal {
    Delivered,
    Missing,
    ReceiverClosed,
}

#[derive(Clone)]
struct PendingRegistration {
    turn_id: String,
    control: mpsc::Sender<ControlMessage>,
    options: Vec<String>,
}

#[derive(Debug)]
enum ControlMessage {
    Permission {
        public_id: String,
        reply: PermissionReply,
    },
    Interrupt,
    Shutdown,
}

#[derive(Debug)]
pub(super) struct Launch {
    pub command: Command,
    pub initial_lines: Vec<String>,
    pub close_stdin_after_initial: bool,
    pub kill_after_terminal: bool,
}

#[derive(Debug)]
enum Protocol {
    Claude(claude::Protocol),
    Codex(codex::Protocol),
    CodexFallback(codex::ExecProtocol),
}

#[derive(Debug)]
pub(super) enum ProtocolAction {
    BindSession(String),
    Event(SessionEvent),
    Permission(PermissionPrompt),
    CancelPermission(String),
    Send(Value),
    Terminal(ProtocolTerminal),
}

#[derive(Debug)]
pub(super) struct PermissionPrompt {
    pub wire_key: String,
    pub tool_name: String,
    pub summary: String,
    pub options: Vec<String>,
    pub prompt: Option<PermissionPromptData>,
    pub payload: PendingPayload,
}

#[derive(Debug)]
pub(super) enum PendingPayload {
    Claude {
        request_id: String,
        input: Value,
    },
    CodexDecision {
        rpc_id: Value,
        kind: CodexDecisionKind,
    },
    CodexPermissions {
        rpc_id: Value,
        permissions: Value,
    },
    CodexUserInput {
        rpc_id: Value,
        questions: Vec<PermissionQuestion>,
    },
    CodexElicitation {
        rpc_id: Value,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) enum CodexDecisionKind {
    Command,
    FileChange,
}

#[derive(Debug)]
pub(super) struct PermissionWireResponse {
    pub line: Value,
    pub interrupt: bool,
}

#[derive(Debug, Clone)]
pub(super) enum ProtocolTerminal {
    Completed {
        duration_ms: Option<u64>,
    },
    Failed {
        message: String,
        duration_ms: Option<u64>,
    },
    Interrupted,
}

#[derive(Debug)]
enum DriveOutcome {
    Completed(Option<u64>),
    Failed(FailureKind, String, Option<u64>),
    Interrupted,
}

#[derive(Debug)]
enum OutputMessage {
    Line(String),
    Error(String),
    Eof,
}

#[derive(Debug)]
enum WriterMessage {
    Line(String),
    Close,
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SupervisorInner {
                active: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                safety_cache: Mutex::new(BTreeMap::new()),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    pub fn reconcile_startup(&self) -> Result<usize, String> {
        sessions::reconcile_interrupted_turns()
    }

    pub fn safety_capabilities(&self, runner: Runner) -> SafetyCapabilities {
        if let Some(hit) = self
            .inner
            .safety_cache
            .lock()
            .expect("safety cache poisoned")
            .get(&runner)
            .cloned()
        {
            return hit;
        }
        let discovered = match runner {
            Runner::ClaudeCode => claude::discover_safety(),
            Runner::Codex => codex::discover_safety(),
        };
        self.inner
            .safety_cache
            .lock()
            .expect("safety cache poisoned")
            .insert(runner, discovered.clone());
        discovered
    }

    pub fn create_and_run(
        &self,
        runner: Runner,
        prompt: String,
        cwd: Option<String>,
        title: Option<String>,
        options: RunOptions,
        channel: Channel<EngineEvent>,
    ) -> Result<RunReceipt, String> {
        let cwd = canonical_cwd(cwd.as_deref())?;
        let safety = self.resolve_safety(runner, options.safety_option_id.as_deref())?;
        let title = title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| title_from_prompt(&prompt));
        let queued = sessions::create_session_with_turn(
            NewSession {
                runner,
                // Claude's supported SDK transport accepts a caller-assigned
                // UUID. Persisting it in the same transaction as the first
                // turn makes even a crash between spawn and init resumable.
                runner_session_id: matches!(runner, Runner::ClaudeCode)
                    .then(|| Uuid::new_v4().to_string()),
                cwd: cwd.to_string_lossy().into_owned(),
                title,
            },
            NewTurn {
                prompt,
                requested_model: options.model,
                requested_effort: options.effort,
                permission_mode: safety.option_id().to_string(),
            },
        )?;
        self.start_queued(queued, safety, channel)
    }

    /// Resolves a current bundle and commits its immutable attachment together
    /// with the session and first queued turn. Runner, cwd and model come only
    /// from that resolved revision; the webview never supplies a snapshot.
    pub fn create_and_run_with_bundle(
        &self,
        bundle_id: &str,
        expected_revision: i64,
        prompt: String,
        title: Option<String>,
        options: RunOptions,
        channel: Channel<EngineEvent>,
    ) -> Result<RunReceipt, String> {
        let catalog = crate::store::bundles::LiveTargetCatalog::scan();
        let prepared =
            crate::store::bundles::resolve_for_attachment(bundle_id, expected_revision, &catalog)
                .map_err(|error| error.to_string())?;
        crate::store::bundles::validate_chat_support(&prepared)
            .map_err(|error| error.to_string())?;
        if options.model.is_some() && options.model != prepared.model_id {
            return Err("the bundle locks the chat model for its attached session".to_string());
        }
        let runner = prepared.runner;
        let cwd = prepared.cwd.clone();
        let locked_model = prepared.model_id.clone();
        let safety = self.resolve_safety(runner, options.safety_option_id.as_deref())?;
        let title = title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| title_from_prompt(&prompt));
        let queued = crate::store::bundles::create_session_with_bundle_turn(
            NewSession {
                runner,
                runner_session_id: matches!(runner, Runner::ClaudeCode)
                    .then(|| Uuid::new_v4().to_string()),
                cwd,
                title,
            },
            NewTurn {
                prompt,
                requested_model: locked_model,
                requested_effort: options.effort,
                permission_mode: safety.option_id().to_string(),
            },
            prepared,
        )
        .map_err(|error| error.to_string())?;
        self.start_queued(queued, safety, channel)
    }

    pub fn resume_and_run(
        &self,
        session_id: &str,
        prompt: String,
        options: RunOptions,
        channel: Channel<EngineEvent>,
    ) -> Result<RunReceipt, String> {
        let detail = sessions::load_session(session_id)?
            .ok_or_else(|| "chat session not found".to_string())?;
        let cwd = canonical_cwd(Some(&detail.session.cwd))?;
        if cwd.to_string_lossy() != detail.session.cwd {
            return Err("the chat session working directory no longer resolves to its original canonical path".to_string());
        }
        // A stored permissive choice is historical display data. Reopening a
        // session without an explicit selection always resolves the runner's
        // currently discovered safe default.
        let safety =
            self.resolve_safety(detail.session.runner, options.safety_option_id.as_deref())?;
        let turn = sessions::queue_turn(
            session_id,
            NewTurn {
                prompt,
                requested_model: options.model,
                requested_effort: options.effort,
                permission_mode: safety.option_id().to_string(),
            },
        )?;
        self.start_queued(
            QueuedTurn {
                session: detail.session,
                turn,
            },
            safety,
            channel,
        )
    }

    pub fn respond_permission(
        &self,
        request_id: &str,
        reply: PermissionReply,
    ) -> Result<(), String> {
        let registration = {
            let mut pending = self
                .inner
                .pending
                .lock()
                .expect("pending request map poisoned");
            let registration = pending
                .get(request_id)
                .ok_or_else(|| "permission request is no longer pending".to_string())?;
            if !registration
                .options
                .iter()
                .any(|option| option == reply.decision.persisted())
            {
                return Err(format!(
                    "decision {:?} is not offered for this permission request",
                    reply.decision.persisted()
                ));
            }
            pending
                .remove(request_id)
                .expect("pending request checked above")
        };
        registration
            .control
            .send(ControlMessage::Permission {
                public_id: request_id.to_string(),
                reply,
            })
            .map_err(|_| {
                format!(
                    "turn {} ended before the permission response arrived",
                    registration.turn_id
                )
            })
    }

    pub fn interrupt(&self, turn_id: &str) -> Result<(), String> {
        let signal = self.signal_active(turn_id, ControlMessage::Interrupt);
        if signal == ActiveSignal::Delivered {
            return Ok(());
        }
        // Registration is installed before the start receipt is returned. A
        // queued row without one is therefore abandoned work (or a worker
        // whose receiver failed), and can be reconciled directly.
        let detail = find_turn(turn_id)?;
        if detail.status == TurnStatus::Queued
            || signal == ActiveSignal::ReceiverClosed && detail.status == TurnStatus::Running
        {
            sessions::interrupt_turn(turn_id)?;
            return Ok(());
        }
        Err("chat turn is not active".to_string())
    }

    pub fn shutdown(&self) {
        if self.inner.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let controls = self
            .inner
            .active
            .lock()
            .expect("active turn map poisoned")
            .values()
            .map(|registration| {
                registration.cancel_requested.store(true, Ordering::SeqCst);
                registration.control.clone()
            })
            .collect::<Vec<_>>();
        for control in controls {
            let _ = control.send(ControlMessage::Shutdown);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let empty = self
                .inner
                .active
                .lock()
                .expect("active turn map poisoned")
                .is_empty();
            if empty {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn resolve_safety(
        &self,
        runner: Runner,
        requested: Option<&str>,
    ) -> Result<SelectedSafety, String> {
        let capabilities = self.safety_capabilities(runner);
        if !capabilities.available {
            return Err(capabilities
                .warning
                .unwrap_or_else(|| format!("{} is not installed", runner.label())));
        }
        let id = match requested {
            Some(id) => id,
            None => capabilities
                .default_option_id
                .as_deref()
                .ok_or_else(|| "runner has no safe execution mode".to_string())?,
        };
        if !capabilities.options.iter().any(|option| option.id == id) {
            return Err(format!(
                "safety option {id:?} is not supported by the installed {}",
                runner.label()
            ));
        }
        match runner {
            Runner::ClaudeCode => claude::select_safety(id),
            Runner::Codex => codex::select_safety(id, &capabilities.protocol),
        }
    }

    /// Registers control before returning the durable queued turn. That start
    /// receipt is the truthful IPC acknowledgement the UI uses to expose Stop;
    /// a runner `started` event is still emitted only by the runner adapter.
    fn start_queued(
        &self,
        queued: QueuedTurn,
        safety: SelectedSafety,
        channel: Channel<EngineEvent>,
    ) -> Result<RunReceipt, String> {
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            let stored = sessions::fail_turn(
                &queued.turn.id,
                FailureKind::Internal,
                "Aviary is shutting down".to_string(),
                None,
            )?;
            deliver(&queued.session.id, stored, &channel);
            return Err("Aviary is shutting down".to_string());
        }
        let receipt = RunReceipt {
            session: queued.session.clone(),
            turn: queued.turn.clone(),
        };
        let registration = match self.register_turn(&queued.turn.id) {
            Ok(registration) => registration,
            Err(error) => {
                let stored = sessions::fail_turn(
                    &queued.turn.id,
                    FailureKind::Internal,
                    sanitize::text(&error),
                    None,
                )?;
                deliver(&queued.session.id, stored, &channel);
                return Err(error);
            }
        };
        let registration_id = registration.id;
        let turn_id = queued.turn.id.clone();
        let session_id = queued.session.id.clone();
        let fallback_channel = channel.clone();
        let panic_channel = channel.clone();
        let worker = self.clone();
        let panic_turn_id = turn_id.clone();
        let panic_session_id = session_id.clone();
        let spawn = thread::Builder::new()
            .name("aviary-chat-turn".to_string())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.drive_registered(queued, safety, channel, registration)
                }));
                if outcome.is_err() {
                    worker.reconcile_worker_panic(
                        &panic_session_id,
                        &panic_turn_id,
                        registration_id,
                        &panic_channel,
                    );
                }
            });
        if let Err(error) = spawn {
            self.claim_completion(&turn_id, registration_id);
            let stored = sessions::fail_turn(
                &turn_id,
                FailureKind::Internal,
                "Aviary could not start its runner worker".to_string(),
                None,
            )?;
            deliver(&session_id, stored, &fallback_channel);
            return Err(format!("could not start runner worker: {error}"));
        }
        Ok(receipt)
    }

    fn register_turn(&self, turn_id: &str) -> Result<RegisteredTurn, String> {
        let (control, receiver) = mpsc::channel();
        let registration = ActiveRegistration {
            id: Uuid::new_v4(),
            control: control.clone(),
            cancel_requested: Arc::new(AtomicBool::new(false)),
        };
        let mut active = self.inner.active.lock().expect("active turn map poisoned");
        if active.contains_key(turn_id) {
            return Err("chat turn is already registered".to_string());
        }
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            return Err("Aviary is shutting down".to_string());
        }
        active.insert(turn_id.to_string(), registration.clone());
        Ok(RegisteredTurn {
            id: registration.id,
            control,
            receiver,
            cancel_requested: registration.cancel_requested,
        })
    }

    fn signal_active(&self, turn_id: &str, message: ControlMessage) -> ActiveSignal {
        let mut active = self.inner.active.lock().expect("active turn map poisoned");
        let Some(registration) = active.get(turn_id) else {
            return ActiveSignal::Missing;
        };
        registration.cancel_requested.store(true, Ordering::SeqCst);
        let id = registration.id;
        if registration.control.send(message).is_ok() {
            ActiveSignal::Delivered
        } else {
            if active.get(turn_id).is_some_and(|current| current.id == id) {
                active.remove(turn_id);
            }
            ActiveSignal::ReceiverClosed
        }
    }

    /// Linearises terminal persistence with interrupt acknowledgement. Once a
    /// worker removes its matching registration, a later Stop cannot report
    /// success and then lose a race to a completed terminal event.
    fn claim_completion(&self, turn_id: &str, registration_id: Uuid) -> bool {
        let mut active = self.inner.active.lock().expect("active turn map poisoned");
        let Some(registration) = active.get(turn_id) else {
            return false;
        };
        if registration.id != registration_id {
            return false;
        }
        let cancelled = registration.cancel_requested.load(Ordering::SeqCst);
        active.remove(turn_id);
        cancelled
    }

    fn reconcile_worker_panic(
        &self,
        session_id: &str,
        turn_id: &str,
        registration_id: Uuid,
        channel: &Channel<EngineEvent>,
    ) {
        let cancelled = self.claim_completion(turn_id, registration_id);
        let result = if cancelled {
            sessions::interrupt_turn_with_event(turn_id, None)
        } else {
            sessions::fail_turn(
                turn_id,
                FailureKind::Internal,
                "Aviary's runner worker stopped unexpectedly".to_string(),
                None,
            )
        };
        match result {
            Ok(stored) => deliver(session_id, stored, channel),
            Err(error) => log::error!("could not reconcile a stopped runner worker: {error}"),
        }
    }

    fn drive_registered(
        &self,
        queued: QueuedTurn,
        safety: SelectedSafety,
        channel: Channel<EngineEvent>,
        registration: RegisteredTurn,
    ) {
        let started = Instant::now();
        let result = if registration.cancel_requested.load(Ordering::SeqCst)
            || self.inner.shutting_down.load(Ordering::SeqCst)
        {
            Ok(DriveOutcome::Interrupted)
        } else if let Err(error) = sessions::mark_turn_running(&queued.turn.id) {
            Err((FailureKind::Internal, error))
        } else if registration.cancel_requested.load(Ordering::SeqCst)
            || self.inner.shutting_down.load(Ordering::SeqCst)
        {
            Ok(DriveOutcome::Interrupted)
        } else {
            self.drive_process(
                &queued,
                safety,
                &channel,
                &registration.control,
                &registration.receiver,
                &registration.cancel_requested,
            )
        };
        let elapsed = u64::try_from(started.elapsed().as_millis()).ok();
        let cancellation_won = self.claim_completion(&queued.turn.id, registration.id);
        let result = if cancellation_won {
            Ok(DriveOutcome::Interrupted)
        } else {
            result
        };
        let final_result: Result<(), String> = (|| match result {
            Ok(DriveOutcome::Completed(duration)) => {
                let stored = sessions::complete_turn(&queued.turn.id, duration.or(elapsed))?;
                deliver(&queued.session.id, stored, &channel);
                Ok(())
            }
            Ok(DriveOutcome::Interrupted) => {
                let stored = sessions::interrupt_turn_with_event(&queued.turn.id, elapsed)?;
                deliver(&queued.session.id, stored, &channel);
                Ok(())
            }
            Ok(DriveOutcome::Failed(kind, message, duration)) => {
                let stored = sessions::fail_turn(
                    &queued.turn.id,
                    kind,
                    sanitize::text(&message),
                    duration.or(elapsed),
                )?;
                deliver(&queued.session.id, stored, &channel);
                Ok(())
            }
            Err((kind, message)) => {
                let stored =
                    sessions::fail_turn(&queued.turn.id, kind, sanitize::text(&message), elapsed)?;
                deliver(&queued.session.id, stored, &channel);
                Ok(())
            }
        })();
        if let Err(error) = final_result {
            log::error!("could not persist terminal state for chat turn: {error}");
        }
    }

    fn drive_process(
        &self,
        queued: &QueuedTurn,
        safety: SelectedSafety,
        channel: &Channel<EngineEvent>,
        control_tx: &mpsc::Sender<ControlMessage>,
        control_rx: &mpsc::Receiver<ControlMessage>,
        cancel_requested: &AtomicBool,
    ) -> Result<DriveOutcome, (FailureKind, String)> {
        if cancel_requested.load(Ordering::SeqCst) {
            return Ok(DriveOutcome::Interrupted);
        }
        let (mut protocol, mut launch) = protocol_and_launch(queued, safety)?;
        if cancel_requested.load(Ordering::SeqCst) {
            return Ok(DriveOutcome::Interrupted);
        }
        configure_process_group(&mut launch.command);
        let mut child = launch
            .command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                (
                    FailureKind::Spawn,
                    format!("could not start {}: {error}", queued.session.runner.label()),
                )
            })?;
        let process_group = child.id();
        let stdin = child
            .stdin
            .take()
            .expect("piped runner stdin must be available after spawn");
        let stdout = child
            .stdout
            .take()
            .expect("piped runner stdout must be available after spawn");
        let stderr = child
            .stderr
            .take()
            .expect("piped runner stderr must be available after spawn");

        let (output_tx, output_rx) = mpsc::sync_channel(128);
        let (stdout_done_tx, stdout_done_rx) = mpsc::channel();
        let stdout_thread = thread::spawn(move || {
            read_protocol_lines(stdout, output_tx);
            let _ = stdout_done_tx.send(());
        });
        let (stderr_tx, stderr_rx) = mpsc::channel();
        let stderr_thread = thread::spawn(move || {
            let drained = drain(stderr);
            let _ = stderr_tx.send(drained);
        });
        let (writer_tx, writer_rx) = mpsc::sync_channel(64);
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let writer_thread =
            thread::spawn(move || write_runner_stdin(stdin, writer_rx, writer_done_tx));
        let mut initial_failure = None;
        if !cancel_requested.load(Ordering::SeqCst) {
            for line in &launch.initial_lines {
                if let Err(error) = queue_line(&writer_tx, line.clone()) {
                    initial_failure = Some((FailureKind::Input, error));
                    break;
                }
            }
        }
        let mut writer = if launch.close_stdin_after_initial {
            if initial_failure.is_none() {
                if let Err(error) = writer_tx.try_send(WriterMessage::Close) {
                    initial_failure = Some((
                        FailureKind::Input,
                        format!("could not close runner stdin after its initial prompt: {error}"),
                    ));
                }
            }
            drop(writer_tx);
            None
        } else {
            Some(writer_tx)
        };
        let mut local_pending = HashMap::<String, (String, PendingPayload)>::new();
        let mut terminal = None;
        let mut interrupted = false;
        let mut interrupt_deadline = None;
        let mut intended_kill = false;
        let mut stdout_eof = false;
        let mut process_status: Option<ExitStatus> = None;
        let mut post_exit_deadline = None;
        let mut failure: Option<(FailureKind, String)> = initial_failure;
        let mut writer_finished = false;

        loop {
            if cancel_requested.load(Ordering::SeqCst) && !interrupted {
                request_process_interrupt(
                    &mut protocol,
                    writer.as_ref(),
                    &mut child,
                    &mut interrupted,
                    &mut intended_kill,
                    &mut interrupt_deadline,
                );
            }
            while let Ok(control) = control_rx.try_recv() {
                match control {
                    ControlMessage::Permission { public_id, reply } => {
                        let Some((_, payload)) = local_pending.remove(&public_id) else {
                            continue;
                        };
                        let response = match protocol.permission_response(payload, &reply) {
                            Ok(response) => response,
                            Err(error) => {
                                failure = Some((FailureKind::Protocol, error));
                                break;
                            }
                        };
                        let stored = match sessions::append_event(
                            &queued.turn.id,
                            SessionEvent::PermissionResolved {
                                request_id: public_id.clone(),
                                decision: reply.decision.persisted().to_string(),
                            },
                        ) {
                            Ok(stored) => stored,
                            Err(error) => {
                                failure = Some((FailureKind::Internal, error));
                                break;
                            }
                        };
                        if let Some(writer) = writer.as_ref() {
                            if let Err(error) = queue_json(writer, &response.line) {
                                if response.interrupt {
                                    interrupted = true;
                                    intended_kill = true;
                                    terminate_process_group(&mut child, true);
                                } else {
                                    failure = Some((FailureKind::Input, error));
                                }
                            }
                        } else {
                            if response.interrupt {
                                interrupted = true;
                                intended_kill = true;
                                terminate_process_group(&mut child, true);
                            } else {
                                failure = Some((
                                    FailureKind::Input,
                                    "runner stdin closed before a permission response".to_string(),
                                ));
                            }
                        }
                        if response.interrupt && !intended_kill {
                            interrupted = true;
                            if let (Some(writer), Some(message)) =
                                (writer.as_ref(), protocol.interrupt_message())
                            {
                                if queue_json(writer, &message).is_err() {
                                    intended_kill = true;
                                    terminate_process_group(&mut child, true);
                                }
                            }
                            if !intended_kill {
                                interrupt_deadline = Some(Instant::now() + INTERRUPT_GRACE);
                            }
                        }
                        deliver(&queued.session.id, stored, channel);
                        if failure.is_some() {
                            break;
                        }
                    }
                    ControlMessage::Interrupt | ControlMessage::Shutdown => {
                        if !interrupted {
                            request_process_interrupt(
                                &mut protocol,
                                writer.as_ref(),
                                &mut child,
                                &mut interrupted,
                                &mut intended_kill,
                                &mut interrupt_deadline,
                            );
                        }
                    }
                }
            }

            if failure.is_some() {
                intended_kill = true;
                terminate_process_group(&mut child, true);
            }
            match writer_done_rx.try_recv() {
                Ok(Ok(())) => writer_finished = true,
                Ok(Err(error)) => {
                    writer_finished = true;
                    if failure.is_none()
                        && writer_failure_is_unexpected(
                            interrupted,
                            intended_kill,
                            terminal.is_some(),
                            process_status.is_some(),
                        )
                    {
                        failure = Some((FailureKind::Input, error));
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => writer_finished = true,
            }

            match output_rx.recv_timeout(PROCESS_POLL) {
                Ok(OutputMessage::Line(line)) if failure.is_none() => {
                    let actions = match protocol.handle_line(&line) {
                        Ok(actions) => actions,
                        Err(error) => {
                            failure = Some((FailureKind::Protocol, error));
                            Vec::new()
                        }
                    };
                    for action in actions {
                        if let Err(error) = self.apply_action(
                            queued,
                            channel,
                            control_tx,
                            &mut protocol,
                            &mut writer,
                            &mut local_pending,
                            &mut terminal,
                            action,
                        ) {
                            failure = Some(error);
                            break;
                        }
                    }
                    if terminal.is_some() && launch.kill_after_terminal {
                        intended_kill = true;
                        terminate_process_group(&mut child, true);
                    }
                }
                Ok(OutputMessage::Error(error)) => {
                    failure = Some((FailureKind::Protocol, error));
                }
                Ok(OutputMessage::Eof) => stdout_eof = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => stdout_eof = true,
                Ok(OutputMessage::Line(_)) => {}
            }

            if interrupt_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                intended_kill = true;
                terminate_process_group(&mut child, true);
                interrupt_deadline = None;
            }
            if process_status.is_none() {
                process_status = child.try_wait().map_err(|error| {
                    (
                        FailureKind::Internal,
                        format!("could not observe runner exit: {error}"),
                    )
                })?;
                if process_status.is_some() {
                    // The direct child may have left descendants holding its
                    // pipes. Close the whole isolated group, then consume the
                    // bytes already buffered in stdout before deciding whether
                    // a terminal frame was present.
                    terminate_process_group_id(process_group, true);
                    post_exit_deadline = Some(Instant::now() + POST_EXIT_DRAIN);
                }
            }
            if stdout_eof
                && process_status.is_none()
                && launch.kill_after_terminal
                && terminal.is_some()
            {
                intended_kill = true;
                terminate_process_group(&mut child, true);
            }
            if process_status.is_some()
                && (stdout_eof
                    || post_exit_deadline.is_some_and(|deadline| Instant::now() >= deadline))
            {
                break;
            }
        }

        drop(writer);
        terminate_process_group_id(process_group, true);
        if stdout_done_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            let _ = stdout_thread.join();
        }
        if stderr_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            let _ = stderr_thread.join();
        }
        if writer_finished || writer_done_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            let _ = writer_thread.join();
        }
        for public_id in local_pending.keys() {
            match sessions::append_event(
                &queued.turn.id,
                SessionEvent::PermissionResolved {
                    request_id: public_id.clone(),
                    decision: if interrupted {
                        "turn-interrupted".to_string()
                    } else {
                        "runner-ended".to_string()
                    },
                },
            ) {
                Ok(stored) => deliver(&queued.session.id, stored, channel),
                Err(error) if failure.is_none() => {
                    failure = Some((FailureKind::Internal, error));
                }
                Err(_) => {}
            }
            self.inner
                .pending
                .lock()
                .expect("pending request map poisoned")
                .remove(public_id);
        }

        if let Some(failure) = failure {
            return Err(failure);
        }
        if interrupted {
            return Ok(DriveOutcome::Interrupted);
        }
        let status = process_status.expect("process status checked above");
        if !status.success() && !intended_kill {
            return Ok(DriveOutcome::Failed(
                FailureKind::RunnerExit,
                match status.code() {
                    Some(code) => format!(
                        "{} exited with status {code}",
                        queued.session.runner.label()
                    ),
                    None => format!("{} was terminated", queued.session.runner.label()),
                },
                None,
            ));
        }
        match terminal {
            Some(ProtocolTerminal::Completed { duration_ms }) => {
                Ok(DriveOutcome::Completed(duration_ms))
            }
            Some(ProtocolTerminal::Failed {
                message,
                duration_ms,
            }) => Ok(DriveOutcome::Failed(
                FailureKind::Protocol,
                message,
                duration_ms,
            )),
            Some(ProtocolTerminal::Interrupted) => Ok(DriveOutcome::Interrupted),
            None => Ok(DriveOutcome::Failed(
                FailureKind::Protocol,
                "runner exited before reporting a terminal turn event".to_string(),
                None,
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_action(
        &self,
        queued: &QueuedTurn,
        channel: &Channel<EngineEvent>,
        control_tx: &mpsc::Sender<ControlMessage>,
        protocol: &mut Protocol,
        writer: &mut Option<mpsc::SyncSender<WriterMessage>>,
        local_pending: &mut HashMap<String, (String, PendingPayload)>,
        terminal: &mut Option<ProtocolTerminal>,
        action: ProtocolAction,
    ) -> Result<(), (FailureKind, String)> {
        match action {
            ProtocolAction::BindSession(runner_id) => {
                sessions::bind_runner_session_id(&queued.session.id, &runner_id)
                    .map_err(|error| (FailureKind::Protocol, error))?;
            }
            ProtocolAction::Event(event) => {
                let stored = sessions::append_event(&queued.turn.id, event)
                    .map_err(|error| (FailureKind::Internal, error))?;
                deliver(&queued.session.id, stored, channel);
            }
            ProtocolAction::Permission(prompt) => {
                let public_id = Uuid::new_v4().to_string();
                let stored = sessions::append_event(
                    &queued.turn.id,
                    SessionEvent::PermissionRequest {
                        request_id: public_id.clone(),
                        tool_name: sanitize::text(&prompt.tool_name),
                        summary: sanitize::text(&prompt.summary),
                        options: prompt.options.clone(),
                        prompt: prompt.prompt.clone(),
                        expires_with_turn: true,
                    },
                )
                .map_err(|error| (FailureKind::Internal, error))?;
                self.inner
                    .pending
                    .lock()
                    .expect("pending request map poisoned")
                    .insert(
                        public_id.clone(),
                        PendingRegistration {
                            turn_id: queued.turn.id.clone(),
                            control: control_tx.clone(),
                            options: prompt.options.clone(),
                        },
                    );
                local_pending.insert(public_id.clone(), (prompt.wire_key, prompt.payload));
                deliver(&queued.session.id, stored, channel);
            }
            ProtocolAction::CancelPermission(wire_key) => {
                if let Some(public_id) = local_pending.iter().find_map(|(public_id, (wire, _))| {
                    (wire == &wire_key).then(|| public_id.clone())
                }) {
                    local_pending.remove(&public_id);
                    self.inner
                        .pending
                        .lock()
                        .expect("pending request map poisoned")
                        .remove(&public_id);
                    let stored = sessions::append_event(
                        &queued.turn.id,
                        SessionEvent::PermissionResolved {
                            request_id: public_id,
                            decision: "cancelled-by-runner".to_string(),
                        },
                    )
                    .map_err(|error| (FailureKind::Internal, error))?;
                    deliver(&queued.session.id, stored, channel);
                }
            }
            ProtocolAction::Send(value) => {
                let writer = writer.as_ref().ok_or_else(|| {
                    (
                        FailureKind::Input,
                        "runner closed stdin during protocol negotiation".to_string(),
                    )
                })?;
                queue_json(writer, &value).map_err(|error| (FailureKind::Input, error))?;
            }
            ProtocolAction::Terminal(value) => *terminal = Some(value),
        }
        // Protocol state may learn IDs while processing an action; keeping the
        // mutable adapter in this method makes that ordering explicit.
        let _ = protocol;
        Ok(())
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SupervisorInner {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Ok(active) = self.active.get_mut() {
            for registration in active.values() {
                registration.cancel_requested.store(true, Ordering::SeqCst);
                let _ = registration.control.send(ControlMessage::Shutdown);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum SelectedSafety {
    Claude {
        option_id: String,
        mode: String,
    },
    CodexAppServer {
        option_id: String,
        sandbox: String,
        approval_policy: String,
    },
    CodexFallback {
        option_id: String,
    },
}

impl SelectedSafety {
    fn option_id(&self) -> &str {
        match self {
            Self::Claude { option_id, .. }
            | Self::CodexAppServer { option_id, .. }
            | Self::CodexFallback { option_id } => option_id,
        }
    }
}

impl Protocol {
    fn handle_line(&mut self, line: &str) -> Result<Vec<ProtocolAction>, String> {
        match self {
            Self::Claude(protocol) => protocol.handle_line(line),
            Self::Codex(protocol) => protocol.handle_line(line),
            Self::CodexFallback(protocol) => protocol.handle_line(line),
        }
    }

    fn permission_response(
        &mut self,
        payload: PendingPayload,
        reply: &PermissionReply,
    ) -> Result<PermissionWireResponse, String> {
        match self {
            Self::Claude(protocol) => protocol.permission_response(payload, reply),
            Self::Codex(protocol) => protocol.permission_response(payload, reply),
            Self::CodexFallback(_) => {
                Err("the read-only Codex fallback cannot receive approval requests".to_string())
            }
        }
    }

    fn interrupt_message(&mut self) -> Option<Value> {
        match self {
            Self::Codex(protocol) => protocol.interrupt_message(),
            Self::Claude(_) | Self::CodexFallback(_) => None,
        }
    }
}

fn protocol_and_launch(
    queued: &QueuedTurn,
    safety: SelectedSafety,
) -> Result<(Protocol, Launch), (FailureKind, String)> {
    match safety {
        SelectedSafety::Claude { mode, .. } => {
            let resume = sessions::runner_session_started(&queued.session.id)
                .map_err(|error| (FailureKind::Internal, error))?;
            let (protocol, launch) = claude::build(queued, &mode, resume)?;
            Ok((Protocol::Claude(protocol), launch))
        }
        SelectedSafety::CodexAppServer {
            sandbox,
            approval_policy,
            ..
        } => {
            let (protocol, launch) = codex::build_app_server(queued, &sandbox, &approval_policy)?;
            Ok((Protocol::Codex(protocol), launch))
        }
        SelectedSafety::CodexFallback { .. } => {
            let (protocol, launch) = codex::build_fallback(queued)?;
            Ok((Protocol::CodexFallback(protocol), launch))
        }
    }
}

fn deliver(session_id: &str, stored: StoredEvent, channel: &Channel<EngineEvent>) {
    if let Err(error) = channel.send(EngineEvent {
        session_id: session_id.to_string(),
        stored,
    }) {
        // The Chat view can legitimately detach while a durable turn keeps
        // running. Rehydration reads SQLite, so losing this transient delivery
        // must not rewrite a successfully persisted turn as failed.
        log::info!("chat event channel detached: {error}");
    }
}

pub(super) fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

pub(super) fn terminate_process_group(child: &mut std::process::Child, force: bool) {
    terminate_process_group_id(child.id(), force);
    if force {
        // Also target the direct child in case process-group creation was not
        // supported by the host platform.
        let _ = child.kill();
    }
}

fn terminate_process_group_id(process_group: u32, force: bool) {
    #[cfg(unix)]
    {
        let Ok(process_group) = i32::try_from(process_group) else {
            return;
        };
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: the child was placed in a fresh process group whose id is its
        // pid. A negative pid addresses that group and cannot target Aviary's.
        unsafe {
            libc::kill(-process_group, signal);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (process_group, force);
    }
}

fn queue_json(writer: &mpsc::SyncSender<WriterMessage>, value: &Value) -> Result<(), String> {
    let line = serde_json::to_string(value).map_err(|error| error.to_string())?;
    queue_line(writer, line)
}

fn queue_line(writer: &mpsc::SyncSender<WriterMessage>, line: String) -> Result<(), String> {
    if line.len() > MAX_PROTOCOL_LINE_BYTES {
        return Err(format!(
            "runner stdin frame exceeds {MAX_PROTOCOL_LINE_BYTES} bytes"
        ));
    }
    writer
        .try_send(WriterMessage::Line(line))
        .map_err(|error| format!("runner stdin queue is unavailable: {error}"))
}

fn write_runner_stdin(
    mut stdin: ChildStdin,
    receiver: mpsc::Receiver<WriterMessage>,
    done: mpsc::Sender<Result<(), String>>,
) {
    let result = loop {
        match receiver.recv() {
            Ok(WriterMessage::Line(line)) => {
                if let Err(error) = write_line(&mut stdin, &line) {
                    break Err(error);
                }
            }
            Ok(WriterMessage::Close) | Err(_) => break Ok(()),
        }
    };
    drop(stdin);
    let _ = done.send(result);
}

fn write_line(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    stdin
        .write_all(line.as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("could not write runner stdin: {error}"))
}

fn read_protocol_lines<R: Read>(mut reader: R, sender: mpsc::SyncSender<OutputMessage>) {
    let mut chunk = [0_u8; 8192];
    let mut line = Vec::new();
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => {
                if !line.is_empty() {
                    send_line(&sender, &line);
                }
                let _ = sender.send(OutputMessage::Eof);
                return;
            }
            Ok(read) => {
                for byte in &chunk[..read] {
                    if *byte == b'\n' {
                        if !send_line(&sender, &line) {
                            return;
                        }
                        line.clear();
                    } else {
                        line.push(*byte);
                        if line.len() > MAX_PROTOCOL_LINE_BYTES {
                            let _ = sender.send(OutputMessage::Error(format!(
                                "runner protocol line exceeds {MAX_PROTOCOL_LINE_BYTES} bytes"
                            )));
                            return;
                        }
                    }
                }
            }
            Err(error) => {
                let _ = sender.send(OutputMessage::Error(format!(
                    "could not read runner stdout: {error}"
                )));
                return;
            }
        }
    }
}

fn send_line(sender: &mpsc::SyncSender<OutputMessage>, bytes: &[u8]) -> bool {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(line) if line.trim().is_empty() => true,
        Ok(line) => sender.send(OutputMessage::Line(line.to_string())).is_ok(),
        Err(_) => sender
            .send(OutputMessage::Error(
                "runner emitted non-UTF-8 protocol output".to_string(),
            ))
            .is_ok(),
    }
}

fn drain<R: Read>(mut reader: R) -> u64 {
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return total,
            Ok(read) => total = total.saturating_add(read as u64),
        }
    }
}

pub fn canonical_cwd(input: Option<&str>) -> Result<PathBuf, String> {
    let path = match input {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "working directory {} is unavailable: {error}",
            path.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "working directory {} is not a directory",
            canonical.display()
        ));
    }
    Ok(canonical)
}

pub fn list_sessions(limit: usize) -> Result<Vec<SessionSummary>, String> {
    sessions::list_sessions(limit.min(500))
}

pub fn load_session(session_id: &str) -> Result<Option<SessionDetail>, String> {
    sessions::load_session(session_id)
}

fn find_turn(turn_id: &str) -> Result<ChatTurn, String> {
    for summary in sessions::list_sessions(500)? {
        if let Some(detail) = sessions::load_session(&summary.session.id)? {
            if let Some(turn) = detail
                .turns
                .into_iter()
                .find(|entry| entry.turn.id == turn_id)
            {
                return Ok(turn.turn);
            }
        }
    }
    Err("chat turn not found".to_string())
}

fn title_from_prompt(prompt: &str) -> String {
    let first = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("New chat");
    let title = sanitize::text(first.trim());
    truncate_chars(&title, 80)
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn request_process_interrupt(
    protocol: &mut Protocol,
    writer: Option<&mpsc::SyncSender<WriterMessage>>,
    child: &mut std::process::Child,
    interrupted: &mut bool,
    intended_kill: &mut bool,
    interrupt_deadline: &mut Option<Instant>,
) {
    *interrupted = true;
    if let (Some(writer), Some(message)) = (writer, protocol.interrupt_message()) {
        if queue_json(writer, &message).is_ok() {
            *interrupt_deadline = Some(Instant::now() + INTERRUPT_GRACE);
            return;
        }
    }
    *intended_kill = true;
    terminate_process_group(child, true);
}

fn writer_failure_is_unexpected(
    interrupted: bool,
    intended_kill: bool,
    has_terminal: bool,
    process_exited: bool,
) -> bool {
    !interrupted && !intended_kill && !has_terminal && !process_exited
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn protocol_reader_bounds_lines_and_preserves_unicode() {
        let (tx, rx) = mpsc::sync_channel(4);
        read_protocol_lines(Cursor::new("{\"text\":\"🪶\"}\n"), tx);
        match rx.recv().unwrap() {
            OutputMessage::Line(line) => assert_eq!(line, "{\"text\":\"🪶\"}"),
            other => panic!("unexpected output: {other:?}"),
        }

        let (tx, rx) = mpsc::sync_channel(4);
        read_protocol_lines(Cursor::new(vec![b'x'; MAX_PROTOCOL_LINE_BYTES + 1]), tx);
        assert!(matches!(rx.recv().unwrap(), OutputMessage::Error(_)));
    }

    #[test]
    fn stderr_drain_is_bounded_memory_and_complete() {
        let bytes = vec![b'e'; 2 * 1024 * 1024];
        assert_eq!(drain(Cursor::new(bytes)), 2 * 1024 * 1024);
    }

    #[test]
    fn canonical_cwd_rejects_files() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(canonical_cwd(Some(file.path().to_str().unwrap())).is_err());
    }

    #[test]
    fn expected_writer_errors_do_not_replace_terminal_outcomes() {
        assert!(writer_failure_is_unexpected(false, false, false, false));
        assert!(!writer_failure_is_unexpected(true, false, false, false));
        assert!(!writer_failure_is_unexpected(false, true, false, false));
        assert!(!writer_failure_is_unexpected(false, false, true, false));
        assert!(!writer_failure_is_unexpected(false, false, false, true));
    }

    #[test]
    fn late_permission_response_is_rejected_without_blocking() {
        let supervisor = Supervisor::new();
        let (tx, rx) = mpsc::channel();
        supervisor.inner.pending.lock().unwrap().insert(
            "request".to_string(),
            PendingRegistration {
                turn_id: "turn".to_string(),
                control: tx,
                options: vec!["deny".to_string()],
            },
        );
        let reply = PermissionReply {
            decision: PermissionDecision::Deny,
            updated_input: None,
            message: None,
            answers: None,
            content: None,
        };
        supervisor
            .respond_permission("request", reply.clone())
            .unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            ControlMessage::Permission { public_id, .. } if public_id == "request"
        ));
        assert!(supervisor.respond_permission("request", reply).is_err());
    }

    #[test]
    fn immediate_interrupt_is_registered_before_runner_initialization() {
        let supervisor = Supervisor::new();
        let registration = supervisor.register_turn("turn-before-init").unwrap();

        assert_eq!(
            supervisor.signal_active("turn-before-init", ControlMessage::Interrupt),
            ActiveSignal::Delivered
        );
        assert!(registration.cancel_requested.load(Ordering::SeqCst));
        assert!(matches!(
            registration.receiver.try_recv(),
            Ok(ControlMessage::Interrupt)
        ));
        assert!(supervisor.claim_completion("turn-before-init", registration.id));
    }

    #[test]
    fn active_registration_cleanup_cannot_remove_a_new_worker() {
        let supervisor = Supervisor::new();
        let first = supervisor.register_turn("reused-key").unwrap();
        assert!(!supervisor.claim_completion("reused-key", first.id));

        let second = supervisor.register_turn("reused-key").unwrap();
        // Cleanup arriving from the old worker is ignored because its
        // registration generation no longer owns this turn key.
        assert!(!supervisor.claim_completion("reused-key", first.id));
        assert_eq!(
            supervisor.signal_active("reused-key", ControlMessage::Interrupt),
            ActiveSignal::Delivered
        );
        assert!(matches!(
            second.receiver.try_recv(),
            Ok(ControlMessage::Interrupt)
        ));
        assert!(supervisor.claim_completion("reused-key", second.id));
    }

    #[test]
    fn completion_claim_closes_the_interrupt_acknowledgement_window() {
        let supervisor = Supervisor::new();
        let registration = supervisor.register_turn("finishing").unwrap();
        assert!(!supervisor.claim_completion("finishing", registration.id));
        assert_eq!(
            supervisor.signal_active("finishing", ControlMessage::Interrupt),
            ActiveSignal::Missing
        );
    }

    #[cfg(unix)]
    #[test]
    fn fake_child_drains_stderr_and_kills_descendants_holding_pipes() {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(
            r#"IFS= read -r prompt
[ "$prompt" = "private prompt" ] || exit 9
i=0
while [ "$i" -lt 20000 ]; do
  printf 'fixture stderr line\n' >&2
  i=$((i + 1))
done
printf '{"terminal":true}\n'
(sleep 30) &
exit 7"#,
        );
        configure_process_group(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let process_group = child.id();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (output_tx, output_rx) = mpsc::sync_channel(8);
        let (stdout_done_tx, stdout_done_rx) = mpsc::channel();
        let stdout_thread = thread::spawn(move || {
            read_protocol_lines(stdout, output_tx);
            let _ = stdout_done_tx.send(());
        });
        let (stderr_tx, stderr_rx) = mpsc::channel();
        let stderr_thread = thread::spawn(move || {
            let _ = stderr_tx.send(drain(stderr));
        });
        writeln!(stdin, "private prompt").unwrap();
        drop(stdin);

        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(7));
        terminate_process_group_id(process_group, true);

        let mut saw_terminal = false;
        while let Ok(message) = output_rx.recv_timeout(Duration::from_secs(1)) {
            match message {
                OutputMessage::Line(line) => saw_terminal |= line == r#"{"terminal":true}"#,
                OutputMessage::Eof => break,
                OutputMessage::Error(error) => panic!("{error}"),
            }
        }
        assert!(saw_terminal);
        assert!(stderr_rx.recv_timeout(Duration::from_secs(1)).unwrap() > 300_000);
        stdout_done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        stdout_thread.join().unwrap();
        stderr_thread.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn terminal_frame_ends_a_child_waiting_for_more_stdin() {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(
            r#"IFS= read -r prompt
printf '{"type":"result","is_error":false}\n'
while IFS= read -r more; do :; done"#,
        );
        configure_process_group(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let process_group = child.id();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (writer_tx, writer_rx) = mpsc::sync_channel(4);
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let writer_thread =
            thread::spawn(move || write_runner_stdin(stdin, writer_rx, writer_done_tx));
        queue_line(&writer_tx, "prompt".to_string()).unwrap();

        let mut reader = std::io::BufReader::new(stdout);
        let mut terminal = String::new();
        std::io::BufRead::read_line(&mut reader, &mut terminal).unwrap();
        assert!(terminal.contains("\"type\":\"result\""));
        terminate_process_group(&mut child, true);
        let status = child.wait().unwrap();
        assert!(!status.success());
        drop(writer_tx);
        assert!(writer_done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        writer_thread.join().unwrap();
        terminate_process_group_id(process_group, true);
    }
}
