//! Orchestrator unit tests.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use codex_app_server_protocol::TurnStatus;
use codex_bridge_core::{
    codex_runtime::{CodexExecutor, CodexTurnResult},
    message_router::{RouteDecision, TaskRequest},
    orchestrator::handle_route_decision,
    scheduler::Scheduler,
};

#[derive(Debug)]
struct FakeCodexExecutor {
    thread_id: String,
    turn_id: String,
    reply_text: String,
}

impl FakeCodexExecutor {
    fn with_reply(thread_id: &str, turn_id: &str, reply_text: &str) -> Self {
        Self {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            reply_text: reply_text.to_string(),
        }
    }
}

#[async_trait]
impl CodexExecutor for FakeCodexExecutor {
    async fn ensure_thread(
        &self,
        _conversation_key: &str,
        _existing_thread_id: Option<&str>,
    ) -> Result<String> {
        Ok(self.thread_id.clone())
    }

    async fn run_turn(&self, thread_id: &str, _input_text: &str) -> Result<CodexTurnResult> {
        Ok(CodexTurnResult {
            thread_id: thread_id.to_string(),
            turn_id: self.turn_id.clone(),
            status: TurnStatus::Completed,
            error_message: None,
            items: vec![],
            final_reply: Some(self.reply_text.clone()),
        })
    }

    async fn interrupt(&self, _thread_id: &str, _turn_id: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Default, Clone)]
struct FakeReplySink {
    messages: Arc<Mutex<Vec<String>>>,
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

#[tokio::test]
async fn task_request_sends_started_and_final_reply() {
    let codex = FakeCodexExecutor::with_reply("thr_123", "turn_1", "已经处理完成");
    let replies = FakeReplySink::default();
    let mut scheduler = Scheduler::new(5);

    let task = TaskRequest {
        conversation_key: "private:42".to_string(),
        source_message_id: 1001,
        source_sender_id: 42,
        source_sender_name: "LB".to_string(),
        source_text: "修一下 README".to_string(),
        is_group: false,
        reply_target_id: 42,
    };

    handle_route_decision(RouteDecision::Task(task), &codex, &replies, &mut scheduler)
        .await
        .expect("handle task");

    let sent = replies.messages();
    assert_eq!(sent[0], "收到，开始处理。");
    assert_eq!(sent[1], "已经处理完成");
}
