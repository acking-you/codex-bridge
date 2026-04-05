//! Task orchestration and command handling for QQ message events.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use codex_app_server_protocol::TurnStatus;
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::{
    codex_runtime::{ActiveTurn, CodexExecutor, CodexTurnResult},
    message_router::{CommandRequest, ControlCommand, MessageRouter, RouteDecision, TaskRequest},
    reply_formatter,
    scheduler::{Scheduler, TaskQueueError, TaskState},
    service::ServiceState,
    state_store::{ConversationBinding, StateStore, TaskStatus},
    system_prompt::SYSTEM_PROMPT_VERSION,
};

/// Abstract sink used by the orchestrator to send formatted replies.
#[async_trait]
pub trait ReplySink: Send + Sync {
    /// Send a private user message.
    async fn send_private(&self, user_id: i64, text: String) -> Result<()>;
    /// Send a group message.
    async fn send_group(&self, group_id: i64, text: String) -> Result<()>;
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
        ControlCommand::Help => "可用命令：/help /status /queue /cancel /retry_last".to_string(),
        ControlCommand::Status => reply_formatter::format_status(
            scheduler.running(),
            scheduler.queue_len(),
            scheduler.last_terminal(),
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
    send_reply(replies, task.is_group, task.reply_target_id, reply_formatter::format_started())
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
            if binding.prompt_version.trim().is_empty() {
                binding.prompt_version = SYSTEM_PROMPT_VERSION.to_string();
                should_update = true;
            }
            (binding, should_update)
        },
        None => (
            ConversationBinding {
                conversation_key: task.conversation_key.clone(),
                thread_id: thread_id.clone(),
                prompt_version: SYSTEM_PROMPT_VERSION.to_string(),
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
    let summary = format!("执行失败。原因：{error_message}");
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
    final_reply: String,
}

#[derive(Debug)]
struct ActiveRuntimeTask {
    task: TaskRequest,
    active_turn: ActiveTurn,
    handle: tokio::task::JoinHandle<Result<TaskExecutionOutcome>>,
}

async fn execute_task_for_runtime(
    codex: Arc<dyn CodexExecutor>,
    state_store: Arc<Mutex<StateStore>>,
    task_id: String,
    active_turn: ActiveTurn,
) -> Result<TaskExecutionOutcome> {
    let result = match codex.wait_for_turn(&active_turn).await {
        Ok(result) => result,
        Err(error) => {
            let store = state_store.lock().await;
            store.update_task_status(&task_id, TaskStatus::Failed)?;
            let final_reply = format!("执行失败。原因：{error}");
            return Ok(TaskExecutionOutcome {
                task_state: TaskState::Failed,
                summary: Some(final_reply.clone()),
                final_reply,
            });
        },
    };

    {
        let store = state_store.lock().await;
        store.update_task_status(&task_id, to_store_task_status(&result.status))?;
    }

    let summary = summarize_turn_result(&result);
    let final_reply = summary.clone().unwrap_or_else(|| "执行完成。".to_string());
    Ok(TaskExecutionOutcome {
        task_state: map_turn_state(&result.status),
        summary,
        final_reply,
    })
}

async fn start_runtime_task(
    task: TaskRequest,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    codex: Arc<dyn CodexExecutor>,
    state_store: Arc<Mutex<StateStore>>,
    already_promoted: bool,
) -> Result<ActiveRuntimeTask> {
    let binding = resolve_binding(&task, codex.as_ref(), Some(state_store.as_ref())).await?;
    let persisted_task_id = {
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
    send_reply(replies, task.is_group, task.reply_target_id, reply_formatter::format_started())
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
    let handle = tokio::spawn(execute_task_for_runtime(
        codex,
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
    task: TaskRequest,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    pending_tasks: &mut VecDeque<TaskRequest>,
) -> Result<()> {
    match scheduler.enqueue(
        task.source_message_id.to_string(),
        task.conversation_key.clone(),
        task.source_sender_id,
        task.source_message_id,
    ) {
        Ok(position) => {
            pending_tasks.push_back(task.clone());
            send_reply(
                replies,
                task.is_group,
                task.reply_target_id,
                reply_formatter::format_enqueued(position),
            )
            .await
        },
        Err(TaskQueueError::QueueFull) => {
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

async fn handle_runtime_command(
    command: CommandRequest,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
    pending_tasks: &mut VecDeque<TaskRequest>,
    retryable_tasks: &mut HashMap<String, TaskRequest>,
    active_task: Option<&ActiveRuntimeTask>,
    codex: &Arc<dyn CodexExecutor>,
) -> Result<Option<TaskRequest>> {
    match command.command {
        ControlCommand::Help => {
            send_reply(
                replies,
                command.is_group,
                command.reply_target_id,
                "可用命令：/help /status /queue /cancel /retry_last".to_string(),
            )
            .await?;
            Ok(None)
        },
        ControlCommand::Status => {
            let text = reply_formatter::format_status(
                scheduler.running(),
                scheduler.queue_len(),
                scheduler.last_terminal(),
            );
            send_reply(replies, command.is_group, command.reply_target_id, text).await?;
            Ok(None)
        },
        ControlCommand::Queue => {
            let text = scheduler.queue_preview();
            send_reply(replies, command.is_group, command.reply_target_id, text).await?;
            Ok(None)
        },
        ControlCommand::Cancel => {
            let text = if let Some(active_task) = active_task {
                codex
                    .interrupt(&active_task.active_turn.thread_id, &active_task.active_turn.turn_id)
                    .await?;
                "已请求取消当前任务，等待任务停止。".to_string()
            } else {
                "当前没有正在执行的任务。".to_string()
            };
            send_reply(replies, command.is_group, command.reply_target_id, text).await?;
            Ok(None)
        },
        ControlCommand::RetryLast => {
            let Some(task) = scheduler
                .retry_candidate(&command.conversation_key, command.source_sender_id)
                .and_then(|summary| retryable_tasks.get(&summary.task_id).cloned())
            else {
                send_reply(
                    replies,
                    command.is_group,
                    command.reply_target_id,
                    "当前会话没有可重试的失败任务。".to_string(),
                )
                .await?;
                return Ok(None);
            };

            if active_task.is_some() {
                enqueue_runtime_task(task, replies, scheduler, pending_tasks).await?;
                Ok(None)
            } else {
                Ok(Some(task))
            }
        },
    }
}

/// Run the routing loop for events and control commands.
pub async fn run(
    state: ServiceState,
    mut control_rx: mpsc::Receiver<crate::service::ServiceCommand>,
    codex: Arc<dyn CodexExecutor>,
    state_store: Arc<Mutex<StateStore>>,
    queue_capacity: usize,
) -> Result<()> {
    let mut event_rx = state.subscribe_events();
    let mut router = MessageRouter::new();
    let mut scheduler = Scheduler::new(queue_capacity);
    let mut pending_tasks: VecDeque<TaskRequest> = VecDeque::new();
    let mut retryable_tasks: HashMap<String, TaskRequest> = HashMap::new();
    let mut last_retryable_conversation_key: Option<String> = None;
    let mut active_task: Option<ActiveRuntimeTask> = None;
    let replies = ServiceReplySink {
        state: state.clone(),
    };
    recover_running_tasks(&state_store, &mut scheduler).await?;
    refresh_snapshot(&state, &scheduler, &state_store, last_retryable_conversation_key.as_deref())
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

                match joined {
                    Ok(Ok(outcome)) => {
                        scheduler.finish_running(outcome.task_state, outcome.summary.clone());
                        if matches!(outcome.task_state, TaskState::Failed | TaskState::Interrupted) {
                            retryable_tasks.insert(
                                current_task.task.source_message_id.to_string(),
                                current_task.task.clone(),
                            );
                            last_retryable_conversation_key = Some(current_task.task.conversation_key.clone());
                        }
                        send_reply(
                            &replies,
                            current_task.task.is_group,
                            current_task.task.reply_target_id,
                            outcome.final_reply,
                        )
                        .await?;
                    },
                    Ok(Err(error)) => {
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
                            &replies,
                            &mut scheduler,
                            codex.clone(),
                            state_store.clone(),
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
                                        &replies,
                                        &mut scheduler,
                                        &mut pending_tasks,
                                        &mut retryable_tasks,
                                        active_task.as_ref(),
                                        &codex,
                                    )
                                    .await? {
                                        active_task = Some(
                                            start_runtime_task(
                                                task,
                                                &replies,
                                                &mut scheduler,
                                                codex.clone(),
                                                state_store.clone(),
                                                false,
                                            )
                                            .await?,
                                        );
                                    }
                                },
                                RouteDecision::Task(task) => {
                                    if active_task.is_some() {
                                        enqueue_runtime_task(task, &replies, &mut scheduler, &mut pending_tasks).await?;
                                    } else {
                                        active_task = Some(
                                            start_runtime_task(
                                                task,
                                                &replies,
                                                &mut scheduler,
                                                codex.clone(),
                                                state_store.clone(),
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
                    &replies,
                    &mut scheduler,
                    &mut pending_tasks,
                    &mut retryable_tasks,
                    active_task.as_ref(),
                    &codex,
                )
                .await? {
                    active_task = Some(
                        start_runtime_task(
                            task,
                            &replies,
                            &mut scheduler,
                            codex.clone(),
                            state_store.clone(),
                            false,
                        )
                        .await?,
                    );
                }
            },
        }
        refresh_snapshot(
            &state,
            &scheduler,
            &state_store,
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
    state_store: &Arc<Mutex<StateStore>>,
    last_retryable_conversation_key: Option<&str>,
) -> Result<()> {
    let running = scheduler.running().cloned();
    let running_conversation_key = running.as_ref().map(|task| task.conversation_key.clone());
    let prompt_version = if let Some(conversation_key) = &running_conversation_key {
        let store = state_store.lock().await;
        store
            .binding(conversation_key)?
            .map(|binding| binding.prompt_version)
            .unwrap_or_else(|| SYSTEM_PROMPT_VERSION.to_string())
    } else {
        SYSTEM_PROMPT_VERSION.to_string()
    };

    let running = scheduler.running();
    let last_terminal_summary = scheduler
        .last_terminal()
        .and_then(|task| task.summary.clone());
    let snapshot = crate::service::TaskSnapshot {
        running_task_id: running.map(|task| task.task_id.clone()),
        running_conversation_key,
        running_summary: running.and_then(|task| task.summary.clone()),
        queue_len: scheduler.queue_len(),
        last_terminal_summary,
        last_retryable_conversation_key: last_retryable_conversation_key.map(str::to_string),
        prompt_version: Some(prompt_version),
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
    let interrupted = {
        let store = state_store.lock().await;
        store.mark_running_tasks_interrupted()?
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
