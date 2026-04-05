//! SQLite state store tests.

use qqbot_core::{
    state_store::{ConversationBinding, TaskStatus, StateStore},
    system_prompt::SYSTEM_PROMPT_VERSION,
};

#[test]
fn binding_round_trip() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-1".to_string(),
        thread_id: 100,
        prompt_version: SYSTEM_PROMPT_VERSION.to_string(),
    };

    store.upsert_binding(&binding).expect("upsert binding");
    let loaded = store
        .binding("conv-1")
        .expect("query binding")
        .expect("binding exists");

    assert_eq!(loaded, binding);
}

#[test]
fn running_task_marks_interrupted() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-2".to_string(),
        thread_id: 200,
        prompt_version: SYSTEM_PROMPT_VERSION.to_string(),
    };

    store
        .upsert_binding(&binding)
        .expect("upsert binding");
    let task_id = store
        .insert_task(&binding, TaskStatus::Running)
        .expect("insert running task");

    let interrupted = store
        .mark_running_tasks_interrupted()
        .expect("mark running interrupted");
    assert_eq!(interrupted, 1);

    let latest = store
        .latest_task_for_conversation("conv-2")
        .expect("query latest task")
        .expect("task exists");
    assert_eq!(latest.task_id, task_id);
    assert_eq!(latest.status, TaskStatus::Interrupted);
}

#[test]
fn latest_task_for_conversation_returns_recent() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-3".to_string(),
        thread_id: 300,
        prompt_version: SYSTEM_PROMPT_VERSION.to_string(),
    };

    let first = store
        .insert_task(&binding, TaskStatus::Queued)
        .expect("insert first task");
    let second = store
        .insert_task(&binding, TaskStatus::Running)
        .expect("insert second task");

    let latest = store
        .latest_task_for_conversation(&binding.conversation_key)
        .expect("query latest task")
        .expect("task exists");

    assert_ne!(first, second);
    assert_eq!(latest.task_id, second);
}

#[test]
fn system_prompt_version_is_seeded() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    assert!(store.has_system_prompt_version(SYSTEM_PROMPT_VERSION).expect("query prompt version"));
}
