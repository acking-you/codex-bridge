//! Shared in-memory state for the foreground QQ bridge.

use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    spawn,
    sync::{broadcast, mpsc, oneshot, Mutex, RwLock},
};
use uuid::Uuid;

use crate::{
    conversation_history::{HistoryQuery, HistoryQueryResult},
    events::NormalizedEvent,
    lane_manager::RuntimeSnapshot,
    message_router::CommandRequest,
    outbound::OutboundMessage,
    reply_context::{ActiveReplyContext, ReplyRegistry},
};

/// Current session lifecycle state exposed by the local API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The service has started but is not yet connected.
    Booting,
    /// The service is waiting for QR-code login or QQ initialization.
    WaitingForLogin,
    /// The bridge is connected and ready to send or receive messages.
    Connected,
    /// The bridge has lost its connection to the QQ runtime.
    Disconnected,
}

/// Public session snapshot returned by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Current service lifecycle state.
    pub status: SessionStatus,
    /// Logged-in QQ identifier when available.
    pub self_id: Option<i64>,
    /// Logged-in QQ nickname when available.
    pub nickname: Option<String>,
    /// Foreground QQ process identifier when available.
    pub qq_pid: Option<u32>,
}

/// Cached runtime snapshot used by API/status endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskSnapshot {
    /// Running task identifier, if any.
    pub running_task_id: Option<String>,
    /// Conversation key for the running task.
    pub running_conversation_key: Option<String>,
    /// Summary of the running task if it has completed.
    pub running_summary: Option<String>,
    /// Recent live text output for the currently running task.
    pub recent_output: Vec<String>,
    /// Current number of tasks waiting in queue.
    pub queue_len: usize,
    /// Summary from the most recent terminal task.
    pub last_terminal_summary: Option<String>,
    /// Conversation key for the most recent retryable task, if any.
    pub last_retryable_conversation_key: Option<String>,
    /// Runtime-owned system prompt file currently used for turn injection.
    pub prompt_file: Option<String>,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            status: SessionStatus::Booting,
            self_id: None,
            nickname: None,
            qq_pid: None,
        }
    }
}

/// Simplified friend profile exposed by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendProfile {
    /// QQ identifier.
    pub user_id: i64,
    /// QQ nickname.
    pub nickname: String,
    /// Optional remark shown in contacts.
    pub remark: Option<String>,
}

/// Simplified group profile exposed by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupProfile {
    /// Group identifier.
    pub group_id: i64,
    /// Group display name.
    pub group_name: String,
}

/// Message send result returned from the bridge worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageReceipt {
    /// Message identifier returned by the underlying transport.
    pub message_id: i64,
}

/// Internal command sent from the API layer to the bridge worker.
#[derive(Debug)]
pub enum ServiceCommand {
    /// Send a private message.
    SendPrivate {
        /// Target QQ identifier.
        user_id: i64,
        /// Plain-text message content.
        text: String,
        /// Response channel for completion.
        respond_to: oneshot::Sender<Result<SendMessageReceipt>>,
    },
    /// Send a group message.
    SendGroup {
        /// Target group identifier.
        group_id: i64,
        /// Plain-text message content.
        text: String,
        /// Response channel for completion.
        respond_to: oneshot::Sender<Result<SendMessageReceipt>>,
    },
    /// Send a structured outbound message assembled by the reply API.
    SendOutbound {
        /// Structured outbound transport payload.
        message: OutboundMessage,
        /// Response channel for completion.
        respond_to: oneshot::Sender<Result<SendMessageReceipt>>,
    },
    /// Apply an emoji reaction to one QQ message.
    SetMessageReaction {
        /// Source QQ message identifier.
        message_id: i64,
        /// Emoji identifier recognized by NapCat.
        emoji_id: String,
        /// Response channel for completion.
        respond_to: oneshot::Sender<Result<()>>,
    },
    /// Fetch a historical QQ message via OneBot `get_msg` so the orchestrator
    /// can surface it as quoted context to the agent.
    FetchMessage {
        /// Target QQ message identifier.
        message_id: i64,
        /// Bot self-id used to render the placeholder-preserving text.
        self_id: i64,
        /// Response channel carrying the rendered message.
        respond_to: oneshot::Sender<Result<crate::napcat::FetchedMessage>>,
    },
    /// Fetch normalized QQ history scoped to the current conversation lane.
    FetchConversationHistory {
        /// Whether the current lane is a group chat.
        is_group: bool,
        /// QQ target identifier for the current lane.
        target_id: i64,
        /// Bot self-id used for placeholder-preserving text rendering.
        self_id: i64,
        /// Query options for filtering the fetched history slice.
        query: HistoryQuery,
        /// Response channel carrying normalized history.
        respond_to: oneshot::Sender<Result<HistoryQueryResult>>,
    },
    /// Route an orchestrator control command.
    Control {
        /// Payload for a local control command.
        command: CommandRequest,
    },
}

#[derive(Debug)]
struct ServiceInner {
    session: RwLock<SessionSnapshot>,
    friends: RwLock<Vec<FriendProfile>>,
    groups: RwLock<Vec<GroupProfile>>,
    task_snapshot: RwLock<TaskSnapshot>,
    runtime_snapshot: RwLock<RuntimeSnapshot>,
    events_tx: broadcast::Sender<NormalizedEvent>,
    command_tx: mpsc::Sender<ServiceCommand>,
    control_tx: mpsc::Sender<ServiceCommand>,
    reply_registry: Mutex<ReplyRegistry>,
    capabilities: RwLock<Arc<crate::model_capabilities::ModelRegistry>>,
    /// Shared, hot-reloadable "Available model capabilities" prompt
    /// block. The same `Arc` is handed to
    /// [`crate::codex_runtime::CodexRuntimeConfig::capabilities_block`]
    /// so an operator-triggered reload is visible on Codex's next
    /// `thread/start` / `thread/resume`.
    capabilities_prompt_block: Arc<std::sync::RwLock<Option<String>>>,
    /// Path to `model_capabilities.toml` on disk. Set by the launcher
    /// before publishing the first registry so
    /// [`ServiceState::reload_capabilities`] knows which file to
    /// re-parse. `None` means reload is not wired up (test fixtures).
    capabilities_file: std::sync::RwLock<Option<PathBuf>>,
}

/// Cloneable handle shared across the API layer and runtime worker.
#[derive(Clone, Debug)]
pub struct ServiceState {
    inner: Arc<ServiceInner>,
}

impl ServiceState {
    /// Build a new service state around the provided command channel.
    pub fn new(command_tx: mpsc::Sender<ServiceCommand>) -> Self {
        let (control_tx, mut control_rx) = mpsc::channel(64);
        spawn(async move {
            while let Some(command) = control_rx.recv().await {
                let ServiceCommand::Control {
                    command: _,
                } = command
                else {
                    continue;
                };
            }
        });
        Self::with_control(command_tx, control_tx)
    }

    /// Build a service state with a custom control channel.
    pub fn with_control(
        command_tx: mpsc::Sender<ServiceCommand>,
        control_tx: mpsc::Sender<ServiceCommand>,
    ) -> Self {
        Self::with_control_and_reply_context_paths(
            command_tx,
            control_tx,
            test_reply_contexts_dir(),
        )
    }

    /// Build a service state with explicit per-conversation reply-context
    /// paths.
    pub fn with_control_and_reply_context_paths(
        command_tx: mpsc::Sender<ServiceCommand>,
        control_tx: mpsc::Sender<ServiceCommand>,
        reply_contexts_dir: PathBuf,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(ServiceInner {
                session: RwLock::new(SessionSnapshot::default()),
                friends: RwLock::new(Vec::new()),
                groups: RwLock::new(Vec::new()),
                task_snapshot: RwLock::new(TaskSnapshot::default()),
                runtime_snapshot: RwLock::new(RuntimeSnapshot::default()),
                events_tx,
                command_tx,
                control_tx,
                reply_registry: Mutex::new(ReplyRegistry::new(reply_contexts_dir)),
                capabilities: RwLock::new(Arc::new(
                    crate::model_capabilities::ModelRegistry::empty(),
                )),
                capabilities_prompt_block: Arc::new(std::sync::RwLock::new(None)),
                capabilities_file: std::sync::RwLock::new(None),
            }),
        }
    }

    /// Build a test-friendly state with no active bridge worker.
    pub fn for_tests() -> Self {
        let (command_tx, mut command_rx) = mpsc::channel(8);
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    ServiceCommand::SendPrivate {
                        user_id,
                        text,
                        respond_to,
                    } => {
                        let _ = respond_to.send(Ok(SendMessageReceipt {
                            message_id: user_id.saturating_add(text.len() as i64),
                        }));
                    },
                    ServiceCommand::SendGroup {
                        group_id,
                        text,
                        respond_to,
                    } => {
                        let _ = respond_to.send(Ok(SendMessageReceipt {
                            message_id: group_id.saturating_add(text.len() as i64),
                        }));
                    },
                    ServiceCommand::SendOutbound {
                        message,
                        respond_to,
                    } => {
                        let _ = respond_to.send(Ok(SendMessageReceipt {
                            message_id: 10_000 + message.segments.len() as i64,
                        }));
                    },
                    ServiceCommand::SetMessageReaction {
                        respond_to, ..
                    } => {
                        let _ = respond_to.send(Ok(()));
                    },
                    ServiceCommand::FetchMessage {
                        respond_to, ..
                    } => {
                        let _ = respond_to
                            .send(Err(anyhow!("FetchMessage is not available in for_tests")));
                    },
                    ServiceCommand::FetchConversationHistory {
                        respond_to, ..
                    } => {
                        let _ = respond_to.send(Err(anyhow!(
                            "FetchConversationHistory is not available in for_tests"
                        )));
                    },
                    ServiceCommand::Control {
                        command: _,
                    } => {},
                }
            }
        });
        Self::new(command_tx)
    }

    /// Replace the current session snapshot.
    pub async fn set_session(&self, snapshot: SessionSnapshot) {
        *self.inner.session.write().await = snapshot;
    }

    /// Read the current session snapshot.
    pub async fn session(&self) -> SessionSnapshot {
        self.inner.session.read().await.clone()
    }

    /// Replace the cached friend list.
    pub async fn set_friends(&self, friends: Vec<FriendProfile>) {
        *self.inner.friends.write().await = friends;
    }

    /// Read the cached friend list.
    pub async fn friends(&self) -> Vec<FriendProfile> {
        self.inner.friends.read().await.clone()
    }

    /// Replace the cached group list.
    pub async fn set_groups(&self, groups: Vec<GroupProfile>) {
        *self.inner.groups.write().await = groups;
    }

    /// Read the cached group list.
    pub async fn groups(&self) -> Vec<GroupProfile> {
        self.inner.groups.read().await.clone()
    }

    /// Replace the current task snapshot.
    pub async fn set_task_snapshot(&self, snapshot: TaskSnapshot) {
        *self.inner.task_snapshot.write().await = snapshot;
    }

    /// Mutate the current task snapshot in place.
    pub async fn update_task_snapshot<F>(&self, update: F)
    where
        F: FnOnce(&mut TaskSnapshot),
    {
        let mut snapshot = self.inner.task_snapshot.write().await;
        update(&mut snapshot);
    }

    /// Read the current task snapshot.
    pub async fn task_snapshot(&self) -> TaskSnapshot {
        self.inner.task_snapshot.read().await.clone()
    }

    /// Replace the current multi-lane runtime snapshot.
    pub async fn set_runtime_snapshot(&self, snapshot: RuntimeSnapshot) {
        *self.inner.runtime_snapshot.write().await = snapshot;
    }

    /// Read the current multi-lane runtime snapshot.
    pub async fn runtime_snapshot(&self) -> RuntimeSnapshot {
        self.inner.runtime_snapshot.read().await.clone()
    }

    /// Publish a normalized event to local websocket subscribers.
    pub fn publish_event(&self, event: NormalizedEvent) {
        let _ = self.inner.events_tx.send(event);
    }

    /// Subscribe to normalized events for websocket streaming.
    pub fn subscribe_events(&self) -> broadcast::Receiver<NormalizedEvent> {
        self.inner.events_tx.subscribe()
    }

    /// Activate a new reply context (concurrent contexts are allowed).
    pub async fn activate_reply_context(&self, context: ActiveReplyContext) -> Result<()> {
        let mut registry = self.inner.reply_registry.lock().await;
        registry.activate(context)
    }

    /// Revoke a reply context once its task finishes.
    pub async fn deactivate_reply_context(&self, token: &str) -> Result<()> {
        let mut registry = self.inner.reply_registry.lock().await;
        registry.deactivate(token)
    }

    /// Resolve an active reply token into its current context.
    pub async fn reply_context(&self, token: &str) -> Result<ActiveReplyContext> {
        let registry = self.inner.reply_registry.lock().await;
        registry.resolve(token)
    }

    /// Mark one successful skill reply against the active token.
    pub async fn mark_reply_sent(&self, token: &str) -> Result<usize> {
        let mut registry = self.inner.reply_registry.lock().await;
        registry.mark_sent(token)
    }

    /// Read the number of successful skill replies issued for one token.
    pub async fn reply_sent_count(&self, token: &str) -> usize {
        let registry = self.inner.reply_registry.lock().await;
        registry.send_count_for(token)
    }

    /// Dispatch a private-message send request to the bridge worker.
    pub async fn send_private_message(
        &self,
        user_id: i64,
        text: String,
    ) -> Result<SendMessageReceipt> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::SendPrivate {
                user_id,
                text,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Dispatch a group-message send request to the bridge worker.
    pub async fn send_group_message(
        &self,
        group_id: i64,
        text: String,
    ) -> Result<SendMessageReceipt> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::SendGroup {
                group_id,
                text,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Dispatch a structured outbound message send request.
    pub async fn send_outbound_message(
        &self,
        message: OutboundMessage,
    ) -> Result<SendMessageReceipt> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::SendOutbound {
                message,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Dispatch one emoji reaction request to the bridge worker.
    pub async fn set_message_reaction(&self, message_id: i64, emoji_id: String) -> Result<()> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::SetMessageReaction {
                message_id,
                emoji_id,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Fetch one historical QQ message via OneBot `get_msg`.
    ///
    /// `self_id` is forwarded so the fetched text is rendered with the same
    /// `@<bot>` placeholder rules the orchestrator already uses for live
    /// events.
    pub async fn fetch_message(
        &self,
        message_id: i64,
        self_id: i64,
    ) -> Result<crate::napcat::FetchedMessage> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::FetchMessage {
                message_id,
                self_id,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Query normalized QQ history for the active conversation identified by
    /// one reply token.
    pub async fn query_current_conversation_history(
        &self,
        token: &str,
        query: HistoryQuery,
    ) -> Result<HistoryQueryResult> {
        let context = self.reply_context(token).await?;
        let self_id = self.session().await.self_id.unwrap_or_default();
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::FetchConversationHistory {
                is_group: context.is_group,
                target_id: context.reply_target_id,
                self_id,
                query,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Dispatch a control command to the orchestrator runtime.
    pub async fn send_control_command(&self, command: CommandRequest) -> Result<()> {
        self.inner
            .control_tx
            .send(ServiceCommand::Control {
                command,
            })
            .await
            .map_err(|_| anyhow!("orchestrator is not available"))?;
        Ok(())
    }

    /// Replace the shared model-capability registry atomically with its
    /// rendered prompt block. Subsequent callers — both the API layer
    /// and the Codex runtime (via the shared `capabilities_block`
    /// `Arc`) — observe the new state on their next access.
    pub async fn set_capabilities(
        &self,
        registry: Arc<crate::model_capabilities::ModelRegistry>,
    ) {
        let block = registry.render_prompt_block();
        *self.inner.capabilities.write().await = registry;
        match self.inner.capabilities_prompt_block.write() {
            Ok(mut guard) => *guard = block,
            Err(poisoned) => {
                tracing::warn!("capabilities_prompt_block lock poisoned; recovering");
                *poisoned.into_inner() = block;
            },
        }
    }

    /// Clone a handle to the currently active model-capability
    /// registry. Cheap: the inner registry is refcounted.
    pub async fn capabilities(&self) -> Arc<crate::model_capabilities::ModelRegistry> {
        self.inner.capabilities.read().await.clone()
    }

    /// Return a cloneable `Arc` handle to the shared capabilities prompt
    /// block. Hand this to [`crate::codex_runtime::CodexRuntimeConfig`]
    /// so prompt injection and hot-reload share one cell.
    pub fn capabilities_prompt_block_handle(
        &self,
    ) -> Arc<std::sync::RwLock<Option<String>>> {
        self.inner.capabilities_prompt_block.clone()
    }

    /// Tell the bridge where the capabilities TOML lives on disk, so
    /// [`Self::reload_capabilities`] knows which file to re-parse. Call
    /// once during launcher setup.
    pub fn set_capabilities_file(&self, path: PathBuf) {
        match self.inner.capabilities_file.write() {
            Ok(mut guard) => *guard = Some(path),
            Err(poisoned) => {
                tracing::warn!("capabilities_file lock poisoned; recovering");
                *poisoned.into_inner() = Some(path);
            },
        }
    }

    /// Re-parse the configured `model_capabilities.toml`, publish the
    /// new registry and rendered prompt block. Returns the number of
    /// registered capabilities after the reload. Fails when no file has
    /// been configured or when the TOML is invalid — neither case
    /// touches the current live registry.
    pub async fn reload_capabilities(&self) -> Result<usize> {
        let path = match self.inner.capabilities_file.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let Some(path) = path else {
            bail!("capabilities config file is not configured");
        };
        let registry = crate::model_capabilities::ModelRegistry::load_from_file(&path)?;
        let count = registry.len();
        self.set_capabilities(Arc::new(registry)).await;
        Ok(count)
    }
}

fn test_reply_contexts_dir() -> PathBuf {
    std::env::temp_dir().join(format!("codex-bridge-reply-contexts-{}", Uuid::new_v4()))
}
