//! Task orchestration and command handling for QQ message events.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use codex_app_server_protocol::TurnStatus;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    admin_approval::{PendingApproval, PendingApprovalError, PendingApprovalPool},
    codex_runtime::{ActiveTurn, CodexExecutor, CodexTurnResult, TurnProgressSink},
    message_router::{CommandRequest, ControlCommand, MessageRouter, RouteDecision, TaskRequest},
    reply_context::ActiveReplyContext,
    reply_formatter,
    scheduler::{Scheduler, TaskQueueError, TaskState},
    service::ServiceState,
    state_store::{ConversationBinding, StateStore, TaskStatus},
};

/// Abstract sink used by the orchestrator to send formatted replies.
#[async_trait]
pub trait ReplySink: Send + Sync {
    /// Send a private user message.
    async fn send_private(&self, user_id: i64, text: String) -> Result<()>;
    /// Send a group message.
    async fn send_group(&self, group_id: i64, text: String) -> Result<()>;
}

/// Runtime-only configuration for reply-token issuance and group start
/// feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorConfig {
    /// Maximum number of queued tasks allowed to wait.
    pub queue_capacity: usize,
    /// Repository root exposed to the active reply context.
    pub repo_root: PathBuf,
    /// Artifact root exposed to the active reply context.
    pub artifacts_dir: PathBuf,
    /// Runtime-owned prompt file exposed via operator status.
    pub prompt_file: PathBuf,
    /// QQ emoji id used for the group "salute" reaction.
    pub group_start_reaction_emoji_id: String,
    /// QQ identifier allowed to bypass approval for task execution.
    pub admin_user_id: i64,
    /// Maximum number of pending approvals held in memory.
    pub pending_approval_capacity: usize,
    /// Expiration timeout for pending approvals.
    pub approval_timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct ScheduledRuntimeTask {
    persisted_task_id: Option<String>,
    task: TaskRequest,
}

impl ScheduledRuntimeTask {
    fn fresh(task: TaskRequest) -> Self {
        Self {
            persisted_task_id: None,
            task,
        }
    }

    fn persisted(task_id: String, task: TaskRequest) -> Self {
        Self {
            persisted_task_id: Some(task_id),
            task,
        }
    }
}

#[derive(Debug)]
struct ServiceReplySink {
    state: ServiceState,
}

#[async_trait]
impl ReplySink for ServiceReplySink {
    async fn send_private(&self, user_id: i64, text: String) -> Result<()> {
        self.state
            .send_private_message(user_id, text)
            .await
            .map(|_| ())
    }

    async fn send_group(&self, group_id: i64, text: String) -> Result<()> {
        self.state
            .send_group_message(group_id, text)
            .await
            .map(|_| ())
    }
}

/// Handle a single routing decision in an injectable way for tests.
pub async fn handle_route_decision(
    decision: RouteDecision,
    codex: &dyn CodexExecutor,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
) -> Result<()> {
    handle_route_decision_with_store(decision, codex, replies, scheduler, None).await
}

/// Handle a single routing decision with optional persistent state backing.
pub async fn handle_route_decision_with_store(
    decision: RouteDecision,
    codex: &dyn CodexExecutor,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    state_store: Option<&Mutex<StateStore>>,
) -> Result<()> {
    match decision {
        RouteDecision::Command(command_request) => {
            handle_command(command_request, replies, scheduler).await
        },
        RouteDecision::Task(task) => {
            handle_task(task, codex, replies, scheduler, state_store).await
        },
    }
}

async fn handle_command(
    command: CommandRequest,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
) -> Result<()> {
    let (is_group, target_id) = (command.is_group, command.reply_target_id);
    let text = match command.command {
        ControlCommand::Help => reply_formatter::format_help(),
        ControlCommand::Status {
            task_id: _,
        } => reply_formatter::format_status(
            scheduler.running(),
            scheduler.queue_len(),
            scheduler.last_terminal(),
            &[],
        ),
        ControlCommand::Queue => scheduler.queue_preview(),
        ControlCommand::Cancel => {
            let _ = scheduler.cancel_running();
            "当前任务已取消。".to_string()
        },
        ControlCommand::RetryLast => scheduler
            .retry_candidate(&command.conversation_key, command.source_sender_id)
            .map(|task| format!("已重新排队：{}", task.task_id))
            .unwrap_or_else(|| "当前会话没有可重试的失败任务。".to_string()),
        ControlCommand::Approve {
            task_id,
        } => format!("审批能力尚未接入：{task_id}"),
        ControlCommand::Deny {
            task_id,
        } => format!("审批能力尚未接入：{task_id}"),
        ControlCommand::Clear => "上下文管理能力尚未接入。".to_string(),
        ControlCommand::Compact => "上下文管理能力尚未接入。".to_string(),
    };

    send_reply(replies, is_group, target_id, text).await
}

async fn handle_task(
    task: TaskRequest,
    codex: &dyn CodexExecutor,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    state_store: Option<&Mutex<StateStore>>,
) -> Result<()> {
    if scheduler.running().is_some() {
        match scheduler.enqueue(
            task.source_message_id.to_string(),
            task.conversation_key.clone(),
            task.source_sender_id,
            task.source_message_id,
        ) {
            Ok(position) => {
                return send_reply(
                    replies,
                    task.is_group,
                    task.reply_target_id,
                    reply_formatter::format_enqueued(position),
                )
                .await;
            },
            Err(TaskQueueError::QueueFull) => {
                return send_reply(
                    replies,
                    task.is_group,
                    task.reply_target_id,
                    reply_formatter::format_queue_full(),
                )
                .await;
            },
        }
    }

    let binding = match resolve_binding(&task, codex, state_store).await {
        Ok(binding) => binding,
        Err(error) => {
            return finish_failed_task(
                replies,
                scheduler,
                task.is_group,
                task.reply_target_id,
                &error.to_string(),
            )
            .await;
        },
    };

    let task_id = if let Some(store) = state_store {
        let store = store.lock().await;
        store.insert_task_with_source(
            &binding,
            TaskStatus::Running,
            task.source_sender_id,
            task.source_message_id,
        )?
    } else {
        task.source_message_id.to_string()
    };
    scheduler
        .start_task(&task_id, &task.conversation_key, task.source_sender_id, task.source_message_id)
        .map_err(|error| anyhow!("failed to start task: {error:?}"))?;
    send_reply(
        replies,
        task.is_group,
        task.reply_target_id,
        reply_formatter::format_started_private(),
    )
    .await?;

    let result = match codex.run_turn(&binding.thread_id, &task.source_text).await {
        Ok(result) => result,
        Err(error) => {
            if let Some(store) = state_store {
                let store = store.lock().await;
                store.update_task_status(&task_id, TaskStatus::Failed)?;
            }
            return finish_failed_task(
                replies,
                scheduler,
                task.is_group,
                task.reply_target_id,
                &error.to_string(),
            )
            .await;
        },
    };
    if let Some(store) = state_store {
        let store = store.lock().await;
        store.update_task_status(&task_id, to_store_task_status(&result.status))?;
    }

    let task_state = map_turn_state(&result.status);
    let summary = summarize_turn_result(&result);
    scheduler.finish_running(task_state, summary.clone());

    let final_reply = summary.unwrap_or_else(|| "执行完成。".to_string());
    send_reply(replies, task.is_group, task.reply_target_id, final_reply).await
}

async fn resolve_binding(
    task: &TaskRequest,
    codex: &dyn CodexExecutor,
    state_store: Option<&Mutex<StateStore>>,
) -> Result<ConversationBinding> {
    let existing_binding = match state_store {
        Some(store) => {
            let store = store.lock().await;
            store.binding(&task.conversation_key)?
        },
        None => None,
    };
    let existing_thread_id = existing_binding
        .as_ref()
        .map(|binding| binding.thread_id.as_str());
    let thread_id = codex
        .ensure_thread(&task.conversation_key, existing_thread_id)
        .await?;

    let (binding, should_update_binding) = match existing_binding {
        Some(mut binding) => {
            let mut should_update = false;
            if binding.thread_id != thread_id {
                binding.thread_id = thread_id.clone();
                should_update = true;
            }
            (binding, should_update)
        },
        None => (
            ConversationBinding {
                conversation_key: task.conversation_key.clone(),
                thread_id: thread_id.clone(),
            },
            true,
        ),
    };

    if should_update_binding {
        if let Some(store) = state_store {
            let store = store.lock().await;
            store.upsert_binding(&binding)?;
        }
    }

    Ok(binding)
}

async fn finish_failed_task(
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    is_group: bool,
    reply_target_id: i64,
    error_message: &str,
) -> Result<()> {
    let summary = reply_formatter::format_failure(error_message);
    scheduler.finish_running(TaskState::Failed, Some(summary.clone()));
    send_reply(replies, is_group, reply_target_id, summary).await
}

fn summarize_turn_result(result: &CodexTurnResult) -> Option<String> {
    if let Some(reply) = result
        .final_reply
        .clone()
        .filter(|reply| !reply.trim().is_empty())
    {
        return Some(reply);
    }
    match result.status {
        TurnStatus::Interrupted => {
            Some("任务因服务重启或异常中断。可使用 /retry_last 重试。".to_string())
        },
        TurnStatus::Failed => result.error_message.clone(),
        _ => None,
    }
}

fn map_turn_state(status: &TurnStatus) -> TaskState {
    match status {
        TurnStatus::Completed => TaskState::Completed,
        TurnStatus::Failed => TaskState::Failed,
        TurnStatus::Interrupted => TaskState::Interrupted,
        TurnStatus::InProgress => TaskState::Completed,
    }
}

#[derive(Debug)]
struct TaskExecutionOutcome {
    task_state: TaskState,
    summary: Option<String>,
    final_reply: Option<String>,
}

#[derive(Debug)]
struct ActiveRuntimeTask {
    task: TaskRequest,
    active_turn: ActiveTurn,
    handle: tokio::task::JoinHandle<Result<TaskExecutionOutcome>>,
}

struct RuntimeTaskDeps<'a> {
    replies: &'a dyn ReplySink,
    state: &'a ServiceState,
    codex: Arc<dyn CodexExecutor>,
    state_store: Arc<Mutex<StateStore>>,
    config: &'a OrchestratorConfig,
}

struct RuntimeCommandDeps<'a> {
    state: &'a ServiceState,
    replies: &'a dyn ReplySink,
    scheduler: &'a mut Scheduler,
    pending_tasks: &'a mut VecDeque<ScheduledRuntimeTask>,
    retryable_tasks: &'a mut HashMap<String, TaskRequest>,
    pending_approvals: &'a mut PendingApprovalPool,
    active_task: Option<&'a ActiveRuntimeTask>,
    codex: &'a Arc<dyn CodexExecutor>,
    state_store: &'a Arc<Mutex<StateStore>>,
    config: &'a OrchestratorConfig,
}

struct RuntimeProgressSink {
    state: ServiceState,
    state_store: Arc<Mutex<StateStore>>,
    task_id: String,
}

#[async_trait]
impl TurnProgressSink for RuntimeProgressSink {
    async fn update_recent_output(&self, recent_output: Vec<String>) -> Result<()> {
        let task_id = self.task_id.clone();
        self.state
            .update_task_snapshot(move |snapshot| {
                let matches_running_task = snapshot.running_task_id.is_none()
                    || snapshot.running_task_id.as_deref() == Some(task_id.as_str());
                if matches_running_task {
                    snapshot.recent_output = recent_output;
                }
            })
            .await;
        Ok(())
    }

    async fn commit_output(&self, text: String) -> Result<()> {
        let store = self.state_store.lock().await;
        store.append_task_output(&self.task_id, &text, 4)
    }
}

async fn execute_task_for_runtime(
    codex: Arc<dyn CodexExecutor>,
    state: ServiceState,
    state_store: Arc<Mutex<StateStore>>,
    task_id: String,
    active_turn: ActiveTurn,
) -> Result<TaskExecutionOutcome> {
    let progress = RuntimeProgressSink {
        state,
        state_store: state_store.clone(),
        task_id: task_id.clone(),
    };
    let result = match codex
        .wait_for_turn_with_progress(&active_turn, Some(&progress))
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let store = state_store.lock().await;
            store.update_task_status(&task_id, TaskStatus::Failed)?;
            let final_reply = format!("执行失败。原因：{error}");
            return Ok(TaskExecutionOutcome {
                task_state: TaskState::Failed,
                summary: Some(final_reply.clone()),
                final_reply: Some(final_reply),
            });
        },
    };

    {
        let store = state_store.lock().await;
        store.update_task_status(&task_id, to_store_task_status(&result.status))?;
    }

    let summary = summarize_turn_result(&result);
    let final_reply = if matches!(result.status, TurnStatus::Failed | TurnStatus::Interrupted) {
        summary.clone()
    } else {
        None
    };
    Ok(TaskExecutionOutcome {
        task_state: map_turn_state(&result.status),
        summary,
        final_reply,
    })
}

async fn start_runtime_task(
    scheduled: ScheduledRuntimeTask,
    scheduler: &mut Scheduler,
    deps: RuntimeTaskDeps<'_>,
    already_promoted: bool,
) -> Result<ActiveRuntimeTask> {
    let RuntimeTaskDeps {
        replies,
        state,
        codex,
        state_store,
        config: runtime_config,
    } = deps;
    let task = scheduled.task;
    let binding = resolve_binding(&task, codex.as_ref(), Some(state_store.as_ref())).await?;
    info!(
        conversation = %task.conversation_key,
        sender_id = task.source_sender_id,
        message_id = task.source_message_id,
        reply_target_id = task.reply_target_id,
        is_group = task.is_group,
        thread_id = %binding.thread_id,
        "starting orchestrated task"
    );
    let persisted_task_id = if let Some(task_id) = scheduled.persisted_task_id {
        let store = state_store.lock().await;
        store.bind_task_to_thread(&task_id, &binding, TaskStatus::Running)?;
        task_id
    } else {
        let store = state_store.lock().await;
        store.insert_task_with_source(
            &binding,
            TaskStatus::Running,
            task.source_sender_id,
            task.source_message_id,
        )?
    };
    if !already_promoted {
        scheduler
            .start_task(
                &persisted_task_id,
                &task.conversation_key,
                task.source_sender_id,
                task.source_message_id,
            )
            .map_err(|error| anyhow!("failed to start task: {error:?}"))?;
    }
    send_start_feedback(state, replies, &task, &runtime_config.group_start_reaction_emoji_id)
        .await?;
    let active_turn = match codex
        .start_turn(&binding.thread_id, &task.source_text)
        .await
    {
        Ok(active_turn) => active_turn,
        Err(error) => {
            let store = state_store.lock().await;
            store.update_task_status(&persisted_task_id, TaskStatus::Failed)?;
            return Err(error);
        },
    };
    let reply_token = Uuid::new_v4().to_string();
    info!(
        task_id = %persisted_task_id,
        conversation = %task.conversation_key,
        reply_token = %reply_token,
        "reply context activated"
    );
    state
        .activate_reply_context(ActiveReplyContext {
            token: reply_token.clone(),
            conversation_key: task.conversation_key.clone(),
            is_group: task.is_group,
            reply_target_id: task.reply_target_id,
            source_message_id: task.source_message_id,
            source_sender_id: task.source_sender_id,
            source_sender_name: task.source_sender_name.clone(),
            repo_root: runtime_config.repo_root.clone(),
            artifacts_dir: runtime_config.artifacts_dir.clone(),
        })
        .await?;
    let handle = tokio::spawn(execute_task_for_runtime(
        codex,
        state.clone(),
        state_store,
        persisted_task_id,
        active_turn.clone(),
    ));
    Ok(ActiveRuntimeTask {
        task,
        active_turn,
        handle,
    })
}

async fn enqueue_runtime_task(
    scheduled: ScheduledRuntimeTask,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    pending_tasks: &mut VecDeque<ScheduledRuntimeTask>,
) -> Result<()> {
    let task = scheduled.task.clone();
    let queue_task_id = scheduled
        .persisted_task_id
        .clone()
        .unwrap_or_else(|| task.source_message_id.to_string());
    match scheduler.enqueue(
        queue_task_id,
        task.conversation_key.clone(),
        task.source_sender_id,
        task.source_message_id,
    ) {
        Ok(position) => {
            info!(
                conversation = %task.conversation_key,
                sender_id = task.source_sender_id,
                position,
                "task enqueued"
            );
            pending_tasks.push_back(scheduled);
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_enqueued(position),
            )
            .await
        },
        Err(TaskQueueError::QueueFull) => {
            warn!(
                conversation = %task.conversation_key,
                sender_id = task.source_sender_id,
                "task rejected because queue is full"
            );
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_queue_full(),
            )
            .await
        },
    }
}

async fn send_start_feedback(
    state: &ServiceState,
    replies: &dyn ReplySink,
    task: &TaskRequest,
    group_start_reaction_emoji_id: &str,
) -> Result<()> {
    if task.is_group {
        state
            .set_message_reaction(task.source_message_id, group_start_reaction_emoji_id.to_string())
            .await
    } else {
        send_reply(replies, false, task.reply_target_id, reply_formatter::format_started_private())
            .await
    }
}

fn is_admin_task(task: &TaskRequest, admin_user_id: i64) -> bool {
    task.source_sender_id == admin_user_id
}

async fn private_sender_is_friend(state: &ServiceState, task: &TaskRequest) -> bool {
    state
        .friends()
        .await
        .into_iter()
        .any(|friend| friend.user_id == task.source_sender_id)
}

async fn register_pending_approval(
    task: TaskRequest,
    replies: &dyn ReplySink,
    pending_approvals: &mut PendingApprovalPool,
    state_store: &Arc<Mutex<StateStore>>,
    approval_timeout_secs: u64,
    admin_user_id: i64,
) -> Result<()> {
    let task_id = Uuid::new_v4().to_string();
    let pending = PendingApproval::new(
        task_id.clone(),
        task.clone(),
        Instant::now(),
        Duration::from_secs(approval_timeout_secs),
    );
    match pending_approvals.insert(pending.clone()) {
        Ok(()) => {},
        Err(PendingApprovalError::ConversationAlreadyWaiting) => {
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_waiting_for_admin_approval_duplicate(),
            )
            .await?;
            return Ok(());
        },
        Err(PendingApprovalError::PoolFull) => {
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_queue_full(),
            )
            .await?;
            return Ok(());
        },
    }

    {
        let store = state_store.lock().await;
        if let Err(error) = store.insert_task_pending_approval_with_id(
            &task_id,
            &task.conversation_key,
            task.source_sender_id,
            task.source_message_id,
        ) {
            let _ = pending_approvals.take(&task_id);
            return Err(error);
        }
    }

    send_reply(
        replies,
        task.is_group,
        task.reply_target_id,
        reply_formatter::format_waiting_for_admin_approval(),
    )
    .await?;
    replies
        .send_private(admin_user_id, reply_formatter::format_admin_approval_notice(&pending))
        .await?;
    replies
        .send_private(
            admin_user_id,
            reply_formatter::format_admin_approve_command(&pending.task_id),
        )
        .await?;
    replies
        .send_private(
            admin_user_id,
            reply_formatter::format_admin_deny_command(&pending.task_id),
        )
        .await?;
    replies
        .send_private(
            admin_user_id,
            reply_formatter::format_admin_status_command(&pending.task_id),
        )
        .await
}

async fn handle_runtime_command(
    command: CommandRequest,
    deps: RuntimeCommandDeps<'_>,
) -> Result<Option<ScheduledRuntimeTask>> {
    let RuntimeCommandDeps {
        state,
        replies,
        scheduler,
        pending_tasks,
        retryable_tasks,
        pending_approvals,
        active_task,
        codex,
        state_store,
        config,
    } = deps;
    let is_admin_private = !command.is_group && command.source_sender_id == config.admin_user_id;
    match command.command {
        ControlCommand::Help => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                "received help command"
            );
            send_reply(
                replies,
                command.is_group,
                command.reply_target_id,
                reply_formatter::format_help(),
            )
            .await?;
            Ok(None)
        },
        _ if !is_admin_private => {
            send_reply(
                replies,
                command.is_group,
                command.reply_target_id,
                reply_formatter::format_admin_only_command(),
            )
            .await?;
            Ok(None)
        },
        ControlCommand::Status {
            task_id,
        } => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                "received status command"
            );
            let running_snapshot = state.task_snapshot().await;
            let text = if let Some(task_id) = task_id {
                if let Some(pending) = pending_approvals.get(&task_id) {
                    reply_formatter::format_admin_approval_notice(pending)
                } else {
                    let store = state_store.lock().await;
                    let recent_output = if running_snapshot.running_task_id.as_deref()
                        == Some(task_id.as_str())
                    {
                        running_snapshot.recent_output.clone()
                    } else {
                        store.recent_task_output(&task_id, 4)?
                    };
                    store
                        .task_by_id(&task_id)?
                        .map(|task| reply_formatter::format_task_status(&task, &recent_output))
                        .unwrap_or_else(|| reply_formatter::format_admin_task_not_found(&task_id))
                }
            } else {
                reply_formatter::format_status(
                    scheduler.running(),
                    scheduler.queue_len(),
                    scheduler.last_terminal(),
                    &running_snapshot.recent_output,
                )
            };
            send_reply(replies, command.is_group, command.reply_target_id, text).await?;
            Ok(None)
        },
        ControlCommand::Queue => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                "received queue command"
            );
            let text = scheduler.queue_preview();
            send_reply(replies, command.is_group, command.reply_target_id, text).await?;
            Ok(None)
        },
        ControlCommand::Cancel => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                "received cancel command"
            );
            let text = if let Some(active_task) = active_task {
                if command.source_sender_id != 0
                    && command.source_sender_id != active_task.task.source_sender_id
                {
                    reply_formatter::format_cancel_denied()
                } else {
                    codex
                        .interrupt(
                            &active_task.active_turn.thread_id,
                            &active_task.active_turn.turn_id,
                        )
                        .await?;
                    reply_formatter::format_cancel_requested()
                }
            } else {
                "当前没有正在执行的任务。".to_string()
            };
            send_reply(replies, command.is_group, command.reply_target_id, text).await?;
            Ok(None)
        },
        ControlCommand::RetryLast => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                "received retry-last command"
            );
            let retry_candidate = if command.source_sender_id == 0 {
                scheduler.retry_candidate_any_owner(&command.conversation_key)
            } else {
                scheduler.retry_candidate(&command.conversation_key, command.source_sender_id)
            };
            let Some(task) =
                retry_candidate.and_then(|summary| retryable_tasks.get(&summary.task_id).cloned())
            else {
                send_reply(
                    replies,
                    command.is_group,
                    command.reply_target_id,
                    reply_formatter::format_retry_missing(),
                )
                .await?;
                return Ok(None);
            };

            if active_task.is_some() {
                enqueue_runtime_task(
                    ScheduledRuntimeTask::fresh(task),
                    replies,
                    scheduler,
                    pending_tasks,
                )
                .await?;
                Ok(None)
            } else {
                Ok(Some(ScheduledRuntimeTask::fresh(task)))
            }
        },
        ControlCommand::Approve {
            task_id,
        } => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                %task_id,
                "received approve command"
            );
            let Some(pending) = pending_approvals.take(&task_id) else {
                send_reply(
                    replies,
                    false,
                    command.reply_target_id,
                    reply_formatter::format_admin_task_not_found(&task_id),
                )
                .await?;
                return Ok(None);
            };
            let scheduled = ScheduledRuntimeTask::persisted(task_id.clone(), pending.task.clone());
            if active_task.is_some() {
                {
                    let store = state_store.lock().await;
                    store.update_task_status(&task_id, TaskStatus::Queued)?;
                }
                enqueue_runtime_task(scheduled, replies, scheduler, pending_tasks).await?;
            } else {
                send_reply(
                    replies,
                    false,
                    command.reply_target_id,
                    reply_formatter::format_admin_approved(&task_id),
                )
                .await?;
                return Ok(Some(scheduled));
            }
            send_reply(
                replies,
                false,
                command.reply_target_id,
                reply_formatter::format_admin_approved(&task_id),
            )
            .await?;
            Ok(None)
        },
        ControlCommand::Deny {
            task_id,
        } => {
            info!(
                conversation = %command.conversation_key,
                sender_id = command.source_sender_id,
                %task_id,
                "received deny command"
            );
            let Some(pending) = pending_approvals.take(&task_id) else {
                send_reply(
                    replies,
                    false,
                    command.reply_target_id,
                    reply_formatter::format_admin_task_not_found(&task_id),
                )
                .await?;
                return Ok(None);
            };
            {
                let store = state_store.lock().await;
                store.update_task_status(&task_id, TaskStatus::Denied)?;
            }
            send_reply(
                replies,
                pending.task.is_group,
                pending.task.reply_target_id,
                reply_formatter::format_approval_denied(),
            )
            .await?;
            send_reply(
                replies,
                false,
                command.reply_target_id,
                reply_formatter::format_admin_denied(&task_id),
            )
            .await?;
            Ok(None)
        },
        ControlCommand::Clear => {
            send_reply(
                replies,
                command.is_group,
                command.reply_target_id,
                "上下文管理能力尚未接入。".to_string(),
            )
            .await?;
            Ok(None)
        },
        ControlCommand::Compact => {
            send_reply(
                replies,
                command.is_group,
                command.reply_target_id,
                "上下文管理能力尚未接入。".to_string(),
            )
            .await?;
            Ok(None)
        },
    }
}

/// Run the routing loop for events and control commands.
pub async fn run(
    state: ServiceState,
    mut control_rx: mpsc::Receiver<crate::service::ServiceCommand>,
    codex: Arc<dyn CodexExecutor>,
    state_store: Arc<Mutex<StateStore>>,
    config: OrchestratorConfig,
) -> Result<()> {
    let mut event_rx = state.subscribe_events();
    let mut router = MessageRouter::new();
    let mut scheduler = Scheduler::new(config.queue_capacity);
    let mut pending_tasks: VecDeque<ScheduledRuntimeTask> = VecDeque::new();
    let mut pending_approvals = PendingApprovalPool::new(config.pending_approval_capacity);
    let mut retryable_tasks: HashMap<String, TaskRequest> = HashMap::new();
    let mut last_retryable_conversation_key: Option<String> = None;
    let mut active_task: Option<ActiveRuntimeTask> = None;
    let mut approval_tick = tokio::time::interval(Duration::from_secs(1));
    let replies = ServiceReplySink {
        state: state.clone(),
    };
    recover_running_tasks(&state_store, &mut scheduler).await?;
    refresh_snapshot(
        &state,
        &scheduler,
        &config.prompt_file,
        last_retryable_conversation_key.as_deref(),
    )
    .await?;

    loop {
        tokio::select! {
            task_result = async {
                let active_task = active_task.as_mut()?;
                let handle = &mut active_task.handle;
                Some(handle.await)
            }, if active_task.is_some() => {
                let current_task = active_task.take().ok_or_else(|| anyhow!("missing active task context"))?;
                let joined = task_result.ok_or_else(|| anyhow!("missing active task result"))?;
                let skill_reply_count = state.current_reply_sent_count().await;
                state.deactivate_reply_context().await?;

                match joined {
                    Ok(Ok(outcome)) => {
                        info!(
                            conversation = %current_task.task.conversation_key,
                            message_id = current_task.task.source_message_id,
                            task_state = ?outcome.task_state,
                            skill_reply_count,
                            "task finished"
                        );
                        scheduler.finish_running(outcome.task_state, outcome.summary.clone());
                        if matches!(outcome.task_state, TaskState::Failed | TaskState::Interrupted) {
                            retryable_tasks.insert(
                                current_task.task.source_message_id.to_string(),
                                current_task.task.clone(),
                            );
                            last_retryable_conversation_key = Some(current_task.task.conversation_key.clone());
                        }
                        if let Some(reply_text) = outcome.final_reply.or_else(|| {
                            if matches!(outcome.task_state, TaskState::Completed)
                                && skill_reply_count == 0
                            {
                                Some(reply_formatter::format_missing_skill_reply())
                            } else {
                                None
                            }
                        }) {
                            send_reply(
                                &replies,
                                current_task.task.is_group,
                                current_task.task.reply_target_id,
                                reply_text,
                            )
                            .await?;
                        } else {
                            info!(
                                conversation = %current_task.task.conversation_key,
                                "task completed with skill-driven reply already sent"
                            );
                        }
                    },
                    Ok(Err(error)) => {
                        warn!(
                            conversation = %current_task.task.conversation_key,
                            error = %error,
                            "task failed before producing terminal outcome"
                        );
                        let message = format!("执行失败。原因：{error}");
                        scheduler.finish_running(TaskState::Failed, Some(message.clone()));
                        retryable_tasks.insert(
                            current_task.task.source_message_id.to_string(),
                            current_task.task.clone(),
                        );
                        last_retryable_conversation_key = Some(current_task.task.conversation_key.clone());
                        send_reply(
                            &replies,
                            current_task.task.is_group,
                            current_task.task.reply_target_id,
                            message,
                        )
                        .await?;
                    },
                    Err(error) => {
                        warn!(
                            conversation = %current_task.task.conversation_key,
                            error = %error,
                            "task join failed"
                        );
                        let message = format!("执行失败。原因：后台任务异常退出：{error}");
                        scheduler.finish_running(TaskState::Failed, Some(message.clone()));
                        retryable_tasks.insert(
                            current_task.task.source_message_id.to_string(),
                            current_task.task.clone(),
                        );
                        last_retryable_conversation_key = Some(current_task.task.conversation_key.clone());
                        send_reply(
                            &replies,
                            current_task.task.is_group,
                            current_task.task.reply_target_id,
                            message,
                        )
                        .await?;
                    },
                }

                if scheduler.running().is_some() {
                    let Some(next_task) = pending_tasks.pop_front() else {
                        return Err(anyhow!("scheduler promoted queued task without pending payload"));
                    };
                    active_task = Some(
                        start_runtime_task(
                            next_task,
                            &mut scheduler,
                            RuntimeTaskDeps {
                                replies: &replies,
                                state: &state,
                                codex: codex.clone(),
                                state_store: state_store.clone(),
                                config: &config,
                            },
                            true,
                        )
                        .await?,
                    );
                }
            },
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        if let Some(decision) = router.route_event(event) {
                            match decision {
                                RouteDecision::Command(command_request) => {
                                    if let Some(task) = handle_runtime_command(
                                        command_request,
                                        RuntimeCommandDeps {
                                            state: &state,
                                            replies: &replies,
                                            scheduler: &mut scheduler,
                                            pending_tasks: &mut pending_tasks,
                                            retryable_tasks: &mut retryable_tasks,
                                            pending_approvals: &mut pending_approvals,
                                            active_task: active_task.as_ref(),
                                            codex: &codex,
                                            state_store: &state_store,
                                            config: &config,
                                        },
                                    )
                                    .await? {
                                        active_task = Some(
                                            start_runtime_task(
                                                task,
                                                &mut scheduler,
                                                RuntimeTaskDeps {
                                                    replies: &replies,
                                                    state: &state,
                                                    codex: codex.clone(),
                                                    state_store: state_store.clone(),
                                                    config: &config,
                                                },
                                                false,
                                            )
                                            .await?,
                                        );
                                    }
                                },
                                RouteDecision::Task(task) => {
                                    if !task.is_group
                                        && !is_admin_task(&task, config.admin_user_id)
                                        && !private_sender_is_friend(&state, &task).await
                                    {
                                        info!(
                                            conversation = %task.conversation_key,
                                            sender_id = task.source_sender_id,
                                            "private message rejected by friend gate"
                                        );
                                        send_reply(
                                            &replies,
                                            false,
                                            task.reply_target_id,
                                            reply_formatter::format_friend_gate(),
                                        )
                                        .await?;
                                    } else if !is_admin_task(&task, config.admin_user_id) {
                                        register_pending_approval(
                                            task,
                                            &replies,
                                            &mut pending_approvals,
                                            &state_store,
                                            config.approval_timeout_secs,
                                            config.admin_user_id,
                                        )
                                        .await?;
                                    } else if active_task.is_some() {
                                        enqueue_runtime_task(
                                            ScheduledRuntimeTask::fresh(task),
                                            &replies,
                                            &mut scheduler,
                                            &mut pending_tasks,
                                        )
                                        .await?;
                                    } else {
                                        active_task = Some(
                                            start_runtime_task(
                                                ScheduledRuntimeTask::fresh(task),
                                                &mut scheduler,
                                                RuntimeTaskDeps {
                                                    replies: &replies,
                                                    state: &state,
                                                    codex: codex.clone(),
                                                    state_store: state_store.clone(),
                                                    config: &config,
                                                },
                                                false,
                                            )
                                            .await?,
                                        );
                                    }
                                },
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            },
            command = control_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                let Some(command_request) = service_command_to_router_command(command) else {
                    continue;
                };
                if let Some(task) = handle_runtime_command(
                    command_request,
                    RuntimeCommandDeps {
                        state: &state,
                        replies: &replies,
                        scheduler: &mut scheduler,
                        pending_tasks: &mut pending_tasks,
                        retryable_tasks: &mut retryable_tasks,
                        pending_approvals: &mut pending_approvals,
                        active_task: active_task.as_ref(),
                        codex: &codex,
                        state_store: &state_store,
                        config: &config,
                    },
                )
                .await? {
                    active_task = Some(
                        start_runtime_task(
                            task,
                            &mut scheduler,
                            RuntimeTaskDeps {
                                replies: &replies,
                                state: &state,
                                codex: codex.clone(),
                                state_store: state_store.clone(),
                                config: &config,
                            },
                            false,
                        )
                        .await?,
                    );
                }
            },
            _ = approval_tick.tick() => {
                let expired = pending_approvals.take_expired(Instant::now());
                for pending in expired {
                    {
                        let store = state_store.lock().await;
                        store.update_task_status(&pending.task_id, TaskStatus::Expired)?;
                    }
                    send_reply(
                        &replies,
                        pending.task.is_group,
                        pending.task.reply_target_id,
                        reply_formatter::format_approval_expired(),
                    )
                    .await?;
                }
            },
        }
        refresh_snapshot(
            &state,
            &scheduler,
            &config.prompt_file,
            last_retryable_conversation_key.as_deref(),
        )
        .await?;
    }
    Ok(())
}

fn service_command_to_router_command(
    command: crate::service::ServiceCommand,
) -> Option<CommandRequest> {
    match command {
        crate::service::ServiceCommand::Control {
            command,
        } => Some(command),
        _ => None,
    }
}

async fn refresh_snapshot(
    state: &ServiceState,
    scheduler: &Scheduler,
    prompt_file: &std::path::Path,
    last_retryable_conversation_key: Option<&str>,
) -> Result<()> {
    let previous = state.task_snapshot().await;
    let running = scheduler.running().cloned();
    let running_task_id = running.as_ref().map(|task| task.task_id.clone());
    let running_conversation_key = running.as_ref().map(|task| task.conversation_key.clone());
    let recent_output = match (running_task_id.as_deref(), previous.running_task_id.as_deref()) {
        (Some(current), Some(previous_task)) if current == previous_task => previous.recent_output,
        (None, _) => Vec::new(),
        _ => Vec::new(),
    };

    let running = scheduler.running();
    let last_terminal_summary = scheduler
        .last_terminal()
        .and_then(|task| task.summary.clone());
    let snapshot = crate::service::TaskSnapshot {
        running_task_id,
        running_conversation_key,
        running_summary: running.and_then(|task| task.summary.clone()),
        recent_output,
        queue_len: scheduler.queue_len(),
        last_terminal_summary,
        last_retryable_conversation_key: last_retryable_conversation_key.map(str::to_string),
        prompt_file: Some(prompt_file.display().to_string()),
    };
    state.set_task_snapshot(snapshot).await;
    Ok(())
}

fn to_store_task_status(status: &TurnStatus) -> TaskStatus {
    match status {
        TurnStatus::Completed => TaskStatus::Completed,
        TurnStatus::Failed => TaskStatus::Failed,
        TurnStatus::Interrupted => TaskStatus::Interrupted,
        TurnStatus::InProgress => TaskStatus::Completed,
    }
}

async fn recover_running_tasks(
    state_store: &Arc<Mutex<StateStore>>,
    scheduler: &mut Scheduler,
) -> Result<()> {
    let (interrupted, expired) = {
        let store = state_store.lock().await;
        (store.mark_running_tasks_interrupted()?, store.mark_pending_tasks_expired()?)
    };
    if interrupted > 0 {
        scheduler.record_terminal_state(
            "recover",
            "system",
            0,
            0,
            TaskState::Interrupted,
            Some(format!("系统重启后恢复了 {interrupted} 个未完成运行态任务。")),
        );
    }
    if expired > 0 {
        scheduler.record_terminal_state(
            "approval-expired",
            "system",
            0,
            0,
            TaskState::Interrupted,
            Some(format!("系统重启后作废了 {expired} 个待审批任务。")),
        );
    }
    Ok(())
}

async fn send_reply(
    sink: &dyn ReplySink,
    is_group: bool,
    target_id: i64,
    text: String,
) -> Result<()> {
    if is_group {
        sink.send_group(target_id, text).await
    } else {
        sink.send_private(target_id, text).await
    }
}
