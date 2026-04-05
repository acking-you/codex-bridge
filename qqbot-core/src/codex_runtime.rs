//! Shared codex runtime primitives and helpers.

use serde_json::Value;

/// Final outcome returned from a codex turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexTurnResult {
    /// Thread id used by the runtime.
    pub thread_id: String,
    /// Turn id used by the runtime.
    pub turn_id: String,
    /// Raw items emitted by the runtime for this turn.
    pub items: Vec<Value>,
    /// Last assistant/agent text message, if one exists.
    pub final_reply: Option<String>,
}

/// Minimal interface for codex execution runtimes.
pub trait CodexExecutor {
    /// Ensure a thread is available and return its id.
    fn ensure_thread(&mut self) -> anyhow::Result<String>;
    /// Run a turn and return a summary result.
    fn run_turn(&mut self, user_input: &str) -> anyhow::Result<CodexTurnResult>;
    /// Interrupt a running turn when supported.
    fn interrupt(&mut self) -> anyhow::Result<()>;
}

/// Extract the last agent/assistant text message from turn items.
pub fn extract_final_reply(items: &[Value]) -> Option<String> {
    items.iter().rev().find_map(extract_message_text)
}

fn extract_message_text(item: &Value) -> Option<String> {
    let item = item.get("item").unwrap_or(item);
    let item_type = item.get("type")?.as_str()?;

    match item_type {
        "agentMessage" | "assistantMessage" | "assistant" => item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string),
        _ => None,
    }
}
