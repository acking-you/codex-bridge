//! SQLite state store tests.

use qqbot_core::{
    state_store::{ConversationBinding, TaskStatus, StateStore},
    system_prompt::SYSTEM_PROMPT_VERSION,
    system_prompt::SYSTEM_PROMPT_TEXT,
};
use rusqlite::Connection;
use tempfile::TempDir;

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

#[test]
fn system_prompt_same_version_kept_if_unchanged() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");

    let store = StateStore::open(&path).expect("initialize state");
    let first = store
        .system_prompt_text_for(SYSTEM_PROMPT_VERSION)
        .expect("read prompt version text")
        .expect("version exists");
    drop(store);

    let reopened = StateStore::open(&path).expect("reopen state");
    let second = reopened
        .system_prompt_text_for(SYSTEM_PROMPT_VERSION)
        .expect("read prompt version text")
        .expect("version exists");

    assert_eq!(first, second);
    assert_eq!(second, SYSTEM_PROMPT_TEXT);
}

#[test]
fn system_prompt_same_version_with_different_text_fails() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");

    let store = StateStore::open(&path).expect("initialize state");
    drop(store);

    let conn = Connection::open(&path).expect("open sqlite db");
    conn.execute(
        "UPDATE system_prompt_versions SET prompt_text = ?1 WHERE version = ?2",
        [&"corrupted prompt", SYSTEM_PROMPT_VERSION],
    )
    .expect("corrupt prompt text");

    let reopened = StateStore::open(&path);
    assert!(
        reopened.is_err(),
        "expected reopening to fail when prompt text changed for same version"
    );
}

#[test]
fn state_store_open_fails_on_newer_schema() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");

    let conn = Connection::open(&path).expect("open sqlite db");
    conn.execute_batch("PRAGMA user_version = 2;")
        .expect("set newer schema version");

    let reopened = StateStore::open(&path);
    assert!(
        reopened.is_err(),
        "expected error when schema version is newer than supported"
    );
}
