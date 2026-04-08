//! SQLite state store tests.

use codex_bridge_core::state_store::{ConversationBinding, StateStore, TaskStatus};
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn binding_round_trip() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-1".to_string(),
        thread_id: "thr-100".to_string(),
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
        thread_id: "thr-200".to_string(),
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
fn updated_task_status_is_not_recovered_as_running() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-2b".to_string(),
        thread_id: "thr-250".to_string(),
    };

    store.upsert_binding(&binding).expect("upsert binding");
    let task_id = store
        .insert_task(&binding, TaskStatus::Running)
        .expect("insert running task");
    store
        .update_task_status(&task_id, TaskStatus::Completed)
        .expect("update task status");

    let interrupted = store
        .mark_running_tasks_interrupted()
        .expect("mark running interrupted");
    assert_eq!(interrupted, 0);

    let latest = store
        .latest_task_for_conversation("conv-2b")
        .expect("query latest task")
        .expect("task exists");
    assert_eq!(latest.task_id, task_id);
    assert_eq!(latest.status, TaskStatus::Completed);
}

#[test]
fn latest_task_for_conversation_returns_recent() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-3".to_string(),
        thread_id: "thr-300".to_string(),
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
fn task_source_metadata_round_trips() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-source".to_string(),
        thread_id: "thr-350".to_string(),
    };

    let task_id = store
        .insert_task_with_source(&binding, TaskStatus::Running, 42, 99001)
        .expect("insert source-aware task");

    let latest = store
        .latest_task_for_conversation(&binding.conversation_key)
        .expect("query latest task")
        .expect("task exists");

    assert_eq!(latest.task_id, task_id);
    assert_eq!(latest.owner_sender_id, 42);
    assert_eq!(latest.source_message_id, 99001);
}

#[test]
fn pending_approval_task_round_trips_and_can_expire() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let task_id = store
        .insert_task_pending_approval("conv-pending", 7, 88001)
        .expect("insert pending approval task");

    let pending = store
        .task_by_id(&task_id)
        .expect("query pending task")
        .expect("pending task exists");
    assert_eq!(pending.status, TaskStatus::PendingApproval);
    assert_eq!(pending.thread_id, "");

    let expired = store
        .mark_pending_tasks_expired()
        .expect("mark pending tasks expired");
    assert_eq!(expired, 1);

    let expired_task = store
        .task_by_id(&task_id)
        .expect("query expired task")
        .expect("expired task exists");
    assert_eq!(expired_task.status, TaskStatus::Expired);
}

#[test]
fn recent_task_output_keeps_only_latest_entries() {
    let store = StateStore::open_in_memory().expect("open in-memory store");
    let binding = ConversationBinding {
        conversation_key: "conv-output".to_string(),
        thread_id: "thr-output".to_string(),
    };
    let task_id = store
        .insert_task_with_source(&binding, TaskStatus::Running, 42, 10001)
        .expect("insert running task");

    for index in 0..6 {
        store
            .append_task_output(&task_id, &format!("line-{index}"), 4)
            .expect("append output");
    }

    let recent = store
        .recent_task_output(&task_id, 4)
        .expect("query recent output");
    assert_eq!(recent, vec![
        "line-2".to_string(),
        "line-3".to_string(),
        "line-4".to_string(),
        "line-5".to_string(),
    ]);
}

#[test]
fn schema_v4_drops_prompt_version_columns_from_runtime_tables() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");

    let _store = StateStore::open(&path).expect("initialize state");
    let conn = Connection::open(&path).expect("open sqlite db");

    let binding_columns = table_columns(&conn, "conversation_bindings");
    assert_eq!(binding_columns, vec!["conversation_key", "thread_id"]);

    let task_columns = table_columns(&conn, "task_runs");
    assert_eq!(task_columns, vec![
        "task_id",
        "conversation_key",
        "thread_id",
        "owner_sender_id",
        "source_message_id",
        "status",
        "created_at",
    ]);

    let output_columns = table_columns(&conn, "task_output");
    assert_eq!(output_columns, vec!["row_id", "task_id", "text", "created_at"]);
}

#[test]
fn legacy_v3_store_migrates_without_prompt_version_columns() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");
    let conn = Connection::open(&path).expect("open sqlite db");
    conn.execute_batch(
        "
        CREATE TABLE conversation_bindings (
            conversation_key TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            prompt_version TEXT NOT NULL
        );
        CREATE TABLE task_runs (
            task_id TEXT PRIMARY KEY,
            conversation_key TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            prompt_version TEXT NOT NULL,
            owner_sender_id INTEGER NOT NULL DEFAULT 0,
            source_message_id INTEGER NOT NULL DEFAULT 0,
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
        INSERT INTO conversation_bindings (conversation_key, thread_id, prompt_version)
            VALUES ('conv-legacy', '777', 'legacy-v1');
        INSERT INTO task_runs (
            task_id,
            conversation_key,
            thread_id,
            prompt_version,
            owner_sender_id,
            source_message_id,
            status,
            created_at
        ) VALUES ('task-1', 'conv-legacy', '777', 'legacy-v1', 42, 9001, 'Completed', \
         strftime('%s','now'));
        PRAGMA user_version = 3;
        ",
    )
    .expect("create legacy schema");
    drop(conn);

    let store = StateStore::open(&path).expect("open legacy db");
    let binding = store
        .binding("conv-legacy")
        .expect("read legacy binding")
        .expect("legacy binding exists");
    assert_eq!(binding.thread_id, "777");

    let conn = Connection::open(&path).expect("open migrated sqlite db");
    let binding_columns = table_columns(&conn, "conversation_bindings");
    let task_columns = table_columns(&conn, "task_runs");
    assert_eq!(binding_columns, vec!["conversation_key", "thread_id"]);
    assert_eq!(task_columns, vec![
        "task_id",
        "conversation_key",
        "thread_id",
        "owner_sender_id",
        "source_message_id",
        "status",
        "created_at",
    ]);
}

#[test]
fn state_store_open_fails_on_newer_schema() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("state.sqlite3");

    let conn = Connection::open(&path).expect("open sqlite db");
    conn.execute_batch("PRAGMA user_version = 6;")
        .expect("set newer schema version");

    let reopened = StateStore::open(&path);
    assert!(reopened.is_err(), "expected error when schema version is newer than supported");
}

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare pragma");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query pragma");
    rows.map(|row| row.expect("column name")).collect()
}
