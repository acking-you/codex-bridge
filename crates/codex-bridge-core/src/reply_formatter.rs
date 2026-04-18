use crate::{admin_approval::PendingApproval, scheduler::TaskSummary, state_store::TaskRecord};

/// Return the persona-consistent private-chat ack for started tasks.
pub fn format_started_private() -> String {
    "欸、我先去看一下……稍等我一下喔～(ง •̀_•́)ง".to_string()
}

/// Return queue position message for an enqueued task.
pub fn format_enqueued(position: usize) -> String {
    format!("先排一下队嘛……前面还有 {position} 个呢 _(:3」∠)_")
}

/// Return queue-capacity denial message.
pub fn format_queue_full() -> String {
    "呜、现在脑内线程真的满了……等我清一清再来嘛～(╥﹏╥)".to_string()
}

/// Return lane-capacity denial message for one busy conversation.
pub fn format_lane_full() -> String {
    "这个群（或私聊）里我已经攒了太多待处理消息啦……先等我把前面的做完嘛 (´；ω；`)".to_string()
}

/// Return failure response including cause.
pub fn format_failure(message: &str) -> String {
    format!("欸……刚才那步翻车了 (´；ω；`)\n原因：{message}")
}

/// Return the help text describing trigger rules and local commands.
pub fn format_help() -> String {
    "触发方式：私聊默认会进来，但不是好友的私聊我接不到喔；群里要先 @我 才行。\
     主人在私聊或群里 @我 就能用管理命令；群聊里其他人发起的任务仍然要等主人点头。\n\
     命令：/help /status /queue /cancel /retry_last /clear /compact /approve <task_id> /deny \
     <task_id>\n权限：全机可读，仅当前仓库可写，新文件只写到 .run/artifacts/，危险操作会被拒绝 _(:3」∠)_"
        .to_string()
}

/// Return the message shown when the caller must wait for admin approval.
pub fn format_waiting_for_admin_approval() -> String {
    "这件事要先等主人点头……等主人确认下来，我再继续呀 (´・ω・`)".to_string()
}

/// Return the message shown when a group request must wait for a salute
/// reaction.
pub fn format_waiting_for_admin_group_approval() -> String {
    "这件事要先等主人点头……请主人对原消息点个敬礼的表情嘛 (｡•ㅅ•｡)".to_string()
}

/// Return the message shown when one conversation is already waiting for
/// approval.
pub fn format_waiting_for_admin_approval_duplicate() -> String {
    "这段会话已经有一条在等主人确认了，先别一下子塞太多给我嘛……(＞ω＜)".to_string()
}

/// Return the message shown when a non-admin user attempts an admin-only
/// command.
pub fn format_admin_only_command() -> String {
    "这个命令只开放给主人……抱歉呐 (´・ω・`)".to_string()
}

/// Return the message shown when current conversation context is cleared.
pub fn format_clear_success() -> String {
    "当前会话的上下文清空啦～下次从新线程开始喔 (๑•̀ㅂ•́)و✧".to_string()
}

/// Return the message shown when there is no current conversation context.
pub fn format_clear_missing() -> String {
    "当前会话没有可以清空的上下文呐 (´・ω・`)".to_string()
}

/// Return the message shown when compaction is started.
pub fn format_compact_started() -> String {
    "已经发起当前会话的上下文压缩啦 _(:3」∠)_".to_string()
}

/// Return the message shown when there is no thread to compact.
pub fn format_compact_missing() -> String {
    "当前会话还没有可压缩的上下文呐～".to_string()
}

/// Return the message shown when current conversation is busy.
pub fn format_compact_busy() -> String {
    "当前会话正在跑任务呢；先等它结束，或者 /cancel 停掉 (＞ω＜)".to_string()
}

/// Return the message shown when compaction failed unexpectedly.
pub fn format_compact_failed() -> String {
    "唔……上下文压缩没成功，稍后再试一次嘛，或者先 /clear 再重开对话 (╥﹏╥)".to_string()
}

/// Return the message shown when a waiting task is denied.
pub fn format_approval_denied() -> String {
    "主人这次没有点头，所以这条请求我先不执行啦 (´・ω・`)".to_string()
}

/// Return the message shown when a waiting task expires.
pub fn format_approval_expired() -> String {
    "这条请求等主人确认等太久了，已经自动作废呐……(╥﹏╥)".to_string()
}

/// Render the admin-facing approval notice for one pending task.
pub fn format_admin_approval_notice(pending: &PendingApproval) -> String {
    format!(
        "待审批任务：{}\n来源：{}\n发起人：{} ({})\n消息：{}\n下面三条命令会分开发，直接复制其中一条就行。",
        pending.task_id,
        if pending.task.is_group { "群聊" } else { "私聊" },
        pending.task.source_sender_name,
        pending.task.source_sender_id,
        pending.task.source_text,
    )
}

/// Render the admin-facing approval notice for one pending group task.
pub fn format_admin_group_approval_notice(pending: &PendingApproval) -> String {
    format!(
        "群待审批任务：{}\n群号：{}\n发起人：{} ({})\n消息：{}\n批准方式：请对原群消息点敬礼表情。\n可选管理：/status {} /deny {}",
        pending.task_id,
        pending.task.reply_target_id,
        pending.task.source_sender_name,
        pending.task.source_sender_id,
        pending.task.source_text,
        pending.task_id,
        pending.task_id,
    )
}

/// Render the admin command used to approve a waiting task.
pub fn format_admin_approve_command(task_id: &str) -> String {
    format!("/approve {task_id}")
}

/// Render the admin command used to deny a waiting task.
pub fn format_admin_deny_command(task_id: &str) -> String {
    format!("/deny {task_id}")
}

/// Render the admin command used to inspect a task.
pub fn format_admin_status_command(task_id: &str) -> String {
    format!("/status {task_id}")
}

/// Render the admin-facing result after approving a task.
pub fn format_admin_approved(task_id: &str) -> String {
    format!("任务已批准啦～{task_id} ✨")
}

/// Render the admin-facing message when a group pending task must use
/// reaction approval.
pub fn format_group_approval_use_reaction() -> String {
    "群聊待审批任务不能用 /approve 呀，要对原群消息点个敬礼表情才行喔～".to_string()
}

/// Render the admin-facing result after denying a task.
pub fn format_admin_denied(task_id: &str) -> String {
    format!("任务已拒绝：{task_id}")
}

/// Render the admin-facing message when one task id cannot be found.
pub fn format_admin_task_not_found(task_id: &str) -> String {
    format!("没有找到这个任务呐：{task_id} (´・ω・`)")
}

/// Render the admin-facing status view for a persisted task.
pub fn format_task_status(task: &TaskRecord, recent_output: &[String]) -> String {
    let header = format!(
        "任务：{}\n状态：{:?}\n会话：{}\n发起人：{}\n源消息：{}",
        task.task_id,
        task.status,
        task.conversation_key,
        task.owner_sender_id,
        task.source_message_id
    );
    match format_recent_output_section(recent_output) {
        Some(section) => format!("{header}\n{section}"),
        None => header,
    }
}

/// Return the private gate message for non-friends.
pub fn format_friend_gate() -> String {
    "那个……先加个好友嘛，不是好友的私聊这边接不进来呐 (´・ω・`)".to_string()
}

/// Return the fallback message when a turn finishes without any reply skill
/// output.
pub fn format_missing_skill_reply() -> String {
    "已经处理完了，但这次没有生成可以回传的结果呐……(´-ω-`)".to_string()
}

/// Return the message shown when cancel is requested successfully.
pub fn format_cancel_requested() -> String {
    "收到～我去把这条任务拦下来，等它停住 (ง •̀_•́)ง".to_string()
}

/// Return the message shown when a cancel command could not interrupt the
/// running turn (for example when Codex restarted and lost the turn state).
pub fn format_cancel_failed() -> String {
    "取消失败了……稍后再试一次嘛；要是还卡住，可以 /clear 再重开对话 (╥﹏╥)".to_string()
}

/// Return the message shown when the caller tries to cancel another user's
/// task.
pub fn format_cancel_denied() -> String {
    "这条任务不是你发起的呐，我不能替你按停 (´・ω・`)".to_string()
}

/// Return the message shown when the caller has no retryable task in context.
pub fn format_retry_missing() -> String {
    "当前会话里没有你可以重试的失败任务呢～".to_string()
}

/// Return `/status` style task summary lines.
pub fn format_status(
    running: Option<&TaskSummary>,
    queue_len: usize,
    last: Option<&TaskSummary>,
    recent_output: &[String],
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
    let mut sections = vec![running_line, queue_line, last_line];
    if let Some(section) = format_recent_output_section(recent_output) {
        sections.push(section);
    }
    sections.join("\n")
}

fn format_recent_output_section(recent_output: &[String]) -> Option<String> {
    if recent_output.is_empty() {
        return None;
    }

    let entries = recent_output
        .iter()
        .enumerate()
        .map(|(index, text)| format!("{}. {}", index + 1, text))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("最近输出：\n{entries}"))
}

#[cfg(test)]
mod cancel_failed_text_tests {
    use super::format_cancel_failed;

    #[test]
    fn cancel_failed_text_mentions_retry_guidance() {
        let text = format_cancel_failed();
        assert!(text.contains("取消"));
        assert!(text.contains("稍后"));
    }
}
