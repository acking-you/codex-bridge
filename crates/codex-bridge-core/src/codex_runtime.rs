//! Thin stdio runtime wrapper for `codex app-server`.

use std::{collections::VecDeque, path::PathBuf, process::Stdio};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use codex_app_server_protocol::{
    ApprovalsReviewer, AskForApproval, ClientInfo, ClientNotification, ClientRequest,
    CommandExecutionApprovalDecision, CommandExecutionRequestApprovalParams,
    CommandExecutionRequestApprovalResponse, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, FileChangeRequestApprovalResponse, InitializeCapabilities,
    InitializeParams, InitializeResponse, JSONRPCMessage, JSONRPCNotification, JSONRPCRequest,
    JSONRPCResponse, ReadOnlyAccess, RequestId, SandboxMode, SandboxPolicy, ServerNotification,
    ServerRequest, ThreadResumeParams, ThreadResumeResponse, ThreadStartParams,
    ThreadStartResponse, Turn, TurnInterruptParams, TurnInterruptResponse, TurnStartParams,
    TurnStartResponse, TurnStatus, UserInput,
};
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

use crate::{
    approval_guard::{ApprovalDecision, ApprovalGuard},
    system_prompt::SYSTEM_PROMPT_TEXT,
};

const DEFAULT_CLIENT_NAME: &str = "codex-bridge";

/// Runtime-local configuration for launching and routing through a local
/// app-server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRuntimeConfig {
    /// Path to the Codex repository root containing the app-server workspace
    /// `Cargo.toml`.
    pub codex_repo_root: PathBuf,
    /// Workspace directory used for thread and turn execution.
    pub workspace_root: PathBuf,
    /// Client name reported during initialize.
    pub client_name: String,
    /// Client version reported during initialize.
    pub client_version: String,
}

impl CodexRuntimeConfig {
    /// Build runtime configuration from repository and workspace roots.
    pub fn new(codex_repo_root: impl Into<PathBuf>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            codex_repo_root: codex_repo_root.into(),
            workspace_root: workspace_root.into(),
            client_name: DEFAULT_CLIENT_NAME.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[derive(Debug)]
struct RuntimeProtocolState {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    pending_notifications: VecDeque<JSONRPCNotification>,
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

/// Minimal interface for codex execution runtimes.
#[async_trait]
pub trait CodexExecutor: Send + Sync {
    /// Ensure a thread is available and return its id.
    async fn ensure_thread(
        &self,
        conversation_key: &str,
        existing_thread_id: Option<&str>,
    ) -> Result<String>;

    /// Run a turn and return a summary result.
    async fn run_turn(&self, thread_id: &str, input_text: &str) -> Result<CodexTurnResult>;

    /// Interrupt a running turn when supported.
    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()>;
}

/// Concrete runtime implementation backed by a child `codex-app-server`
/// process.
#[derive(Debug)]
pub struct CodexRuntime {
    state: Mutex<RuntimeProtocolState>,
    config: CodexRuntimeConfig,
    guard: ApprovalGuard,
    next_request_id: Mutex<u64>,
}

impl CodexRuntime {
    /// Create and initialize a runtime connected to a local app-server process.
    pub async fn new(config: CodexRuntimeConfig) -> Result<Self> {
        let state = RuntimeProtocolState::spawn(&config).await?;
        let runtime = Self {
            state: Mutex::new(state),
            guard: ApprovalGuard::new(&config.workspace_root),
            config,
            next_request_id: Mutex::new(1),
        };
        runtime.initialize().await?;
        Ok(runtime)
    }

    /// Replace the command guard used for approval requests.
    pub fn with_guard(mut self, guard: ApprovalGuard) -> Self {
        self.guard = guard;
        self
    }

    async fn initialize(&self) -> Result<()> {
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
        Ok(())
    }

    async fn send_notification(&self, notification: ClientNotification) -> Result<()> {
        let mut state = self.state.lock().await;
        let value = serde_json::to_value(notification).context("serialize client notification")?;
        let notification: JSONRPCNotification =
            serde_json::from_value(value).context("convert client notification to json-rpc")?;
        state.write_message(notification).await
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
        let mut state = self.state.lock().await;
        let request = client_request_to_jsonrpc(request)?;
        state.write_message(request).await?;

        loop {
            match state.read_message().await? {
                JSONRPCMessage::Response(JSONRPCResponse {
                    id,
                    result,
                }) if id == request_id => {
                    return serde_json::from_value(result)
                        .with_context(|| format!("decode {method} response payload"));
                },
                JSONRPCMessage::Error(error) if error.id == request_id => {
                    return Err(anyhow!("{method} failed: {}", error.error.message));
                },
                JSONRPCMessage::Notification(notification) => {
                    state.pending_notifications.push_back(notification);
                },
                JSONRPCMessage::Request(request) => {
                    self.handle_server_request(&mut state, request).await?;
                },
                JSONRPCMessage::Response(_) | JSONRPCMessage::Error(_) => {},
            }
        }
    }

    async fn handle_server_request(
        &self,
        state: &mut RuntimeProtocolState,
        request: JSONRPCRequest,
    ) -> Result<()> {
        let request = ServerRequest::try_from(request).context("decode server request")?;
        match request {
            ServerRequest::CommandExecutionRequestApproval {
                request_id,
                params,
            } => {
                let response = build_command_approval_response(&self.guard, &params);
                state.write_response(request_id, response).await?;
            },
            ServerRequest::FileChangeRequestApproval {
                request_id,
                params,
            } => {
                state
                    .write_response(
                        request_id,
                        build_file_change_approval_response(&self.guard, &params),
                    )
                    .await?;
            },
            other => {
                return Err(anyhow!("unsupported server request: {other:?}"));
            },
        }
        Ok(())
    }

    async fn wait_for_turn_completion(
        &self,
        state: &mut RuntimeProtocolState,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<TurnCompletion> {
        let mut items = Vec::new();
        loop {
            let message = if let Some(notification) = state.pending_notifications.pop_front() {
                JSONRPCMessage::Notification(notification)
            } else {
                state.read_message().await?
            };

            match message {
                JSONRPCMessage::Notification(notification) => {
                    let notification = match ServerNotification::try_from(notification) {
                        Ok(notification) => notification,
                        Err(_) => continue,
                    };

                    match notification {
                        ServerNotification::ItemCompleted(payload) => {
                            items.push(
                                serde_json::to_value(payload.item)
                                    .context("serialize completed item")?,
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
                },
                JSONRPCMessage::Request(request) => {
                    self.handle_server_request(state, request).await?;
                },
                JSONRPCMessage::Response(_) | JSONRPCMessage::Error(_) => {},
            }
        }
    }

    async fn next_request_id(&self) -> RequestId {
        let mut next = self.next_request_id.lock().await;
        let id = *next;
        *next += 1;
        RequestId::String(format!("qqbot-{id}"))
    }
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
                    params: build_thread_resume_params(&self.config, thread_id),
                };
                let response: ThreadResumeResponse = self
                    .send_request(request, request_id, "thread/resume")
                    .await?;
                Ok(response.thread.id)
            },
            None => {
                let request_id = self.next_request_id().await;
                let request = ClientRequest::ThreadStart {
                    request_id: request_id.clone(),
                    params: build_thread_start_params(&self.config, conversation_key),
                };
                let response: ThreadStartResponse = self
                    .send_request(request, request_id, "thread/start")
                    .await?;
                Ok(response.thread.id)
            },
        }
    }

    async fn run_turn(&self, thread_id: &str, input_text: &str) -> Result<CodexTurnResult> {
        let request_id = self.next_request_id().await;
        let request = ClientRequest::TurnStart {
            request_id: request_id.clone(),
            params: build_turn_start_params(&self.config, thread_id, input_text),
        };
        let response: TurnStartResponse =
            self.send_request(request, request_id, "turn/start").await?;

        let mut state = self.state.lock().await;
        let completion = self
            .wait_for_turn_completion(&mut state, thread_id, &response.turn.id)
            .await?;
        let final_reply = summarize_turn_result(&completion.turn, &completion.items);
        let error_message = completion
            .turn
            .error
            .as_ref()
            .map(|error| error.message.clone());

        Ok(CodexTurnResult {
            thread_id: thread_id.to_string(),
            turn_id: response.turn.id,
            status: completion.turn.status.clone(),
            error_message,
            items: completion.items,
            final_reply,
        })
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        let request_id = self.next_request_id().await;
        let request = ClientRequest::TurnInterrupt {
            request_id: request_id.clone(),
            params: build_turn_interrupt_params(thread_id, turn_id),
        };
        let _: TurnInterruptResponse = self
            .send_request(request, request_id, "turn/interrupt")
            .await?;
        Ok(())
    }
}

impl RuntimeProtocolState {
    async fn spawn(config: &CodexRuntimeConfig) -> Result<Self> {
        let manifest_path = config.codex_repo_root.join("Cargo.toml");
        let mut child = Command::new("cargo")
            .arg("run")
            .arg("--manifest-path")
            .arg(&manifest_path)
            .arg("-p")
            .arg("codex-app-server")
            .arg("--")
            .arg("--listen")
            .arg("stdio://")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| {
                format!("spawn codex-app-server via manifest {}", manifest_path.display())
            })?;

        let stdin = child
            .stdin
            .take()
            .context("capture codex-app-server stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("capture codex-app-server stdout")?;

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            pending_notifications: VecDeque::new(),
        })
    }

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

    async fn read_message(&mut self) -> Result<JSONRPCMessage> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = self.stdout.read_line(&mut line).await?;
            if bytes == 0 {
                return Err(anyhow!("codex app-server stdout closed"));
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let payload: Value =
                serde_json::from_str(trimmed).context("decode json-rpc payload")?;
            return serde_json::from_value(payload).context("decode json-rpc message");
        }
    }
}

impl Drop for RuntimeProtocolState {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Build `thread/start` params for a new long-lived QQ conversation thread.
pub fn build_thread_start_params(
    config: &CodexRuntimeConfig,
    conversation_key: &str,
) -> ThreadStartParams {
    ThreadStartParams {
        cwd: Some(config.workspace_root.to_string_lossy().into_owned()),
        approval_policy: Some(default_approval_policy()),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        sandbox: Some(SandboxMode::WorkspaceWrite),
        service_name: (!conversation_key.is_empty()).then(|| conversation_key.to_string()),
        developer_instructions: Some(SYSTEM_PROMPT_TEXT.to_string()),
        persist_extended_history: true,
        ..Default::default()
    }
}

/// Build `thread/resume` params without mutating the existing prompt version.
pub fn build_thread_resume_params(
    config: &CodexRuntimeConfig,
    thread_id: &str,
) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id: thread_id.to_string(),
        cwd: Some(config.workspace_root.to_string_lossy().into_owned()),
        approval_policy: Some(default_approval_policy()),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        sandbox: Some(SandboxMode::WorkspaceWrite),
        persist_extended_history: true,
        ..Default::default()
    }
}

/// Build `turn/start` params using the QQ bot's fixed safety policy.
pub fn build_turn_start_params(
    config: &CodexRuntimeConfig,
    thread_id: &str,
    input_text: &str,
) -> TurnStartParams {
    TurnStartParams {
        thread_id: thread_id.to_string(),
        input: vec![UserInput::Text {
            text: input_text.to_string(),
            text_elements: Vec::new(),
        }],
        cwd: Some(config.workspace_root.clone()),
        approval_policy: Some(default_approval_policy()),
        approvals_reviewer: Some(ApprovalsReviewer::User),
        sandbox_policy: Some(default_sandbox_policy(&config.workspace_root)),
        ..Default::default()
    }
}

/// Build `turn/interrupt` params for the active turn.
pub fn build_turn_interrupt_params(thread_id: &str, turn_id: &str) -> TurnInterruptParams {
    TurnInterruptParams {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
    }
}

/// Extract the last agent/assistant text message from completed turn items.
pub fn extract_final_reply(items: &[Value]) -> Option<String> {
    items.iter().rev().find_map(extract_message_text)
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
        || params.skill_metadata.is_some()
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
