//! SQLite state store tests.

use qqbot_core::{
    state_store::{ConversationBinding, StateStore, TaskStatus},
    system_prompt::{SYSTEM_PROMPT_TEXT, SYSTEM_PROMPT_VERSION},
};
use rusqlite::Connection;
use tempfile::TempDir;

const LEGACY_SYSTEM_PROMPT_VERSION: &str = "v1.0.0";
const LEGACY_SYSTEM_PROMPT_TEXT: &str =
    "You are an assistant constrained to this project only.\nDo not help with other systems \
     outside the repository under task. For this project,\nyou may use web search when external \
     references are required and you may run low-risk\nshell inspection (for example, listing \
     directories, reading non-sensitive logs,\nand checking process status). Do NOT use \
     thread/shellCommand. Never issue or\nrecommend commands such as kill, pkill, killall, \
     reboot, shutdown, poweroff,\nsystemctl stop, systemctl restart, or kill. If a request is \
     blocked by policy,\nexplain the refusal clearly and switch to a safe workflow that still \
     meets the\nintent if possible.";

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

    store.upsert_binding(&binding).expect("upsert binding");
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
    assert!(store
        .has_system_prompt_version(SYSTEM_PROMPT_VERSION)
        .expect("query prompt version"));
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
    conn.execute("UPDATE system_prompt_versions SET prompt_text = ?1 WHERE version = ?2", [
        "corrupted prompt",
        SYSTEM_PROMPT_VERSION,
    ])
    .expect("corrupt prompt text");

    let reopened = StateStore::open(&path);
    assert!(
        reopened.is_err(),
        "expected reopening to fail when prompt text changed for same version"
    );
}

#[test]
fn legacy_system_prompt_version_remains_and_current_version_is_seeded() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");
    let conn = Connection::open(&path).expect("open sqlite db");
    conn.execute_batch(
        "
        CREATE TABLE conversation_bindings (
            conversation_key TEXT PRIMARY KEY,
            thread_id INTEGER NOT NULL,
            prompt_version TEXT NOT NULL
        );
        CREATE TABLE task_runs (
            task_id TEXT PRIMARY KEY,
            conversation_key TEXT NOT NULL,
            thread_id INTEGER NOT NULL,
            prompt_version TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE TABLE bot_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE system_prompt_versions (
            version TEXT PRIMARY KEY,
            prompt_text TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE INDEX task_runs_by_conversation_created
            ON task_runs (conversation_key, created_at);
        ",
    )
    .expect("create legacy schema");
    conn.execute(
        "INSERT INTO system_prompt_versions (version, prompt_text, created_at) VALUES (?1, ?2, \
         strftime('%s', 'now'))",
        (&LEGACY_SYSTEM_PROMPT_VERSION, &LEGACY_SYSTEM_PROMPT_TEXT),
    )
    .expect("insert legacy prompt row");
    conn.execute_batch("PRAGMA user_version = 1")
        .expect("set legacy schema version");
    drop(conn);

    let store = StateStore::open(&path).expect("open legacy db");

    let legacy = store
        .system_prompt_text_for(LEGACY_SYSTEM_PROMPT_VERSION)
        .expect("read legacy version row")
        .expect("legacy version exists");
    let current = store
        .system_prompt_text_for(SYSTEM_PROMPT_VERSION)
        .expect("read current version row")
        .expect("current version exists");

    assert_eq!(legacy, LEGACY_SYSTEM_PROMPT_TEXT);
    assert_eq!(current, SYSTEM_PROMPT_TEXT);
}

#[test]
fn state_store_open_fails_on_newer_schema() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");

    let conn = Connection::open(&path).expect("open sqlite db");
    conn.execute_batch("PRAGMA user_version = 2;")
        .expect("set newer schema version");

    let reopened = StateStore::open(&path);
    assert!(reopened.is_err(), "expected error when schema version is newer than supported");
}
