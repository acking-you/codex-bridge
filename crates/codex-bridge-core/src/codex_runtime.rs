//! Thin stdio runtime wrapper for `codex app-server`.

use std::{
    collections::{HashMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use codex_app_server_protocol::{
    ApprovalsReviewer, AskForApproval, ClientInfo, ClientNotification, ClientRequest,
    CommandExecutionApprovalDecision, CommandExecutionRequestApprovalParams,
    CommandExecutionRequestApprovalResponse, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, FileChangeRequestApprovalResponse, InitializeCapabilities,
    InitializeParams, InitializeResponse, JSONRPCMessage, JSONRPCNotification, JSONRPCRequest,
    JSONRPCResponse, ReadOnlyAccess, RequestId, SandboxMode, SandboxPolicy, ServerNotification,
    ServerRequest, ThreadCompactStartParams, ThreadCompactStartResponse, ThreadItem,
    ThreadResumeParams, ThreadResumeResponse, ThreadStartParams, ThreadStartResponse, Turn,
    TurnInterruptParams, TurnStartParams, TurnStartResponse, TurnStatus, UserInput,
};
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot, Mutex},
    task::JoinHandle,
};
use tracing::{info, warn};

use crate::{
    approval_guard::{ApprovalDecision, ApprovalGuard},
    lane_manager::RuntimeSlotSnapshot,
    system_prompt::load_persona,
};

const DEFAULT_CLIENT_NAME: &str = "codex-bridge";

/// Runtime-local configuration for launching and routing through a local
/// app-server.
#[derive(Debug, Clone)]
pub struct CodexRuntimeConfig {
    /// Path to the Codex repository root containing the app-server workspace
    /// `Cargo.toml`.
    pub codex_repo_root: PathBuf,
    /// Workspace directory used for thread and turn execution.
    pub workspace_root: PathBuf,
    /// Runtime-owned prompt file injected into threads at use time.
    pub prompt_file: PathBuf,
    /// Isolated HOME directory used to hide user-global repo skills.
    pub child_home_root: PathBuf,
    /// Isolated CODEX_HOME directory used for app-server state and system
    /// skills.
    pub codex_home_root: PathBuf,
    /// Client name reported during initialize.
    pub client_name: String,
    /// Client version reported during initialize.
    pub client_version: String,
    /// Bridge admin's QQ identifier. Consumed by
    /// [`crate::system_prompt::render_admin_block`] to tell Codex which
    /// id is 主人 so the persona register kicks in even when the
    /// inbound `[主人]` marker is not present. Zero or negative means
    /// no admin is configured.
    pub admin_user_id: i64,
    /// Directory holding per-conversation reply-context files. The
    /// per-thread `developer_instructions` derive the absolute path of
    /// this thread's context file from this directory so Codex never
    /// has to race on the singleton mirror when multiple conversations
    /// are active concurrently.
    pub reply_contexts_dir: PathBuf,
    /// Shared, hot-reloadable "Available model capabilities" section
    /// appended to the loaded system prompt at `thread/start` /
    /// `thread/resume` time. The same `Arc` is held by
    /// [`crate::service::ServiceState`] so a reload via
    /// [`crate::service::ServiceState::reload_capabilities`] is visible
    /// on the very next prompt build — no runtime restart required.
    /// Existing running threads keep their embedded developer
    /// instructions until Codex next resumes them.
    pub capabilities_block: std::sync::Arc<std::sync::RwLock<Option<String>>>,
}

impl CodexRuntimeConfig {
    /// Build runtime configuration from repository and workspace roots.
    pub fn new(
        codex_repo_root: impl Into<PathBuf>,
        workspace_root: impl Into<PathBuf>,
        prompt_file: impl Into<PathBuf>,
        child_home_root: impl Into<PathBuf>,
        codex_home_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            codex_repo_root: codex_repo_root.into(),
            workspace_root: workspace_root.into(),
            prompt_file: prompt_file.into(),
            child_home_root: child_home_root.into(),
            codex_home_root: codex_home_root.into(),
            client_name: DEFAULT_CLIENT_NAME.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            admin_user_id: 0,
            reply_contexts_dir: PathBuf::new(),
            capabilities_block: std::sync::Arc::new(std::sync::RwLock::new(None)),
        }
    }
}

#[derive(Debug)]
struct RuntimeWriteState {
    stdin: BufWriter<ChildStdin>,
}

/// Outcome delivered from the demuxer to a waiting request.
#[derive(Debug)]
enum ResponseOutcome {
    Ok(Value),
    Err(String),
}

/// Shared demuxer state used by the background reader task and by request
/// senders to route JSON-RPC responses and turn notifications to their
/// respective waiters.
#[derive(Debug, Default)]
struct Demuxer {
    /// Pending request-response handshakes, keyed by request id.
    pending_responses: Mutex<HashMap<RequestId, oneshot::Sender<ResponseOutcome>>>,
    /// Active per-thread notification senders held by the reader task.
    thread_senders: Mutex<HashMap<String, mpsc::Sender<ServerNotification>>>,
    /// Per-thread notification receivers, parked for consumption by
    /// `wait_for_turn_*`. A fresh pair is created for each `start_turn`.
    thread_receivers: Mutex<HashMap<String, mpsc::Receiver<ServerNotification>>>,
}

#[derive(Debug)]
struct ChildGuard {
    child: StdMutex<Option<Child>>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self {
            child: StdMutex::new(Some(child)),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
        }
    }
}

/// Final outcome returned from a codex turn.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurnResult {
    /// Thread id used by the runtime.
    pub thread_id: String,
    /// Turn id returned by `turn/start`.
    pub turn_id: String,
    /// Terminal turn status reported by the app-server.
    pub status: TurnStatus,
    /// Terminal error message when the turn fails.
    pub error_message: Option<String>,
    /// Raw completed items emitted by the runtime for this turn.
    pub items: Vec<Value>,
    /// Last assistant/agent text message, if one exists.
    pub final_reply: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct TurnCompletion {
    turn: Turn,
    items: Vec<Value>,
}

/// Active turn identity returned immediately after `turn/start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTurn {
    /// Thread id that owns this turn.
    pub thread_id: String,
    /// Active turn id allocated by the app-server.
    pub turn_id: String,
}

/// Sink used to publish recent human-readable turn progress while a task is
/// still running.
#[async_trait]
pub trait TurnProgressSink: Send + Sync {
    /// Replace the current recent-output view for the active task.
    async fn update_recent_output(&self, recent_output: Vec<String>) -> Result<()>;

    /// Persist one completed output entry for later task-status queries.
    async fn commit_output(&self, text: String) -> Result<()>;

    /// Deliver one completed assistant/agent text message for the active task.
    async fn commit_agent_message(&self, _text: String) -> Result<()> {
        Ok(())
    }
}

/// Minimal interface for codex execution runtimes.
#[async_trait]
pub trait CodexExecutor: Send + Sync {
    /// Ensure a thread is available and return its id.
    async fn ensure_thread(
        &self,
        conversation_key: &str,
        existing_thread_id: Option<&str>,
    ) -> Result<String>;

    /// Start a turn and return the active turn identity immediately.
    async fn start_turn(&self, thread_id: &str, input_text: &str) -> Result<ActiveTurn>;

    /// Wait until an active turn reaches a terminal state.
    async fn wait_for_turn(&self, active_turn: &ActiveTurn) -> Result<CodexTurnResult>;

    /// Wait until an active turn reaches a terminal state while streaming
    /// recent human-readable progress into the provided sink.
    async fn wait_for_turn_with_progress(
        &self,
        active_turn: &ActiveTurn,
        progress: Option<&dyn TurnProgressSink>,
    ) -> Result<CodexTurnResult> {
        let _ = progress;
        self.wait_for_turn(active_turn).await
    }

    /// Run a turn and return a summary result.
    async fn run_turn(&self, thread_id: &str, input_text: &str) -> Result<CodexTurnResult> {
        let active_turn = self.start_turn(thread_id, input_text).await?;
        self.wait_for_turn(&active_turn).await
    }

    /// Interrupt a running turn when supported.
    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()>;

    /// Start compaction for one existing thread.
    async fn compact_thread(&self, thread_id: &str) -> Result<()>;

    /// Return a point-in-time snapshot of runtime slot occupancy.
    async fn runtime_slots(&self) -> Vec<RuntimeSlotSnapshot> {
        Vec::new()
    }
}

const RECENT_OUTPUT_LIMIT: usize = 4;
const RECENT_OUTPUT_MAX_CHARS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveOutputKind {
    AgentMessage,
    Plan,
    CommandExecution,
    FileChange,
}

#[derive(Debug, Clone)]
struct LiveOutputEntry {
    kind: LiveOutputKind,
    text: String,
    order: u64,
}

#[derive(Debug, Clone)]
struct CommittedLiveOutput {
    rendered: String,
    agent_message: Option<String>,
}

#[derive(Debug, Default)]
struct RecentOutputTracker {
    active: HashMap<String, LiveOutputEntry>,
    committed: VecDeque<LiveOutputEntry>,
    last_recent_output: Vec<String>,
    next_order: u64,
}

impl RecentOutputTracker {
    fn push_delta(
        &mut self,
        item_id: &str,
        kind: LiveOutputKind,
        delta: &str,
    ) -> Option<Vec<String>> {
        if delta.is_empty() {
            return None;
        }

        let order = self.bump_order();
        let entry = self
            .active
            .entry(item_id.to_string())
            .or_insert_with(|| LiveOutputEntry {
                kind,
                text: String::new(),
                order,
            });
        if entry.kind != kind {
            entry.kind = kind;
            entry.text.clear();
        }
        entry.text.push_str(delta);
        entry.order = order;
        self.recent_output_if_changed()
    }

    fn commit_item(
        &mut self,
        item: &ThreadItem,
    ) -> (Option<Vec<String>>, Option<CommittedLiveOutput>) {
        let (item_id, kind, explicit_text) = match item {
            ThreadItem::AgentMessage {
                id,
                text,
                ..
            } => (id.as_str(), LiveOutputKind::AgentMessage, Some(text.clone())),
            ThreadItem::Plan {
                id,
                text,
            } => (id.as_str(), LiveOutputKind::Plan, Some(text.clone())),
            ThreadItem::CommandExecution {
                id, ..
            } => (id.as_str(), LiveOutputKind::CommandExecution, None),
            ThreadItem::FileChange {
                id, ..
            } => (id.as_str(), LiveOutputKind::FileChange, None),
            _ => return (None, None),
        };

        let active = self.active.remove(item_id);
        let text = explicit_text.or_else(|| active.as_ref().map(|entry| entry.text.clone()));
        let Some(raw_text) = text else {
            return (self.recent_output_if_changed(), None);
        };
        let Some(recent_text) = normalize_output_text(&raw_text) else {
            return (self.recent_output_if_changed(), None);
        };

        let kind = active.map(|entry| entry.kind).unwrap_or(kind);
        let order = self.bump_order();
        self.committed.push_back(LiveOutputEntry {
            kind,
            text: recent_text.clone(),
            order,
        });
        while self.committed.len() > RECENT_OUTPUT_LIMIT {
            let _ = self.committed.pop_front();
        }

        let agent_message = if kind == LiveOutputKind::AgentMessage {
            normalize_agent_reply_text(&raw_text)
        } else {
            None
        };
        (
            self.recent_output_if_changed(),
            Some(CommittedLiveOutput {
                rendered: render_output_entry(kind, &recent_text),
                agent_message,
            }),
        )
    }

    fn recent_output_if_changed(&mut self) -> Option<Vec<String>> {
        let recent_output = self.render_recent_output();
        if recent_output == self.last_recent_output {
            return None;
        }
        self.last_recent_output = recent_output.clone();
        Some(recent_output)
    }

    fn render_recent_output(&self) -> Vec<String> {
        let mut entries = self
            .committed
            .iter()
            .cloned()
            .chain(self.active.values().cloned())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.order);
        let start = entries.len().saturating_sub(RECENT_OUTPUT_LIMIT);
        entries[start..]
            .iter()
            .filter_map(|entry| {
                normalize_output_text(&entry.text)
                    .map(|text| render_output_entry(entry.kind, &text))
            })
            .collect()
    }

    fn bump_order(&mut self) -> u64 {
        self.next_order = self.next_order.saturating_add(1);
        self.next_order
    }
}

/// Concrete runtime implementation backed by a child `codex-app-server`
/// process.
#[derive(Debug)]
pub struct CodexRuntime {
    _child: ChildGuard,
    _reader_task: JoinHandle<()>,
    _approval_task: JoinHandle<()>,
    demuxer: Arc<Demuxer>,
    write_state: Arc<Mutex<RuntimeWriteState>>,
    config: CodexRuntimeConfig,
    guard: Arc<ApprovalGuard>,
    next_request_id: Mutex<u64>,
}

impl CodexRuntime {
    /// Create and initialize a runtime connected to a local app-server process.
    pub async fn new(config: CodexRuntimeConfig) -> Result<Self> {
        let (child, write_state, stdout) = spawn_protocol_state(&config).await?;
        let demuxer = Arc::new(Demuxer::default());
        let guard = Arc::new(ApprovalGuard::new(&config.workspace_root));
        let write_state = Arc::new(Mutex::new(write_state));

        let (server_request_tx, server_request_rx) = mpsc::unbounded_channel();
        let reader_task = tokio::spawn(reader_loop(stdout, demuxer.clone(), server_request_tx));
        let approval_task =
            tokio::spawn(approval_loop(server_request_rx, guard.clone(), write_state.clone()));

        let runtime = Self {
            _child: ChildGuard::new(child),
            _reader_task: reader_task,
            _approval_task: approval_task,
            demuxer,
            write_state,
            guard,
            config,
            next_request_id: Mutex::new(1),
        };
        runtime.initialize().await?;
        Ok(runtime)
    }

    /// Replace the command guard used for approval requests.
    pub fn with_guard(mut self, guard: ApprovalGuard) -> Self {
        self.guard = Arc::new(guard);
        self
    }

    async fn initialize(&self) -> Result<()> {
        info!(
            codex_repo_root = %self.config.codex_repo_root.display(),
            workspace_root = %self.config.workspace_root.display(),
            "starting codex app-server runtime"
        );
        let request_id = self.next_request_id().await;
        let request = ClientRequest::Initialize {
            request_id: request_id.clone(),
            params: InitializeParams {
                client_info: ClientInfo {
                    name: self.config.client_name.clone(),
                    title: Some("Codex Bridge".to_string()),
                    version: self.config.client_version.clone(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: true,
                    opt_out_notification_methods: None,
                }),
            },
        };
        let _: InitializeResponse = self
            .send_request(request, request_id, "initialize")
            .await
            .context("initialize codex app-server")?;
        self.send_notification(ClientNotification::Initialized)
            .await
            .context("send initialized notification")?;
        info!("codex app-server initialized");
        Ok(())
    }

    async fn send_notification(&self, notification: ClientNotification) -> Result<()> {
        let value = serde_json::to_value(notification).context("serialize client notification")?;
        let notification: JSONRPCNotification =
            serde_json::from_value(value).context("convert client notification to json-rpc")?;
        self.write_message(notification).await
    }

    async fn send_request<T>(
        &self,
        request: ClientRequest,
        request_id: RequestId,
        method: &str,
    ) -> Result<T>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        info!(method, "codex request started");
        let request = client_request_to_jsonrpc(request)?;
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.demuxer.pending_responses.lock().await;
            pending.insert(request_id.clone(), tx);
        }
        if let Err(error) = self.write_message(request).await {
            // Write failed before the reader could produce any response for
            // this id; drop the slot so subsequent ids are clean.
            self.demuxer
                .pending_responses
                .lock()
                .await
                .remove(&request_id);
            return Err(error).with_context(|| format!("write {method} request"));
        }

        match rx.await {
            Ok(ResponseOutcome::Ok(result)) => {
                info!(method, "codex request completed");
                serde_json::from_value(result)
                    .with_context(|| format!("decode {method} response payload"))
            },
            Ok(ResponseOutcome::Err(message)) => {
                info!(method, error = %message, "codex request failed");
                Err(anyhow!("{method} failed: {message}"))
            },
            Err(_) => {
                warn!(method, "codex runtime shut down before response arrived");
                Err(anyhow!("{method} failed: codex runtime shut down before response arrived"))
            },
        }
    }

    async fn wait_for_turn_completion(
        &self,
        mut notifications: mpsc::Receiver<ServerNotification>,
        thread_id: &str,
        turn_id: &str,
        progress: Option<&dyn TurnProgressSink>,
    ) -> Result<TurnCompletion> {
        let mut items = Vec::new();
        let mut tracker = progress.map(|_| RecentOutputTracker::default());
        loop {
            let Some(notification) = notifications.recv().await else {
                return Err(anyhow!(
                    "codex notification channel for thread {thread_id} closed before turn \
                     {turn_id} completed"
                ));
            };
            log_server_notification(&notification);

            match notification {
                ServerNotification::AgentMessageDelta(payload) => {
                    if let (Some(progress), Some(tracker)) = (progress, tracker.as_mut()) {
                        if let Some(recent_output) = tracker.push_delta(
                            &payload.item_id,
                            LiveOutputKind::AgentMessage,
                            &payload.delta,
                        ) {
                            progress.update_recent_output(recent_output).await?;
                        }
                    }
                },
                ServerNotification::PlanDelta(payload) => {
                    if let (Some(progress), Some(tracker)) = (progress, tracker.as_mut()) {
                        if let Some(recent_output) = tracker.push_delta(
                            &payload.item_id,
                            LiveOutputKind::Plan,
                            &payload.delta,
                        ) {
                            progress.update_recent_output(recent_output).await?;
                        }
                    }
                },
                ServerNotification::CommandExecutionOutputDelta(payload) => {
                    if let (Some(progress), Some(tracker)) = (progress, tracker.as_mut()) {
                        if let Some(recent_output) = tracker.push_delta(
                            &payload.item_id,
                            LiveOutputKind::CommandExecution,
                            &payload.delta,
                        ) {
                            progress.update_recent_output(recent_output).await?;
                        }
                    }
                },
                ServerNotification::FileChangeOutputDelta(payload) => {
                    if let (Some(progress), Some(tracker)) = (progress, tracker.as_mut()) {
                        if let Some(recent_output) = tracker.push_delta(
                            &payload.item_id,
                            LiveOutputKind::FileChange,
                            &payload.delta,
                        ) {
                            progress.update_recent_output(recent_output).await?;
                        }
                    }
                },
                ServerNotification::ItemCompleted(payload) => {
                    if let (Some(progress), Some(tracker)) = (progress, tracker.as_mut()) {
                        let (recent_output, committed_output) = tracker.commit_item(&payload.item);
                        if let Some(recent_output) = recent_output {
                            progress.update_recent_output(recent_output).await?;
                        }
                        if let Some(committed_output) = committed_output {
                            progress.commit_output(committed_output.rendered).await?;
                            if let Some(agent_message) = committed_output.agent_message {
                                progress.commit_agent_message(agent_message).await?;
                            }
                        }
                    }
                    items.push(
                        serde_json::to_value(payload.item).context("serialize completed item")?,
                    );
                },
                ServerNotification::TurnCompleted(payload)
                    if payload.thread_id == thread_id && payload.turn.id == turn_id =>
                {
                    return Ok(TurnCompletion {
                        turn: payload.turn,
                        items,
                    });
                },
                _ => {},
            }
        }
    }

    async fn next_request_id(&self) -> RequestId {
        let mut next = self.next_request_id.lock().await;
        let id = *next;
        *next += 1;
        RequestId::String(format!("codex-bridge-{id}"))
    }

    async fn register_thread_channel(&self, thread_id: &str) {
        let (tx, rx) = mpsc::channel::<ServerNotification>(256);
        let mut senders = self.demuxer.thread_senders.lock().await;
        let mut receivers = self.demuxer.thread_receivers.lock().await;
        senders.insert(thread_id.to_string(), tx);
        receivers.insert(thread_id.to_string(), rx);
    }

    async fn drop_thread_channel(&self, thread_id: &str) {
        self.demuxer.thread_senders.lock().await.remove(thread_id);
        self.demuxer.thread_receivers.lock().await.remove(thread_id);
    }

    async fn write_message<T>(&self, message: T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let mut state = self.write_state.lock().await;
        state.write_message(message).await
    }
}

/// Background reader task: demultiplexes JSON-RPC responses into pending
/// `send_request` waiters and server notifications into per-thread channels.
/// Server requests (approval prompts) are forwarded to the approval task.
async fn reader_loop(
    mut stdout: BufReader<ChildStdout>,
    demuxer: Arc<Demuxer>,
    server_request_tx: mpsc::UnboundedSender<JSONRPCRequest>,
) {
    loop {
        let message = match read_jsonrpc_message(&mut stdout).await {
            Ok(message) => message,
            Err(error) => {
                warn!(%error, "codex reader task exiting");
                break;
            },
        };
        match message {
            JSONRPCMessage::Response(JSONRPCResponse {
                id,
                result,
            }) => {
                let waiter = demuxer.pending_responses.lock().await.remove(&id);
                if let Some(tx) = waiter {
                    let _ = tx.send(ResponseOutcome::Ok(result));
                }
            },
            JSONRPCMessage::Error(err) => {
                let waiter = demuxer.pending_responses.lock().await.remove(&err.id);
                if let Some(tx) = waiter {
                    let _ = tx.send(ResponseOutcome::Err(err.error.message));
                }
            },
            JSONRPCMessage::Notification(raw) => {
                let notification = match ServerNotification::try_from(raw) {
                    Ok(notification) => notification,
                    Err(_) => continue,
                };
                let thread_id = match notification_thread_id(&notification) {
                    Some(id) => id,
                    None => {
                        log_server_notification(&notification);
                        continue;
                    },
                };
                let sender = demuxer.thread_senders.lock().await.get(&thread_id).cloned();
                match sender {
                    Some(tx) => {
                        if let Err(error) = tx.send(notification).await {
                            warn!(thread_id = %thread_id, %error, "dropping notification for thread without active receiver");
                        }
                    },
                    None => {
                        // No active turn listening on this thread; drop.
                    },
                }
            },
            JSONRPCMessage::Request(raw) => {
                if server_request_tx.send(raw).is_err() {
                    warn!("approval task gone; ignoring server request");
                }
            },
        }
    }

    // Reader is exiting. Drop all pending waiters so they see an error.
    demuxer.pending_responses.lock().await.clear();
    demuxer.thread_senders.lock().await.clear();
    demuxer.thread_receivers.lock().await.clear();
}

/// Background approval task: serializes access to the runtime's write half
/// for server-originated approval requests, keeping them off the hot request
/// path.
async fn approval_loop(
    mut rx: mpsc::UnboundedReceiver<JSONRPCRequest>,
    guard: Arc<ApprovalGuard>,
    write_state: Arc<Mutex<RuntimeWriteState>>,
) {
    while let Some(raw) = rx.recv().await {
        let request = match ServerRequest::try_from(raw) {
            Ok(request) => request,
            Err(error) => {
                warn!(%error, "failed to decode codex server request");
                continue;
            },
        };
        log_server_request(&request);
        match request {
            ServerRequest::CommandExecutionRequestApproval {
                request_id,
                params,
            } => {
                let response = build_command_approval_response(&guard, &params);
                info!(
                    decision = ?response.decision,
                    command = ?params.command,
                    "command approval resolved"
                );
                let mut state = write_state.lock().await;
                if let Err(error) = state.write_response(request_id, response).await {
                    warn!(%error, "failed to write command approval response");
                }
            },
            ServerRequest::FileChangeRequestApproval {
                request_id,
                params,
            } => {
                let response = build_file_change_approval_response(&guard, &params);
                info!(
                    decision = ?response.decision,
                    grant_root = ?params.grant_root,
                    "file-change approval resolved"
                );
                let mut state = write_state.lock().await;
                if let Err(error) = state.write_response(request_id, response).await {
                    warn!(%error, "failed to write file-change approval response");
                }
            },
            other => {
                warn!(?other, "ignoring unsupported server request");
            },
        }
    }
}

async fn read_jsonrpc_message(stdout: &mut BufReader<ChildStdout>) -> Result<JSONRPCMessage> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = stdout.read_line(&mut line).await?;
        if bytes == 0 {
            return Err(anyhow!("codex app-server stdout closed"));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let payload: Value = serde_json::from_str(trimmed).context("decode json-rpc payload")?;
        return serde_json::from_value(payload).context("decode json-rpc message");
    }
}

/// Extract the thread id tagged onto a server notification, if the variant
/// carries one.
fn notification_thread_id(notification: &ServerNotification) -> Option<String> {
    match notification {
        ServerNotification::TurnStarted(payload) => Some(payload.thread_id.clone()),
        ServerNotification::TurnCompleted(payload) => Some(payload.thread_id.clone()),
        ServerNotification::ItemStarted(payload) => Some(payload.thread_id.clone()),
        ServerNotification::ItemCompleted(payload) => Some(payload.thread_id.clone()),
        ServerNotification::RawResponseItemCompleted(payload) => Some(payload.thread_id.clone()),
        ServerNotification::AgentMessageDelta(payload) => Some(payload.thread_id.clone()),
        ServerNotification::PlanDelta(payload) => Some(payload.thread_id.clone()),
        ServerNotification::CommandExecutionOutputDelta(payload) => Some(payload.thread_id.clone()),
        ServerNotification::FileChangeOutputDelta(payload) => Some(payload.thread_id.clone()),
        ServerNotification::ReasoningSummaryTextDelta(payload) => Some(payload.thread_id.clone()),
        ServerNotification::ReasoningTextDelta(payload) => Some(payload.thread_id.clone()),
        ServerNotification::TurnPlanUpdated(payload) => Some(payload.thread_id.clone()),
        ServerNotification::HookCompleted(payload) => Some(payload.thread_id.clone()),
        _ => None,
    }
}

/// Return whether an app-server error indicates a stale thread binding whose
/// rollout no longer exists in the current Codex state store.
pub fn is_missing_thread_rollout_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("no rollout found for thread id")
}

/// Return whether an app-server error indicates the target thread is not
/// currently loaded (either the in-memory handle is gone after a restart, or
/// the rollout truly no longer exists on disk).
pub fn is_thread_unavailable_error(error: &anyhow::Error) -> bool {
    if is_missing_thread_rollout_error(error) {
        return true;
    }
    error.to_string().contains("thread not found")
}

#[async_trait]
impl CodexExecutor for CodexRuntime {
    async fn ensure_thread(
        &self,
        conversation_key: &str,
        existing_thread_id: Option<&str>,
    ) -> Result<String> {
        match existing_thread_id.filter(|thread_id| !thread_id.trim().is_empty()) {
            Some(thread_id) => {
                let request_id = self.next_request_id().await;
                let request = ClientRequest::ThreadResume {
                    request_id: request_id.clone(),
                    params: build_thread_resume_params(&self.config, thread_id, conversation_key)?,
                };
                match self
                    .send_request::<ThreadResumeResponse>(request, request_id, "thread/resume")
                    .await
                {
                    Ok(response) => Ok(response.thread.id),
                    Err(error) if is_missing_thread_rollout_error(&error) => {
                        warn!(
                            thread_id,
                            conversation_key,
                            "stale codex thread binding detected; creating a fresh thread"
                        );
                        let request_id = self.next_request_id().await;
                        let request = ClientRequest::ThreadStart {
                            request_id: request_id.clone(),
                            params: build_thread_start_params(&self.config, conversation_key)?,
                        };
                        let response: ThreadStartResponse = self
                            .send_request(request, request_id, "thread/start")
                            .await?;
                        Ok(response.thread.id)
                    },
                    Err(error) => Err(error),
                }
            },
            None => {
                let request_id = self.next_request_id().await;
                let request = ClientRequest::ThreadStart {
                    request_id: request_id.clone(),
                    params: build_thread_start_params(&self.config, conversation_key)?,
                };
                let response: ThreadStartResponse = self
                    .send_request(request, request_id, "thread/start")
                    .await?;
                Ok(response.thread.id)
            },
        }
    }

    async fn start_turn(&self, thread_id: &str, input_text: &str) -> Result<ActiveTurn> {
        // Register a fresh notification channel for this thread BEFORE
        // writing turn/start so that the reader task does not drop any
        // early notifications emitted by the app-server.
        self.register_thread_channel(thread_id).await;

        let request_id = self.next_request_id().await;
        let request = ClientRequest::TurnStart {
            request_id: request_id.clone(),
            params: build_turn_start_params(&self.config, thread_id, input_text)?,
        };
        info!(thread_id, "starting codex turn");
        let response: TurnStartResponse =
            match self.send_request(request, request_id, "turn/start").await {
                Ok(response) => response,
                Err(error) => {
                    // turn/start failed; tear down the pre-registered channel.
                    self.drop_thread_channel(thread_id).await;
                    return Err(error);
                },
            };
        info!(thread_id, turn_id = %response.turn.id, "codex turn started");

        Ok(ActiveTurn {
            thread_id: thread_id.to_string(),
            turn_id: response.turn.id,
        })
    }

    async fn wait_for_turn(&self, active_turn: &ActiveTurn) -> Result<CodexTurnResult> {
        self.wait_for_turn_with_progress(active_turn, None).await
    }

    async fn wait_for_turn_with_progress(
        &self,
        active_turn: &ActiveTurn,
        progress: Option<&dyn TurnProgressSink>,
    ) -> Result<CodexTurnResult> {
        info!(
            thread_id = %active_turn.thread_id,
            turn_id = %active_turn.turn_id,
            "waiting for codex turn completion"
        );
        let notifications = {
            let mut receivers = self.demuxer.thread_receivers.lock().await;
            receivers.remove(&active_turn.thread_id).ok_or_else(|| {
                anyhow!("no notification channel registered for thread {}", active_turn.thread_id)
            })?
        };
        let completion = self
            .wait_for_turn_completion(
                notifications,
                &active_turn.thread_id,
                &active_turn.turn_id,
                progress,
            )
            .await?;
        let final_reply = summarize_turn_result(&completion.turn, &completion.items);
        let error_message = completion
            .turn
            .error
            .as_ref()
            .map(|error| error.message.clone());
        info!(
            thread_id = %active_turn.thread_id,
            turn_id = %active_turn.turn_id,
            status = ?completion.turn.status,
            items = completion.items.len(),
            "codex turn completed"
        );

        Ok(CodexTurnResult {
            thread_id: active_turn.thread_id.clone(),
            turn_id: active_turn.turn_id.clone(),
            status: completion.turn.status.clone(),
            error_message,
            items: completion.items,
            final_reply,
        })
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        info!(thread_id, turn_id, "interrupting codex turn");
        let request_id = self.next_request_id().await;
        let request = ClientRequest::TurnInterrupt {
            request_id: request_id.clone(),
            params: build_turn_interrupt_params(thread_id, turn_id),
        };
        let request = client_request_to_jsonrpc(request)?;
        self.write_message(request).await?;
        Ok(())
    }

    async fn compact_thread(&self, thread_id: &str) -> Result<()> {
        info!(thread_id, "starting codex thread compaction");
        let request_id = self.next_request_id().await;
        let request = ClientRequest::ThreadCompactStart {
            request_id: request_id.clone(),
            params: build_thread_compact_start_params(thread_id),
        };
        let _: ThreadCompactStartResponse = self
            .send_request(request, request_id, "thread/compact/start")
            .await?;
        Ok(())
    }
}

async fn spawn_protocol_state(
    config: &CodexRuntimeConfig,
) -> Result<(Child, RuntimeWriteState, BufReader<ChildStdout>)> {
    let command = build_codex_app_server_command(config);
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(codex_app_server_workdir(config))
        .envs(build_codex_app_server_env(config))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn codex-app-server via {}", command.join(" ")))?;

    let stdin = child
        .stdin
        .take()
        .context("capture codex-app-server stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("capture codex-app-server stdout")?;

    Ok((
        child,
        RuntimeWriteState {
            stdin: BufWriter::new(stdin),
        },
        BufReader::new(stdout),
    ))
}

/// Return a short safe description for one server notification.
pub fn describe_server_notification(notification: &ServerNotification) -> Option<String> {
    match notification {
        ServerNotification::TurnStarted(payload) => Some(format!(
            "turn started: thread={} turn={} status={}",
            payload.thread_id,
            payload.turn.id,
            turn_status_label(&payload.turn.status)
        )),
        ServerNotification::TurnCompleted(payload) => Some(format!(
            "turn completed: thread={} turn={} status={}",
            payload.thread_id,
            payload.turn.id,
            turn_status_label(&payload.turn.status)
        )),
        ServerNotification::ItemStarted(payload) => Some(format!(
            "item started: thread={} turn={} item={} type={}",
            payload.thread_id,
            payload.turn_id,
            thread_item_id(&payload.item),
            thread_item_type(&payload.item)
        )),
        ServerNotification::ItemCompleted(payload) => Some(format!(
            "item completed: thread={} turn={} item={} type={}",
            payload.thread_id,
            payload.turn_id,
            thread_item_id(&payload.item),
            thread_item_type(&payload.item)
        )),
        ServerNotification::AgentMessageDelta(payload) => Some(format!(
            "assistant delta: thread={} turn={} item={} text={:?}",
            payload.thread_id, payload.turn_id, payload.item_id, payload.delta
        )),
        ServerNotification::CommandExecutionOutputDelta(payload) => Some(format!(
            "command output delta: thread={} turn={} item={} bytes={}",
            payload.thread_id,
            payload.turn_id,
            payload.item_id,
            payload.delta.len()
        )),
        ServerNotification::FileChangeOutputDelta(payload) => Some(format!(
            "file change delta: thread={} turn={} item={} bytes={}",
            payload.thread_id,
            payload.turn_id,
            payload.item_id,
            payload.delta.len()
        )),
        ServerNotification::PlanDelta(payload) => Some(format!(
            "plan delta: thread={} turn={} item={} bytes={}",
            payload.thread_id,
            payload.turn_id,
            payload.item_id,
            payload.delta.len()
        )),
        ServerNotification::TurnPlanUpdated(payload) => Some(format!(
            "turn plan updated: thread={} turn={} steps={}",
            payload.thread_id,
            payload.turn_id,
            payload.plan.len()
        )),
        ServerNotification::ReasoningSummaryTextDelta(_)
        | ServerNotification::ReasoningSummaryPartAdded(_)
        | ServerNotification::ReasoningTextDelta(_) => {
            Some("reasoning delta received (hidden)".to_string())
        },
        _ => Some(notification.to_string()),
    }
}

fn log_server_notification(notification: &ServerNotification) {
    if let Some(message) = describe_server_notification(notification) {
        info!(event = notification.to_string(), "{message}");
    }
}

fn log_server_request(request: &ServerRequest) {
    match request {
        ServerRequest::CommandExecutionRequestApproval {
            params, ..
        } => {
            info!(
                request = "commandApproval",
                thread_id = %params.thread_id,
                turn_id = %params.turn_id,
                item_id = %params.item_id,
                command = ?params.command,
                reason = ?params.reason,
                "codex approval requested"
            );
        },
        ServerRequest::FileChangeRequestApproval {
            params, ..
        } => {
            info!(
                request = "fileChangeApproval",
                thread_id = %params.thread_id,
                turn_id = %params.turn_id,
                item_id = %params.item_id,
                grant_root = ?params.grant_root,
                reason = ?params.reason,
                "codex file-change approval requested"
            );
        },
        other => {
            info!(request = %serde_json::to_string(other).unwrap_or_else(|_| "unknown".to_string()), "codex server request");
        },
    }
}

fn thread_item_field(item: &impl serde::Serialize, field: &str) -> Option<String> {
    serde_json::to_value(item)
        .ok()
        .and_then(|value| value.get(field).and_then(Value::as_str).map(str::to_string))
}

fn thread_item_id(item: &impl serde::Serialize) -> String {
    thread_item_field(item, "id").unwrap_or_else(|| "<unknown>".to_string())
}

fn thread_item_type(item: &impl serde::Serialize) -> String {
    thread_item_field(item, "type").unwrap_or_else(|| "<unknown>".to_string())
}

fn turn_status_label(status: &TurnStatus) -> &'static str {
    match status {
        TurnStatus::InProgress => "in_progress",
        TurnStatus::Completed => "completed",
        TurnStatus::Failed => "failed",
        TurnStatus::Interrupted => "interrupted",
    }
}

fn normalize_output_text(text: &str) -> Option<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }

    let truncated = trimmed
        .chars()
        .take(RECENT_OUTPUT_MAX_CHARS)
        .collect::<String>();
    if trimmed.chars().count() > RECENT_OUTPUT_MAX_CHARS {
        Some(format!("{truncated}..."))
    } else {
        Some(truncated)
    }
}

fn normalize_agent_reply_text(text: &str) -> Option<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn render_output_entry(kind: LiveOutputKind, text: &str) -> String {
    let label = match kind {
        LiveOutputKind::AgentMessage => "进度",
        LiveOutputKind::Plan => "计划",
        LiveOutputKind::CommandExecution => "命令输出",
        LiveOutputKind::FileChange => "改动输出",
    };
    format!("{label}：\n{text}")
}

/// Build the `cargo run` command used to launch the local `codex-app-server`.
pub fn build_codex_app_server_command(config: &CodexRuntimeConfig) -> Vec<String> {
    let manifest_path = config.codex_repo_root.join("Cargo.toml");
    vec![
        "cargo".to_string(),
        "run".to_string(),
        "--manifest-path".to_string(),
        manifest_path.to_string_lossy().into_owned(),
        "--bin".to_string(),
        "codex-app-server".to_string(),
        "--".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
    ]
}

/// Return the working directory used to launch the local `codex-app-server`.
pub fn codex_app_server_workdir(config: &CodexRuntimeConfig) -> PathBuf {
    config.workspace_root.clone()
}

/// Return the isolated child environment used to launch `codex-app-server`.
pub fn build_codex_app_server_env(config: &CodexRuntimeConfig) -> Vec<(String, String)> {
    let real_home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home.join(".cargo"));
    let rustup_home = env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home.join(".rustup"));
    vec![
        ("HOME".to_string(), config.child_home_root.to_string_lossy().into_owned()),
        ("CODEX_HOME".to_string(), config.codex_home_root.to_string_lossy().into_owned()),
        (
            "XDG_CONFIG_HOME".to_string(),
            config
                .child_home_root
                .join(".config")
                .to_string_lossy()
                .into_owned(),
        ),
        ("CARGO_HOME".to_string(), cargo_home.to_string_lossy().into_owned()),
        ("RUSTUP_HOME".to_string(), rustup_home.to_string_lossy().into_owned()),
    ]
}

impl RuntimeWriteState {
    async fn write_message<T>(&mut self, message: T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let mut payload = serde_json::to_string(&message).context("serialize json-rpc message")?;
        payload.push('\n');
        self.stdin.write_all(payload.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn write_response<T>(&mut self, request_id: RequestId, response: T) -> Result<()>
    where
        T: serde::Serialize,
    {
        self.write_message(JSONRPCResponse {
            id: request_id,
            result: serde_json::to_value(response).context("serialize server response payload")?,
        })
        .await
    }
}

/// Build `thread/start` params for a new long-lived QQ conversation thread.
pub fn build_thread_start_params(
    config: &CodexRuntimeConfig,
    conversation_key: &str,
) -> Result<ThreadStartParams> {
    let prompt = build_developer_instructions(config, conversation_key)?;
    Ok(ThreadStartParams {
        cwd: Some(config.workspace_root.to_string_lossy().into_owned()),
        approval_policy: Some(default_approval_policy()),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        sandbox: Some(SandboxMode::WorkspaceWrite),
        service_name: (!conversation_key.is_empty()).then(|| conversation_key.to_string()),
        developer_instructions: Some(prompt),
        persist_extended_history: true,
        ..Default::default()
    })
}

/// Build `thread/resume` params using the current runtime-owned prompt file.
pub fn build_thread_resume_params(
    config: &CodexRuntimeConfig,
    thread_id: &str,
    conversation_key: &str,
) -> Result<ThreadResumeParams> {
    let prompt = build_developer_instructions(config, conversation_key)?;
    Ok(ThreadResumeParams {
        thread_id: thread_id.to_string(),
        cwd: Some(config.workspace_root.to_string_lossy().into_owned()),
        approval_policy: Some(default_approval_policy()),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        sandbox: Some(SandboxMode::WorkspaceWrite),
        developer_instructions: Some(prompt),
        persist_extended_history: true,
        ..Default::default()
    })
}

/// Build `thread/compact/start` params for one existing thread.
pub fn build_thread_compact_start_params(thread_id: &str) -> ThreadCompactStartParams {
    ThreadCompactStartParams {
        thread_id: thread_id.to_string(),
    }
}

/// Assemble the five-layer developer instructions handed to Codex at
/// every `thread/start` / `thread/resume`:
///
/// 1. **Persona** — operator-editable identity / voice / project skills loaded
///    from [`CodexRuntimeConfig::prompt_file`] (which points at `persona.md`
///    since the layered refactor).
/// 2. **Bridge protocol** — static, embedded-in-binary text covering the
///    turn-start checklist, mention / quote / reply-to / permissions rules.
/// 3. **Admin context** — a tiny runtime-rendered block that embeds the admin's
///    QQ id so Codex can recognise 主人 even when the inbound `[主人]` marker
///    is absent.
/// 4. **Model capabilities** — the hot-reloadable "Available model
///    capabilities" section rendered from the
///    [`crate::model_capabilities::ModelRegistry`], when non-empty.
/// 5. **Reply context** — the per-thread absolute path to the reply context
///    file for THIS conversation. Prevents cross-conversation reply delivery
///    when multiple tasks are active concurrently.
fn build_developer_instructions(
    config: &CodexRuntimeConfig,
    conversation_key: &str,
) -> Result<String> {
    let persona = load_persona(&config.prompt_file)?;
    let bridge = crate::system_prompt::BRIDGE_PROTOCOL_TEXT;
    let admin = crate::system_prompt::render_admin_block(config.admin_user_id);

    let capabilities_block = match config.capabilities_block.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => {
            tracing::warn!("capabilities_block lock poisoned; ignoring");
            poisoned.into_inner().clone()
        },
    };

    let mut layers: Vec<String> =
        vec![persona.trim().to_string(), bridge.trim().to_string(), admin.trim().to_string()];
    if let Some(block) = capabilities_block {
        let trimmed = block.trim();
        if !trimmed.is_empty() {
            layers.push(trimmed.to_string());
        }
    }
    if !conversation_key.is_empty() && !config.reply_contexts_dir.as_os_str().is_empty() {
        layers.push(
            crate::system_prompt::render_reply_context_block(
                &config.reply_contexts_dir,
                conversation_key,
            )
            .trim()
            .to_string(),
        );
    }

    Ok(layers.join("\n\n") + "\n")
}

fn discover_project_skills(workspace_root: &Path) -> Result<Vec<UserInput>> {
    let skills_root = workspace_root.join("skills");
    let entries = fs::read_dir(&skills_root)
        .with_context(|| format!("read project skills directory {}", skills_root.display()))?;
    let mut skills = Vec::new();

    for entry in entries {
        let entry = entry
            .with_context(|| format!("read project skill entry under {}", skills_root.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", entry.path().display()))?;
        if !file_type.is_dir() {
            continue;
        }

        let skill_name = entry.file_name().to_string_lossy().into_owned();
        let skill_path = entry.path().join("SKILL.md");
        if skill_path.is_file() {
            skills.push(UserInput::Skill {
                name: skill_name,
                path: skill_path,
            });
        }
    }

    skills.sort_by(|left, right| match (left, right) {
        (
            UserInput::Skill {
                name: left, ..
            },
            UserInput::Skill {
                name: right, ..
            },
        ) => left.cmp(right),
        _ => std::cmp::Ordering::Equal,
    });

    if skills.is_empty() {
        anyhow::bail!("no project skills were found under {}", skills_root.display());
    }

    if !skills.iter().any(|skill| {
        matches!(
            skill,
            UserInput::Skill { name, .. } if name == "reply-current"
        )
    }) {
        anyhow::bail!(
            "required project skill reply-current is missing under {}",
            skills_root.display()
        );
    }

    Ok(skills)
}

/// Build `turn/start` params using the QQ bot's fixed safety policy.
pub fn build_turn_start_params(
    config: &CodexRuntimeConfig,
    thread_id: &str,
    input_text: &str,
) -> Result<TurnStartParams> {
    let mut input = vec![UserInput::Text {
        text: input_text.to_string(),
        text_elements: Vec::new(),
    }];
    input.extend(discover_project_skills(&config.workspace_root)?);

    Ok(TurnStartParams {
        thread_id: thread_id.to_string(),
        input,
        cwd: Some(config.workspace_root.clone()),
        approval_policy: Some(default_approval_policy()),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        sandbox_policy: Some(default_sandbox_policy(&config.workspace_root)),
        ..Default::default()
    })
}

/// Build `turn/interrupt` params for the active turn.
pub fn build_turn_interrupt_params(thread_id: &str, turn_id: &str) -> TurnInterruptParams {
    TurnInterruptParams {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
    }
}

/// Extract all agent/assistant text messages from completed turn items.
pub fn extract_final_reply(items: &[Value]) -> Option<String> {
    let replies = items
        .iter()
        .filter_map(extract_message_text)
        .collect::<Vec<_>>();
    if replies.is_empty() {
        None
    } else {
        Some(replies.join("\n\n"))
    }
}

/// Summarize a terminal turn into the QQ-facing reply text.
pub fn summarize_turn_result(turn: &Turn, items: &[Value]) -> Option<String> {
    if let Some(reply) = extract_final_reply(items) {
        return Some(reply);
    }

    match turn.status {
        TurnStatus::Failed => Some(match turn.error.as_ref() {
            Some(error) => format!("执行失败。\n原因：{}", error.message),
            None => "执行失败。".to_string(),
        }),
        TurnStatus::Interrupted => {
            Some("任务因服务重启或异常中断。可使用 /retry_last 重试。".to_string())
        },
        TurnStatus::Completed | TurnStatus::InProgress => None,
    }
}

fn client_request_to_jsonrpc(request: ClientRequest) -> Result<JSONRPCRequest> {
    let value = serde_json::to_value(request).context("serialize client request")?;
    serde_json::from_value(value).context("convert client request to json-rpc")
}

fn default_approval_policy() -> AskForApproval {
    AskForApproval::Granular {
        sandbox_approval: true,
        rules: false,
        skill_approval: false,
        request_permissions: false,
        mcp_elicitations: false,
    }
}

fn default_sandbox_policy(workspace_root: &PathBuf) -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![AbsolutePathBuf::from_absolute_path(workspace_root)
            .expect("workspace root must be absolute")],
        read_only_access: ReadOnlyAccess::FullAccess,
        network_access: true,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
    }
}

/// Build the command-approval response under the local workspace policy.
pub fn build_command_approval_response(
    guard: &ApprovalGuard,
    params: &CommandExecutionRequestApprovalParams,
) -> CommandExecutionRequestApprovalResponse {
    if requests_additional_permissions(params) {
        return CommandExecutionRequestApprovalResponse {
            decision: CommandExecutionApprovalDecision::Decline,
        };
    }

    let command = params.command.as_deref().unwrap_or_default();
    let cwd = params
        .cwd
        .as_ref()
        .and_then(|path| path.to_str())
        .unwrap_or_default();

    let decision = match guard.review_command(command, cwd, &[]) {
        ApprovalDecision::Allow => CommandExecutionApprovalDecision::Accept,
        ApprovalDecision::Deny(_) => CommandExecutionApprovalDecision::Decline,
    };

    CommandExecutionRequestApprovalResponse {
        decision,
    }
}

/// Build the file-change approval response under the local workspace policy.
pub fn build_file_change_approval_response(
    guard: &ApprovalGuard,
    params: &FileChangeRequestApprovalParams,
) -> FileChangeRequestApprovalResponse {
    let decision = match guard.review_file_change(params.grant_root.as_deref()) {
        ApprovalDecision::Allow => FileChangeApprovalDecision::Accept,
        ApprovalDecision::Deny(_) => FileChangeApprovalDecision::Decline,
    };

    FileChangeRequestApprovalResponse {
        decision,
    }
}

fn requests_additional_permissions(params: &CommandExecutionRequestApprovalParams) -> bool {
    params.additional_permissions.is_some()
        || params.network_approval_context.is_some()
        || params.proposed_execpolicy_amendment.is_some()
        || params
            .proposed_network_policy_amendments
            .as_ref()
            .is_some_and(|amendments| !amendments.is_empty())
}

fn extract_message_text(item: &Value) -> Option<String> {
    let item = item.get("item").unwrap_or(item);
    let item_type = item.get("type")?.as_str()?;

    match item_type {
        "agentMessage" | "assistantMessage" | "assistant" => item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string),
        _ => None,
    }
}
