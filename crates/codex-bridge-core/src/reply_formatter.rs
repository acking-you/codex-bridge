use crate::scheduler::TaskSummary;

/// Return the persona-consistent private-chat ack for started tasks.
pub fn format_started_private() -> String {
    "欸、我先去看一下……稍等我一下。".to_string()
}

/// Return queue position message for an enqueued task.
pub fn format_enqueued(position: usize) -> String {
    format!("先排一下队……现在前面还有 {position} 个。")
}

/// Return queue-capacity denial message.
pub fn format_queue_full() -> String {
    "现在脑内线程真的满了……等我清一清再来吧。".to_string()
}

/// Return failure response including cause.
pub fn format_failure(message: &str) -> String {
    format!("欸，刚才那步翻车了。\n原因：{message}")
}

/// Return the help text describing trigger rules and local commands.
pub fn format_help() -> String {
    "触发方式：私聊默认触发，但非好友不会接入；群里需要先 @我。\n命令：/help /status /queue \
     /cancel /retry_last\n权限：全机可读，仅当前仓库可写，新文件只会写到 \
     .run/artifacts/，危险操作会被拒绝。"
        .to_string()
}

/// Return the private gate message for non-friends.
pub fn format_friend_gate() -> String {
    "那个……先加个好友吧。没加好友的私聊这边不会直接接入。".to_string()
}

/// Return the fallback message when a turn finishes without any reply skill
/// output.
pub fn format_missing_skill_reply() -> String {
    "已经处理完了，但这次没有生成可回传的结果。".to_string()
}

/// Return the message shown when cancel is requested successfully.
pub fn format_cancel_requested() -> String {
    "收到，我去把这条任务拦下来……等它停住。".to_string()
}

/// Return the message shown when the caller tries to cancel another user's
/// task.
pub fn format_cancel_denied() -> String {
    "这条任务不是你发起的，我不能替你按停。".to_string()
}

/// Return the message shown when the caller has no retryable task in context.
pub fn format_retry_missing() -> String {
    "当前会话里没有你可以重试的失败任务。".to_string()
}

/// Return `/status` style task summary lines.
pub fn format_status(
    running: Option<&TaskSummary>,
    queue_len: usize,
    last: Option<&TaskSummary>,
) -> String {
    let running_line = match running {
        Some(task) => format!("当前任务：{} ({})", task.task_id, task.conversation_key),
        None => "当前任务：无".to_string(),
    };
    let queue_line = format!("排队数量：{queue_len}");
    let last_line = match last {
        Some(task) => format!(
            "最近结果：{} {}",
            task.task_id,
            task.summary.clone().unwrap_or_else(|| "无摘要".to_string())
        ),
        None => "最近结果：无".to_string(),
    };
    [running_line, queue_line, last_line].join("\n")
}
