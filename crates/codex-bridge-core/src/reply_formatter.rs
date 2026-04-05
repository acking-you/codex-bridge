use crate::scheduler::TaskSummary;

/// Return a standard ack for started tasks.
pub fn format_started() -> String {
    "收到，开始处理。".to_string()
}

/// Return queue position message for an enqueued task.
pub fn format_enqueued(position: usize) -> String {
    format!("已进入队列，当前排队第 {position} 位。")
}

/// Return queue-capacity denial message.
pub fn format_queue_full() -> String {
    "当前任务过多，请稍后再试。".to_string()
}

/// Return failure response including cause.
pub fn format_failure(message: &str) -> String {
    format!("执行失败。\n原因：{message}")
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
