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
    scheduler::Scheduler,
    service::{FriendProfile, SendMessageReceipt, ServiceCommand, ServiceState},
    state_store::{ConversationBinding, StateStore},
    system_prompt::SYSTEM_PROMPT_VERSION,
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
    interrupt_notify: Notify,
    turn_id: String,
    reply_text: String,
    status: TurnStatus,
    wait_for_interrupt: bool,
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
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: reply_text.to_string(),
            status,
            wait_for_interrupt: false,
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
            interrupt_notify: Notify::new(),
            turn_id: turn_id.to_string(),
            reply_text: String::new(),
            status: TurnStatus::Interrupted,
            wait_for_interrupt: true,
        }
    }

    async fn ensure_thread_calls(&self) -> Vec<(String, Option<String>)> {
        self.ensure_thread_calls.lock().await.clone()
    }

    async fn interrupt_calls(&self) -> Vec<(String, String)> {
        self.interrupt_calls.lock().await.clone()
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
        Ok(ActiveTurn {
            thread_id: thread_id.to_string(),
            turn_id: self.turn_id.clone(),
        })
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

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        self.interrupt_calls
            .lock()
            .await
            .push((thread_id.to_string(), turn_id.to_string()));
        self.interrupt_notify.notify_waiters();
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
    }
}

fn make_private_event(message_id: i64, text: &str) -> NormalizedEvent {
    serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "message_id": message_id,
        "user_id": 42,
        "self_id": 2993013575i64,
        "sender": { "nickname": "LB" },
        "message": [{ "type": "text", "data": { "text": text } }]
    })
    .try_into()
    .expect("normalize private event")
}

fn make_command_request(command: ControlCommand) -> CommandRequest {
    CommandRequest {
        command,
        conversation_key: "private:42".to_string(),
        reply_target_id: 42,
        is_group: false,
        source_message_id: 9001,
        source_sender_id: 42,
        source_sender_name: "LB".to_string(),
    }
}

fn spawn_bridge_sink(
    mut command_rx: mpsc::Receiver<ServiceCommand>,
    sent_messages: Arc<StdMutex<Vec<String>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                ServiceCommand::SendPrivate {
                    text,
                    respond_to,
                    ..
                } => {
                    sent_messages.lock().expect("messages").push(text);
                    let _ = respond_to.send(Ok(SendMessageReceipt {
                        message_id: 1,
                    }));
                },
                ServiceCommand::SendGroup {
                    text,
                    respond_to,
                    ..
                } => {
                    sent_messages.lock().expect("messages").push(text);
                    let _ = respond_to.send(Ok(SendMessageReceipt {
                        message_id: 1,
                    }));
                },
                ServiceCommand::SetMessageReaction {
                    message_id,
                    emoji_id,
                    respond_to,
                } => {
                    sent_messages
                        .lock()
                        .expect("messages")
                        .push(format!("REACTION:{message_id}:{emoji_id}"));
                    let _ = respond_to.send(Ok(()));
                },
                ServiceCommand::SendOutbound {
                    message,
                    respond_to,
                } => {
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
                    let _ = respond_to.send(Ok(SendMessageReceipt {
                        message_id: 1,
                    }));
                },
                ServiceCommand::Control {
                    ..
                } => {},
            }
        }
    })
}

fn runtime_config(repo_root: &std::path::Path) -> OrchestratorConfig {
    let artifacts_dir = repo_root.join(".run/artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    OrchestratorConfig {
        queue_capacity: 5,
        repo_root: repo_root.to_path_buf(),
        artifacts_dir,
        group_start_reaction_emoji_id: "282".to_string(),
    }
}

#[tokio::test]
async fn task_request_sends_started_and_final_reply() {
    let codex = FakeCodexExecutor::with_reply(vec!["thr_123"], "turn_1", "已经处理完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5);
    let task = make_task(1001, "private:42");

    handle_route_decision(RouteDecision::Task(task), &codex, &replies, &mut scheduler)
        .await
        .expect("handle task");

    let sent = replies.messages();
    assert_eq!(sent[0], "欸、我先去看一下……稍等我一下。");
    assert_eq!(sent[1], "已经处理完成");
}

#[tokio::test]
async fn task_request_persists_conversation_binding_when_missing() {
    let codex = FakeCodexExecutor::with_reply(vec!["thread-1"], "turn_1", "已完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5);
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
    assert_eq!(binding.prompt_version, SYSTEM_PROMPT_VERSION);
}

#[tokio::test]
async fn task_request_reuses_binding_for_follow_up_task() {
    let codex = FakeCodexExecutor::with_reply(vec!["thread-2", "thread-2"], "turn_1", "已完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5);
    let store = Arc::new(AsyncMutex::new(
        StateStore::open_in_memory().expect("open in-memory state store"),
    ));

    store
        .lock()
        .await
        .upsert_binding(&ConversationBinding {
            conversation_key: "private:88".to_string(),
            thread_id: "thread-2".to_string(),
            prompt_version: "legacy-v1".to_string(),
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
    assert_eq!(binding.prompt_version, "legacy-v1");
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

    timeout(Duration::from_secs(1), async {
        loop {
            if state.task_snapshot().await.prompt_version.as_deref() == Some(SYSTEM_PROMPT_VERSION)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("orchestrator initialized");
    state
        .set_friends(vec![FriendProfile {
            user_id: 42,
            nickname: "LB".to_string(),
            remark: None,
        }])
        .await;

    state.publish_event(make_private_event(3001, "开始长任务"));

    timeout(Duration::from_secs(1), async {
        loop {
            if state
                .task_snapshot()
                .await
                .running_conversation_key
                .as_deref()
                == Some("private:42")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task started");

    state
        .send_control_command(make_command_request(ControlCommand::Cancel))
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
        .any(|text| text == "欸、我先去看一下……稍等我一下。"));
    assert!(messages
        .iter()
        .any(|text| text == "收到，我去把这条任务拦下来……等它停住。"));

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
    timeout(Duration::from_secs(1), async {
        loop {
            if state.task_snapshot().await.prompt_version.as_deref() == Some(SYSTEM_PROMPT_VERSION)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("orchestrator initialized");
    state.publish_event(make_private_event(4001, "在吗"));

    timeout(Duration::from_secs(1), async {
        loop {
            if sent_messages
                .lock()
                .expect("messages")
                .iter()
                .any(|text| text == "那个……先加个好友吧。没加好友的私聊这边不会直接接入。")
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
async fn group_start_uses_salute_and_completed_turn_without_skill_reply_gets_fallback() {
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
        store,
        runtime_config(tempdir.path()),
    ));
    timeout(Duration::from_secs(1), async {
        loop {
            if state.task_snapshot().await.prompt_version.as_deref() == Some(SYSTEM_PROMPT_VERSION)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("orchestrator initialized");

    let raw = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "message_id": 5001,
        "group_id": 777,
        "user_id": 42,
        "self_id": 2993013575i64,
        "raw_message": "@bot 生成一张图",
        "message": [
            { "type": "at", "data": { "qq": "2993013575", "name": "bot" } },
            { "type": "text", "data": { "text": " 生成一张图" } }
        ],
        "sender": { "nickname": "alice" }
    });
    let event = NormalizedEvent::try_from(raw).expect("normalize group event");
    state.publish_event(event);

    timeout(Duration::from_secs(1), async {
        loop {
            let messages = sent_messages.lock().expect("messages").clone();
            if messages.iter().any(|text| text == "REACTION:5001:282")
                && messages
                    .iter()
                    .any(|text| text == "已经处理完了，但这次没有生成可回传的结果。")
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
