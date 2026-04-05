//! Task orchestration and command handling for QQ message events.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use codex_app_server_protocol::TurnStatus;
use tokio::sync::mpsc;

use crate::{
    codex_runtime::{CodexExecutor, CodexTurnResult},
    events::NormalizedEvent,
    message_router::{CommandRequest, ControlCommand, MessageRouter, RouteDecision, TaskRequest},
    reply_formatter,
    scheduler::{Scheduler, TaskQueueError, TaskState},
    service::ServiceState,
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
    match decision {
        RouteDecision::Command(command_request) => {
            handle_command(command_request, replies, scheduler).await
        },
        RouteDecision::Task(task) => handle_task(task, codex, replies, scheduler).await,
    }
}

async fn handle_command(
    command: CommandRequest,
    replies: &dyn ReplySink,
    scheduler: &mut Scheduler,
) -> Result<()> {
    let (is_group, target_id) = (command.is_group, command.reply_target_id);
    let text = match command.command {
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
            .retry_candidate(&command.conversation_key)
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
) -> Result<()> {
    let task_id = task.source_message_id.to_string();
    if scheduler.running().is_some() {
        match scheduler.enqueue(task_id, task.conversation_key.clone()) {
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

    scheduler
        .start_task(&task_id, &task.conversation_key)
        .map_err(|error| anyhow!("failed to start task: {error:?}"))?;
    send_reply(replies, task.is_group, task.reply_target_id, reply_formatter::format_started())
        .await?;

    let thread_id = codex.ensure_thread(&task.conversation_key, None).await?;
    let result = codex.run_turn(&thread_id, &task.source_text).await?;

    let task_state = map_turn_state(&result.status);
    let summary = summarize_turn_result(&result);
    scheduler.finish_running(task_state, summary.clone());

    let final_reply = summary.unwrap_or_else(|| "执行完成。".to_string());
    send_reply(replies, task.is_group, task.reply_target_id, final_reply).await
}

fn summarize_turn_result(result: &CodexTurnResult) -> Option<String> {
    if let Some(reply) = result.final_reply.clone() {
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

/// Run the routing loop for events and control commands.
pub async fn run(
    state: ServiceState,
    mut event_rx: mpsc::Receiver<NormalizedEvent>,
    mut control_rx: mpsc::Receiver<crate::service::ServiceCommand>,
    codex: &dyn CodexExecutor,
) -> Result<()> {
    let mut router = MessageRouter::new();
    let mut scheduler = Scheduler::new(5);
    let replies = ServiceReplySink {
        state: state.clone(),
    };

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some(event) = event else {
                    break;
                };
                if let Some(decision) = router.route_event(event) {
                    handle_route_decision(decision, codex, &replies, &mut scheduler).await?;
                }
            },
            command = control_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                let Some(command_request) = service_command_to_router_command(command) else {
                    continue;
                };
                handle_route_decision(
                    RouteDecision::Command(command_request),
                    codex,
                    &replies,
                    &mut scheduler,
                )
                .await?;
            },
        }
        refresh_snapshot(&state, &scheduler).await;
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

async fn refresh_snapshot(state: &ServiceState, scheduler: &Scheduler) {
    let running = scheduler.running();
    let last_terminal_summary = scheduler
        .last_terminal()
        .and_then(|task| task.summary.clone());
    let snapshot = crate::service::TaskSnapshot {
        running_task_id: running.map(|task| task.task_id.clone()),
        running_conversation_key: running.map(|task| task.conversation_key.clone()),
        running_summary: running.and_then(|task| task.summary.clone()),
        queue_len: scheduler.queue_len(),
        last_terminal_summary,
        prompt_version: Some(SYSTEM_PROMPT_VERSION.to_string()),
    };
    state.set_task_snapshot(snapshot).await;
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
