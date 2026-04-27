//! Orchestrator unit tests.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use codex_app_server_protocol::TurnStatus;
use codex_bridge_core::{
    codex_runtime::{ActiveTurn, CodexExecutor, CodexTurnResult},
    events::NormalizedEvent,
    message_router::{CommandRequest, ControlCommand, RouteDecision, TaskRequest},
    orchestrator::{
        self, handle_route_decision, handle_route_decision_with_store, OrchestratorConfig,
    },
    reply_formatter,
    scheduler::Scheduler,
    service::{FriendProfile, SendMessageReceipt, ServiceCommand, ServiceState},
    state_store::{ConversationBinding, StateStore, TaskStatus},
};
use tempfile::TempDir;
use tokio::{
    sync::{mpsc, Mutex as AsyncMutex, Notify},
    time::timeout,
};

#[derive(Debug)]
struct FakeCodexExecutor {
    thread_ids: AsyncMutex<VecDeque<String>>,
    ensure_thread_calls: AsyncMutex<Vec<(String, Option<String>)>>,
    interrupt_calls: AsyncMutex<Vec<(String, String)>>,
    compact_calls: AsyncMutex<Vec<String>>,
    interrupt_notify: Notify,
    turn_id: String,
    reply_text: String,
    progress_updates: Vec<String>,
    status: TurnStatus,
    wait_for_interrupt: bool,
    fail_interrupt: bool,
    agent_messages: Vec<String>,
}

impl FakeCodexExecutor {
    fn with_reply(thread_ids: Vec<&str>, turn_id: &str, reply_text: &str) -> Self {
        Self::with_status(thread_ids, turn_id, TurnStatus::Completed, reply_text)
    }

    fn with_status(
        thread_ids: Vec<&str>,
        turn_id: &str,
        status: TurnStatus,
        reply_text: &str,
    ) -> Self {
        Self {
            thread_ids: AsyncMutex::new(
                thread_ids
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            ),
            ensure_thread_calls: AsyncMutex::new(Vec::new()),
            interrupt_calls: AsyncMutex::new(Vec::new()),
            compact_calls: AsyncMutex::new(Vec::new()),
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: reply_text.to_string(),
            progress_updates: Vec::new(),
            status,
            wait_for_interrupt: false,
            fail_interrupt: false,
            agent_messages: Vec::new(),
        }
    }

    fn blocking(thread_ids: Vec<&str>, turn_id: &str) -> Self {
        Self {
            thread_ids: AsyncMutex::new(
                thread_ids
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            ),
            ensure_thread_calls: AsyncMutex::new(Vec::new()),
            interrupt_calls: AsyncMutex::new(Vec::new()),
            compact_calls: AsyncMutex::new(Vec::new()),
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: String::new(),
            progress_updates: Vec::new(),
            status: TurnStatus::Interrupted,
            wait_for_interrupt: true,
            fail_interrupt: false,
            agent_messages: Vec::new(),
        }
    }

    fn blocking_with_progress(
        thread_ids: Vec<&str>,
        turn_id: &str,
        progress_updates: Vec<&str>,
    ) -> Self {
        Self {
            thread_ids: AsyncMutex::new(
                thread_ids
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect(),
            ),
            ensure_thread_calls: AsyncMutex::new(Vec::new()),
            interrupt_calls: AsyncMutex::new(Vec::new()),
            compact_calls: AsyncMutex::new(Vec::new()),
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: String::new(),
            progress_updates: progress_updates.into_iter().map(str::to_string).collect(),
            status: TurnStatus::Interrupted,
            wait_for_interrupt: true,
            fail_interrupt: false,
            agent_messages: Vec::new(),
        }
    }

    fn with_agent_messages(
        thread_ids: Vec<&str>,
        turn_id: &str,
        agent_messages: Vec<&str>,
    ) -> Self {
        let mut fake = Self::with_reply(thread_ids, turn_id, "");
        fake.agent_messages = agent_messages.into_iter().map(str::to_string).collect();
        fake
    }

    fn with_failing_interrupt(mut self) -> Self {
        self.fail_interrupt = true;
        self
    }

    async fn ensure_thread_calls(&self) -> Vec<(String, Option<String>)> {
        self.ensure_thread_calls.lock().await.clone()
    }

    async fn interrupt_calls(&self) -> Vec<(String, String)> {
        self.interrupt_calls.lock().await.clone()
    }

    async fn compact_calls(&self) -> Vec<String> {
        self.compact_calls.lock().await.clone()
    }
}

#[async_trait]
impl CodexExecutor for FakeCodexExecutor {
    async fn ensure_thread(
        &self,
        conversation_key: &str,
        existing_thread_id: Option<&str>,
    ) -> Result<String> {
        self.ensure_thread_calls
            .lock()
            .await
            .push((conversation_key.to_string(), existing_thread_id.map(str::to_string)));
        let thread_id = self
            .thread_ids
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("no thread id configured"))?;
        Ok(thread_id)
    }

    async fn start_turn(&self, thread_id: &str, _input_text: &str) -> Result<ActiveTurn> {
        Ok(ActiveTurn { thread_id: thread_id.to_string(), turn_id: self.turn_id.clone() })
    }

    async fn wait_for_turn(&self, active_turn: &ActiveTurn) -> Result<CodexTurnResult> {
        if self.wait_for_interrupt {
            self.interrupt_notify.notified().await;
        }
        Ok(CodexTurnResult {
            thread_id: active_turn.thread_id.clone(),
            turn_id: self.turn_id.clone(),
            status: self.status.clone(),
            error_message: None,
            items: vec![],
            final_reply: Some(self.reply_text.clone()),
        })
    }

    async fn wait_for_turn_with_progress(
        &self,
        active_turn: &ActiveTurn,
        progress: Option<&dyn codex_bridge_core::codex_runtime::TurnProgressSink>,
    ) -> Result<CodexTurnResult> {
        if let Some(progress) = progress {
            for update in &self.progress_updates {
                progress.update_recent_output(vec![update.clone()]).await?;
                progress.commit_output(update.clone()).await?;
            }
            for message in &self.agent_messages {
                progress.commit_agent_message(message.clone()).await?;
            }
        }
        self.wait_for_turn(active_turn).await
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        self.interrupt_calls
            .lock()
            .await
            .push((thread_id.to_string(), turn_id.to_string()));
        if self.fail_interrupt {
            return Err(anyhow::anyhow!("simulated interrupt failure: codex restarted"));
        }
        self.interrupt_notify.notify_waiters();
        Ok(())
    }

    async fn compact_thread(&self, thread_id: &str) -> Result<()> {
        self.compact_calls.lock().await.push(thread_id.to_string());
        Ok(())
    }
}

#[derive(Default, Clone)]
struct FakeReplySink {
    messages: Arc<StdMutex<Vec<String>>>,
}

impl FakeReplySink {
    fn messages(&self) -> Vec<String> {
        self.messages.lock().expect("messages").clone()
    }
}

#[async_trait::async_trait]
impl codex_bridge_core::orchestrator::ReplySink for FakeReplySink {
    async fn send_private(&self, _user_id: i64, text: String) -> Result<()> {
        self.messages.lock().expect("messages").push(text);
        Ok(())
    }

    async fn send_group(&self, _group_id: i64, text: String) -> Result<()> {
        self.messages.lock().expect("messages").push(text);
        Ok(())
    }
}

fn make_task(source_message_id: i64, conversation_key: &str) -> TaskRequest {
    TaskRequest {
        conversation_key: conversation_key.to_string(),
        source_message_id,
        source_sender_id: 42,
        source_sender_name: "LB".to_string(),
        source_text: "修一下 README".to_string(),
        is_group: false,
        reply_target_id: 42,
        self_id: 99,
        quoted_message_id: None,
    }
}

fn make_private_event_from(
    sender_id: i64,
    sender_name: &str,
    message_id: i64,
    text: &str,
) -> NormalizedEvent {
    serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": message_id,
        "user_id": sender_id,
        "self_id": 2993013575i64,
        "sender": { "nickname": sender_name },
        "message": [{ "type": "text", "data": { "text": text } }]
    })
    .try_into()
    .expect("normalize private event")
}

fn make_private_event(message_id: i64, text: &str) -> NormalizedEvent {
    make_private_event_from(42, "LB", message_id, text)
}

fn make_group_event_from(
    sender_id: i64,
    sender_name: &str,
    group_id: i64,
    message_id: i64,
    text: &str,
) -> NormalizedEvent {
    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "message_id": message_id,
        "group_id": group_id,
        "user_id": sender_id,
        "self_id": 2993013575i64,
        "raw_message": format!("@bot {text}"),
        "message": [
            { "type": "at", "data": { "qq": "2993013575", "name": "bot" } },
            { "type": "text", "data": { "text": format!(" {text}") } }
        ],
        "sender": { "nickname": sender_name }
    });
    NormalizedEvent::try_from(raw).expect("normalize group event")
}

fn make_group_reaction_event(
    operator_id: i64,
    group_id: i64,
    message_id: i64,
    emoji_id: &str,
    is_add: bool,
) -> NormalizedEvent {
    serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_msg_emoji_like",
        "group_id": group_id,
        "user_id": operator_id,
        "message_id": message_id,
        "self_id": 2993013575i64,
        "likes": [{ "emoji_id": emoji_id, "count": 1 }],
        "is_add": is_add
    })
    .try_into()
    .expect("normalize reaction event")
}

fn make_command_request_from(sender_id: i64, command: ControlCommand) -> CommandRequest {
    CommandRequest {
        command,
        conversation_key: format!("private:{sender_id}"),
        reply_target_id: sender_id,
        is_group: false,
        source_message_id: 9001,
        source_sender_id: sender_id,
        source_sender_name: if sender_id == 42 { "LB".to_string() } else { "admin".to_string() },
    }
}

fn make_group_command_request(
    sender_id: i64,
    group_id: i64,
    command: ControlCommand,
) -> CommandRequest {
    CommandRequest {
        command,
        conversation_key: format!("group:{group_id}"),
        reply_target_id: group_id,
        is_group: true,
        source_message_id: 9100,
        source_sender_id: sender_id,
        source_sender_name: if sender_id == 2_394_626_220 {
            "admin".to_string()
        } else {
            "LB".to_string()
        },
    }
}

fn spawn_bridge_sink(
    mut command_rx: mpsc::Receiver<ServiceCommand>,
    sent_messages: Arc<StdMutex<Vec<String>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                ServiceCommand::SendPrivate { text, respond_to, .. } => {
                    sent_messages.lock().expect("messages").push(text);
                    let _ = respond_to.send(Ok(SendMessageReceipt { message_id: 1 }));
                },
                ServiceCommand::SendGroup { text, respond_to, .. } => {
                    sent_messages.lock().expect("messages").push(text);
                    let _ = respond_to.send(Ok(SendMessageReceipt { message_id: 1 }));
                },
                ServiceCommand::SetMessageReaction { message_id, emoji_id, respond_to } => {
                    sent_messages
                        .lock()
                        .expect("messages")
                        .push(format!("REACTION:{message_id}:{emoji_id}"));
                    let _ = respond_to.send(Ok(()));
                },
                ServiceCommand::SendOutbound { message, respond_to } => {
                    sent_messages.lock().expect("messages").push(format!(
                        "OUTBOUND:{}:{}",
                        match message.target {
                            codex_bridge_core::outbound::OutboundTarget::Private(id) => {
                                format!("private:{id}")
                            },
                            codex_bridge_core::outbound::OutboundTarget::Group(id) => {
                                format!("group:{id}")
                            },
                        },
                        message.segments.len()
                    ));
                    let _ = respond_to.send(Ok(SendMessageReceipt { message_id: 1 }));
                },
                ServiceCommand::Control { .. } => {},
                ServiceCommand::FetchMessage { respond_to, .. } => {
                    let _ =
                        respond_to.send(Err(anyhow::anyhow!("FetchMessage stubbed out in tests")));
                },
                ServiceCommand::FetchConversationHistory { respond_to, .. } => {
                    let _ = respond_to.send(Err(anyhow::anyhow!(
                        "FetchConversationHistory stubbed out in tests"
                    )));
                },
            }
        }
    })
}

fn runtime_config(repo_root: &std::path::Path) -> OrchestratorConfig {
    let artifacts_dir = repo_root.join(".run/artifacts");
    let prompt_file = repo_root.join(".run/default/prompt/persona.md");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    std::fs::create_dir_all(prompt_file.parent().expect("prompt dir")).expect("create prompt dir");
    std::fs::write(&prompt_file, "# Test persona\n\nprompt from test runtime")
        .expect("write persona file");
    OrchestratorConfig {
        queue_capacity: 5,
        lane_pending_capacity: 5,
        runtime_pool_size: 2,
        repo_root: repo_root.to_path_buf(),
        artifacts_dir,
        prompt_file,
        group_start_reaction_emoji_id: "282".to_string(),
        admin_user_id: 2_394_626_220,
        trusted_group_ids: Vec::new(),
        pending_approval_capacity: 32,
        approval_timeout_secs: 900,
    }
}

fn runtime_config_with_timeout(
    repo_root: &std::path::Path,
    approval_timeout_secs: u64,
) -> OrchestratorConfig {
    let mut config = runtime_config(repo_root);
    config.approval_timeout_secs = approval_timeout_secs;
    config
}

async fn wait_for_snapshot_prompt_file(state: &ServiceState) {
    timeout(Duration::from_secs(1), async {
        loop {
            if state.task_snapshot().await.prompt_file.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("orchestrator initialized");
}

#[tokio::test]
async fn task_request_sends_started_and_final_reply() {
    let codex = FakeCodexExecutor::with_reply(vec!["thr_123"], "turn_1", "已经处理完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5, 5);
    let task = make_task(1001, "private:42");

    handle_route_decision(RouteDecision::Task(task), &codex, &replies, &mut scheduler)
        .await
        .expect("handle task");

    let sent = replies.messages();
    assert_eq!(sent[0], reply_formatter::format_started_private());
    assert_eq!(sent[1], "已经处理完成");
}

#[tokio::test]
async fn task_request_persists_conversation_binding_when_missing() {
    let codex = FakeCodexExecutor::with_reply(vec!["thread-1"], "turn_1", "已完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5, 5);
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));

    handle_route_decision_with_store(
        RouteDecision::Task(make_task(1002, "private:17")),
        &codex,
        &replies,
        &mut scheduler,
        Some(store.as_ref()),
    )
    .await
    .expect("handle task");

    let binding = store
        .lock()
        .await
        .binding("private:17")
        .expect("query binding")
        .expect("binding exists");

    assert_eq!(binding.conversation_key, "private:17");
    assert_eq!(binding.thread_id, "thread-1");
}

#[tokio::test]
async fn task_request_reuses_binding_for_follow_up_task() {
    let codex = FakeCodexExecutor::with_reply(vec!["thread-2", "thread-2"], "turn_1", "已完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5, 5);
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "private:88".to_string(),
            thread_id: "thread-2".to_string(),
        })
        .expect("seed legacy binding");

    handle_route_decision_with_store(
        RouteDecision::Task(make_task(2001, "private:88")),
        &codex,
        &replies,
        &mut scheduler,
        Some(store.as_ref()),
    )
    .await
    .expect("handle task first");
    handle_route_decision_with_store(
        RouteDecision::Task(make_task(2002, "private:88")),
        &codex,
        &replies,
        &mut scheduler,
        Some(store.as_ref()),
    )
    .await
    .expect("handle task second");

    let calls = codex.ensure_thread_calls().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "private:88");
    assert_eq!(calls[0].1, Some("thread-2".to_string()));
    assert_eq!(calls[1].0, "private:88");
    assert_eq!(calls[1].1, Some("thread-2".to_string()));

    let binding = store
        .lock()
        .await
        .binding("private:88")
        .expect("query binding")
        .expect("binding exists");

    assert_eq!(binding.thread_id, "thread-2");
}

#[tokio::test]
async fn cancel_command_interrupts_active_turn() {
    let codex = Arc::new(FakeCodexExecutor::blocking(vec!["thread-9"], "turn-9"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));

    let tempdir = TempDir::new().expect("tempdir");
    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));

    wait_for_snapshot_prompt_file(&state).await;
    state.publish_event(make_private_event_from(2_394_626_220, "admin", 3001, "开始长任务"));

    timeout(Duration::from_secs(1), async {
        loop {
            if state
                .task_snapshot()
                .await
                .running_conversation_key
                .as_deref()
                == Some("private:2394626220")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task started");

    state
        .send_control_command(make_command_request_from(2_394_626_220, ControlCommand::Cancel))
        .await
        .expect("send cancel");

    timeout(Duration::from_secs(1), async {
        loop {
            if !codex.interrupt_calls().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("interrupt called");

    let interrupts = codex.interrupt_calls().await;
    assert_eq!(interrupts, vec![("thread-9".to_string(), "turn-9".to_string())]);

    timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = state.task_snapshot().await;
            if snapshot.running_task_id.is_none()
                && snapshot.last_terminal_summary.as_deref()
                    == Some("任务因服务重启或异常中断。可使用 /retry_last 重试。")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task interrupted");

    let messages = sent_messages.lock().expect("messages").clone();
    assert!(messages
        .iter()
        .any(|text| text == &reply_formatter::format_started_private()));
    assert!(messages
        .iter()
        .any(|text| text == &reply_formatter::format_cancel_requested()));

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_friend_private_message_is_rejected_before_codex() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-7"], "turn-7", "不会执行"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));

    state.set_friends(Vec::<FriendProfile>::new()).await;
    wait_for_snapshot_prompt_file(&state).await;
    state.publish_event(make_private_event(4001, "在吗"));

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_friend_gate())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("friend gate reply");

    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_group_message_is_approved_by_admin_salute_reaction() {
    let codex = Arc::new(FakeCodexExecutor::with_status(
        vec!["thread-8"],
        "turn-8",
        TurnStatus::Completed,
        "",
    ));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex,
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(42, "alice", 777, 5001, "生成一张图"));

    let pending = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(task) = store
                .lock()
                .await
                .latest_task_for_conversation("group:777")
                .expect("query group task")
            {
                break task;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("group task pending");

    assert!(
        sent_messages
            .lock()
            .expect("messages")
            .iter()
            .any(|text| text.contains("待审批任务：")),
        "admin private approval notice should still be sent for non-admin group requests"
    );

    assert_eq!(pending.status, TaskStatus::PendingApproval);
    state.publish_event(make_group_reaction_event(2_394_626_220, 777, 5001, "282", true));

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages.iter().any(|text| text == "REACTION:5001:282")
                && messages
                    .iter()
                    .any(|text| text == &reply_formatter::format_missing_skill_reply())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("group salute and fallback reply");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn approve_command_rejects_group_pending_task() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-unused", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(42, "LB", 777, 5101, "帮我跑一下"));
    let pending = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(task) = store
                .lock()
                .await
                .latest_task_for_conversation("group:777")
                .expect("query latest")
            {
                break task;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending task appears");

    state
        .send_control_command(make_command_request_from(
            2_394_626_220,
            ControlCommand::Approve { task_id: pending.task_id.clone() },
        ))
        .await
        .expect("approve command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("请对原群消息点敬礼表情"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approve correction reply");
    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_or_wrong_emoji_reaction_does_not_approve_group_task() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-unused", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(42, "LB", 777, 5201, "还在等审批"));
    timeout(Duration::from_secs(1), async {
        loop {
            if let Some(task) = store
                .lock()
                .await
                .latest_task_for_conversation("group:777")
                .expect("query latest")
            {
                if task.status == TaskStatus::PendingApproval {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending task appears");

    state.publish_event(make_group_reaction_event(42, 777, 5201, "282", true));
    state.publish_event(make_group_reaction_event(2_394_626_220, 777, 5201, "13", true));

    tokio::time::sleep(Duration::from_millis(100)).await;

    let task = store
        .lock()
        .await
        .latest_task_for_conversation("group:777")
        .expect("query latest task")
        .expect("pending task exists");
    assert_eq!(task.status, TaskStatus::PendingApproval);
    assert!(codex.ensure_thread_calls().await.is_empty());
    assert!(sent_messages
        .lock()
        .expect("messages")
        .iter()
        .all(|text| text != "REACTION:5201:282"));

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn admin_group_message_bypasses_approval() {
    let codex = Arc::new(FakeCodexExecutor::with_status(
        vec!["thread-admin-group"],
        "turn-admin-group",
        TurnStatus::Completed,
        "",
    ));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(
        2_394_626_220,
        "admin",
        778,
        5002,
        "直接执行这个任务",
    ));

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages.iter().any(|text| text == "REACTION:5002:282")
                && messages
                    .iter()
                    .any(|text| text == &reply_formatter::format_missing_skill_reply())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("admin group task executed without approval");

    let maybe_task = store
        .lock()
        .await
        .latest_task_for_conversation("group:778")
        .expect("query admin group task");
    let task = maybe_task.expect("admin group task exists");
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(
        !sent_messages
            .lock()
            .expect("messages")
            .iter()
            .any(|text| text.contains("待审批任务：")),
        "admin group requests must not generate approval notices"
    );
    assert_eq!(codex.ensure_thread_calls().await, vec![("group:778".to_string(), None)]);

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_task_in_trusted_group_bypasses_approval() {
    let codex = Arc::new(FakeCodexExecutor::with_status(
        vec!["thread-trusted-group"],
        "turn-trusted-group",
        TurnStatus::Completed,
        "",
    ));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    // Trust group 778 wholesale, so a NON-admin member's @bot task runs
    // without waiting for the admin salute reaction.
    let mut config = runtime_config(tempdir.path());
    config.trusted_group_ids = vec![778];

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        config,
    ));
    wait_for_snapshot_prompt_file(&state).await;

    // A plain member (not the admin) addresses the bot in the trusted group.
    state.publish_event(make_group_event_from(
        100_001, // non-admin sender
        "friend",
        778, // trusted group
        6001,
        "直接跑一下这个",
    ));

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages.iter().any(|text| text == "REACTION:6001:282")
                && messages
                    .iter()
                    .any(|text| text == &reply_formatter::format_missing_skill_reply())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("trusted-group task executed without approval");

    let maybe_task = store
        .lock()
        .await
        .latest_task_for_conversation("group:778")
        .expect("query trusted-group task");
    let task = maybe_task.expect("trusted-group task exists");
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(
        !sent_messages
            .lock()
            .expect("messages")
            .iter()
            .any(|text| text.contains("待审批任务：")),
        "trusted-group tasks must not generate approval notices"
    );
    assert_eq!(codex.ensure_thread_calls().await, vec![("group:778".to_string(), None)]);

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn admin_group_status_command_is_allowed() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-1"], "turn-1", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex,
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Status { task_id: None },
        ))
        .await
        .expect("status command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("当前任务"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("admin group status reply");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_group_status_command_is_rejected() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-1", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            42,
            777,
            ControlCommand::Status { task_id: None },
        ))
        .await
        .expect("status command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_admin_only_command())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("non-admin group status denied");
    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn compact_command_without_binding_reports_missing_context() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-1", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Compact,
        ))
        .await
        .expect("compact command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_compact_missing())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact missing-context reply");
    assert!(codex.compact_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn clear_command_removes_current_conversation_binding_and_next_turn_uses_new_thread() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-reset"], "turn-reset", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "group:777".to_string(),
            thread_id: "thread-old".to_string(),
        })
        .expect("seed binding");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(2_394_626_220, 777, ControlCommand::Clear))
        .await
        .expect("clear command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_clear_success())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("clear reply");

    assert!(store
        .lock()
        .await
        .binding("group:777")
        .expect("query cleared binding")
        .is_none());

    state.publish_event(make_group_event_from(2_394_626_220, "admin", 777, 9101, "清空后重新开始"));

    timeout(Duration::from_secs(1), async {
        loop {
            if !codex.ensure_thread_calls().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("next task should start");

    let calls = codex.ensure_thread_calls().await;
    assert_eq!(calls[0], ("group:777".to_string(), None));

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn compact_command_starts_thread_compaction_when_idle() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-unused"], "turn-1", "ok"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "group:777".to_string(),
            thread_id: "thread-compact".to_string(),
        })
        .expect("seed binding");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Compact,
        ))
        .await
        .expect("compact command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_compact_started())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact started reply");
    assert_eq!(codex.compact_calls().await, vec!["thread-compact".to_string()]);

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn compact_command_for_running_conversation_reports_busy() {
    let codex = Arc::new(FakeCodexExecutor::blocking(vec!["thread-busy"], "turn-busy"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "group:777".to_string(),
            thread_id: "thread-busy".to_string(),
        })
        .expect("seed binding");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(2_394_626_220, "admin", 777, 9201, "执行中的群任务"));

    timeout(Duration::from_secs(1), async {
        loop {
            if state
                .task_snapshot()
                .await
                .running_conversation_key
                .as_deref()
                == Some("group:777")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("group task running");

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Compact,
        ))
        .await
        .expect("compact command");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_compact_busy())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compact busy reply");
    assert!(codex.compact_calls().await.is_empty());

    state
        .send_control_command(make_group_command_request(
            2_394_626_220,
            777,
            ControlCommand::Cancel,
        ))
        .await
        .expect("cancel running task");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn non_admin_friend_private_message_waits_for_admin_approval() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-10"], "turn-10", "已批准执行"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;
    state
        .set_friends(vec![FriendProfile { user_id: 42, nickname: "LB".to_string(), remark: None }])
        .await;

    state.publish_event(make_private_event(6001, "帮我跑个任务"));

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages
                .iter()
                .any(|text| text == &reply_formatter::format_waiting_for_admin_approval())
                && messages.iter().any(|text| text.contains("待审批任务："))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval notices delivered");

    assert!(codex.ensure_thread_calls().await.is_empty());

    let pending = store
        .lock()
        .await
        .latest_task_for_conversation("private:42")
        .expect("query pending task")
        .expect("pending task exists");
    assert_eq!(pending.status, TaskStatus::PendingApproval);
    let messages = sent_messages.lock().expect("messages").clone();
    assert!(messages
        .iter()
        .any(|text| text == &format!("/approve {}", pending.task_id)));
    assert!(messages
        .iter()
        .any(|text| text == &format!("/deny {}", pending.task_id)));
    assert!(messages
        .iter()
        .any(|text| text == &format!("/status {}", pending.task_id)));

    state
        .send_control_command(make_command_request_from(
            2_394_626_220,
            ControlCommand::Approve { task_id: pending.task_id.clone() },
        ))
        .await
        .expect("approve pending task");

    timeout(Duration::from_secs(1), async {
        loop {
            if !codex.ensure_thread_calls().await.is_empty()
                && sent_messages
                    .lock()
                    .expect("messages")
                    .iter()
                    .any(|text| text == &reply_formatter::format_missing_skill_reply())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approved task executed");

    let completed = store
        .lock()
        .await
        .task_by_id(&pending.task_id)
        .expect("query completed task")
        .expect("completed task exists");
    assert_eq!(completed.status, TaskStatus::Completed);

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn admin_status_shows_recent_live_output_for_running_task() {
    let codex = Arc::new(FakeCodexExecutor::blocking_with_progress(
        vec!["thread-live"],
        "turn-live",
        vec!["先定位 reply formatter", "现在补 status 输出"],
    ));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_group_event_from(
        2_394_626_220,
        "admin",
        900,
        7001,
        "执行一个会慢一点的任务",
    ));

    timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = state.task_snapshot().await;
            if snapshot
                .recent_output
                .iter()
                .any(|line| line.contains("现在补 status 输出"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("recent output should appear in snapshot");

    state
        .send_control_command(make_command_request_from(
            2_394_626_220,
            ControlCommand::Status { task_id: None },
        ))
        .await
        .expect("request status");

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("最近输出") && text.contains("现在补 status 输出"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("status output should include live task text");

    state
        .send_control_command(make_command_request_from(2_394_626_220, ControlCommand::Cancel))
        .await
        .expect("cancel running task");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn runtime_task_sends_each_agent_message_without_missing_result_fallback() {
    let codex = Arc::new(FakeCodexExecutor::with_agent_messages(
        vec!["thread-agent-output"],
        "turn-agent-output",
        vec!["我先看一下。", "查完了，是配置问题。"],
    ));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex,
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_private_event_from(
        2_394_626_220,
        "admin",
        8101,
        "帮我看一下这个问题",
    ));

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages.iter().any(|text| text == "我先看一下。")
                && messages.iter().any(|text| text == "查完了，是配置问题。")
            {
                assert!(
                    !messages
                        .iter()
                        .any(|text| text == &reply_formatter::format_missing_skill_reply()),
                    "agent text output should count as a delivered reply"
                );
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("agent messages delivered");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn duplicate_pending_approval_from_same_conversation_is_rejected() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-11"], "turn-11", "不会执行"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;
    state
        .set_friends(vec![FriendProfile { user_id: 42, nickname: "LB".to_string(), remark: None }])
        .await;

    state.publish_event(make_private_event(6101, "第一条"));
    state.publish_event(make_private_event(6102, "第二条"));

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_waiting_for_admin_approval_duplicate())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("duplicate approval rejection");

    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn pending_approval_expires_without_admin_reply() {
    let codex = Arc::new(FakeCodexExecutor::with_reply(vec!["thread-12"], "turn-12", "不会执行"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store.clone(),
        runtime_config_with_timeout(tempdir.path(), 1),
    ));
    wait_for_snapshot_prompt_file(&state).await;
    state
        .set_friends(vec![FriendProfile { user_id: 42, nickname: "LB".to_string(), remark: None }])
        .await;

    state.publish_event(make_private_event(6201, "等审批超时"));

    timeout(Duration::from_secs(3), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == &reply_formatter::format_approval_expired())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending approval expired");

    let expired = store
        .lock()
        .await
        .latest_task_for_conversation("private:42")
        .expect("query expired task")
        .expect("expired task exists");
    assert_eq!(expired.status, TaskStatus::Expired);
    assert!(codex.ensure_thread_calls().await.is_empty());

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn cancel_command_replies_gracefully_when_interrupt_fails() {
    let codex =
        Arc::new(FakeCodexExecutor::blocking(vec!["thread-x"], "turn-x").with_failing_interrupt());
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    // publish an event that will become the running task (admin bypasses approval
    // gate)
    state.publish_event(make_private_event_from(2_394_626_220, "admin", 9301, "长跑任务"));

    // wait until the task is running
    timeout(Duration::from_secs(1), async {
        loop {
            if state.task_snapshot().await.running_task_id.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task running");

    // send a Cancel command from the same sender (admin, 2_394_626_220)
    state
        .send_control_command(make_command_request_from(2_394_626_220, ControlCommand::Cancel))
        .await
        .expect("cancel command");

    // assert: the user eventually receives a cancel-failed reply
    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text.contains("取消失败"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancel failed reply");

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn send_reply_best_effort_swallows_error_with_log() {
    struct FailingSink;
    #[async_trait::async_trait]
    impl codex_bridge_core::orchestrator::ReplySink for FailingSink {
        async fn send_private(&self, _user_id: i64, _text: String) -> Result<()> {
            Err(anyhow::anyhow!("transport closed"))
        }
        async fn send_group(&self, _group_id: i64, _text: String) -> Result<()> {
            Err(anyhow::anyhow!("transport closed"))
        }
    }
    // The function returns (), so absence of panic + no `?` == swallowed error.
    codex_bridge_core::orchestrator::send_reply_best_effort(&FailingSink, false, 1, "hi".into())
        .await;
    codex_bridge_core::orchestrator::send_reply_best_effort(&FailingSink, true, 2, "hi".into())
        .await;
}

#[tokio::test]
async fn distinct_conversations_run_their_turns_concurrently() {
    // Both tasks block in wait_for_turn until interrupt_notify fires. If the
    // orchestrator still serialised tasks at the global level, the second
    // start_turn would never happen until the first finishes — so the second
    // ensure_thread_calls entry would never be observed within the timeout.
    let codex =
        Arc::new(FakeCodexExecutor::blocking(vec!["thread-priv", "thread-group"], "turn-x"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages.clone());
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");

    let run_handle = tokio::spawn(orchestrator::run(
        state.clone(),
        control_rx,
        codex.clone(),
        store,
        runtime_config(tempdir.path()),
    ));
    wait_for_snapshot_prompt_file(&state).await;

    // Admin private chat — bypasses friend gate and approval.
    state.publish_event(make_private_event_from(2_394_626_220, "admin", 9401, "私聊长跑"));
    // Admin group chat — bypasses approval too.
    state.publish_event(make_group_event_from(2_394_626_220, "admin", 7777, 9402, "群里也长跑"));

    timeout(Duration::from_secs(2), async {
        loop {
            if codex.ensure_thread_calls().await.len() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both ensure_thread calls observed; concurrency works");

    let calls = codex.ensure_thread_calls().await;
    let conversations: std::collections::HashSet<&String> =
        calls.iter().map(|(key, _)| key).collect();
    assert!(conversations.contains(&"private:2394626220".to_string()));
    assert!(conversations.contains(&"group:7777".to_string()));

    run_handle.abort();
    bridge_handle.abort();
}

#[tokio::test]
async fn runtime_pool_limit_queues_extra_conversations_until_a_slot_frees() {
    let codex =
        Arc::new(FakeCodexExecutor::blocking(vec!["thread-priv", "thread-group"], "turn-x"));
    let (command_tx, command_rx) = mpsc::channel(16);
    let (control_tx, control_rx) = mpsc::channel(16);
    let state = ServiceState::with_control(command_tx, control_tx);
    let sent_messages = Arc::new(StdMutex::new(Vec::new()));
    let bridge_handle = spawn_bridge_sink(command_rx, sent_messages);
    let store = Arc::new(AsyncMutex::new(StateStore::open_in_memory().expect("store")));
    let tempdir = TempDir::new().expect("tempdir");
    let mut config = runtime_config(tempdir.path());
    config.runtime_pool_size = 1;

    let run_handle =
        tokio::spawn(orchestrator::run(state.clone(), control_rx, codex.clone(), store, config));
    wait_for_snapshot_prompt_file(&state).await;

    state.publish_event(make_private_event_from(2_394_626_220, "admin", 9501, "私聊长跑"));
    state.publish_event(make_group_event_from(2_394_626_220, "admin", 8888, 9502, "群里排队"));

    timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = state.runtime_snapshot().await;
            let running_count = snapshot
                .lanes
                .iter()
                .filter(|lane| {
                    lane.state == codex_bridge_core::lane_manager::LaneRuntimeState::Running
                })
                .count();
            let queued_count = snapshot
                .lanes
                .iter()
                .filter(|lane| {
                    lane.state == codex_bridge_core::lane_manager::LaneRuntimeState::Queued
                })
                .count();
            if running_count == 1 && queued_count == 1 && snapshot.ready_lane_count == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("one lane running and one queued");

    assert_eq!(codex.ensure_thread_calls().await.len(), 1);

    state
        .send_control_command(make_command_request_from(2_394_626_220, ControlCommand::Cancel))
        .await
        .expect("cancel running lane");

    timeout(Duration::from_secs(2), async {
        loop {
            if codex.ensure_thread_calls().await.len() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued lane started after slot freed");

    run_handle.abort();
    bridge_handle.abort();
}
