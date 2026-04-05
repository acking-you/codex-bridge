//! SQLite-backed task and conversation persistence.

use std::{
    fs,
    io::{Error as IoError, ErrorKind},
    path::Path,
};

use anyhow::{Context, Result};
use rusqlite::{params, types::Type, Connection, Error, OptionalExtension};
use uuid::Uuid;

use crate::system_prompt::{SYSTEM_PROMPT_TEXT, SYSTEM_PROMPT_VERSION};

/// State schema migration level.
const CURRENT_SCHEMA_VERSION: i32 = 3;

/// Task lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// The task has been queued and not started yet.
    Queued,
    /// The task is currently being processed.
    Running,
    /// The task finished successfully.
    Completed,
    /// The task failed during execution.
    Failed,
    /// The task was cancelled by caller input.
    Canceled,
    /// The task was interrupted by runtime restart/recovery flow.
    Interrupted,
}

impl TaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Canceled => "Canceled",
            Self::Interrupted => "Interrupted",
        }
    }

    fn from_storage_str(value: &str) -> rusqlite::Result<Self> {
        match value {
            "Queued" => Ok(Self::Queued),
            "Running" => Ok(Self::Running),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Canceled" => Ok(Self::Canceled),
            "Interrupted" => Ok(Self::Interrupted),
            other => Err(Error::FromSqlConversionFailure(
                0,
                Type::Text,
                Box::new(IoError::new(
                    ErrorKind::InvalidData,
                    format!("unknown task status: {other}"),
                )),
            )),
        }
    }
}

/// One logical conversation binding entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationBinding {
    /// External conversation identifier (e.g. source message thread id).
    pub conversation_key: String,
    /// Bot runtime thread id bound to this conversation.
    pub thread_id: String,
    /// System prompt version this conversation expects.
    pub prompt_version: String,
}

/// Minimal task row returned by store queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    /// Stable task identifier.
    pub task_id: String,
    /// Current task status.
    pub status: TaskStatus,
    /// QQ identifier of the user that initiated the task.
    pub owner_sender_id: i64,
    /// Source QQ message identifier.
    pub source_message_id: i64,
}

/// SQLite-backed state persistence store.
pub struct StateStore {
    /// Active SQLite connection.
    conn: Connection,
}

impl StateStore {
    /// Open a state store from disk and run migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create state store directory {}", parent.display()))?;
        }

        let conn = Connection::open(path).context("open sqlite state database")?;
        let mut store = Self {
            conn,
        };
        store.migrate_and_seed()?;
        Ok(store)
    }

    /// Open a new in-memory state store and run migrations.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory sqlite state database")?;
        let mut store = Self {
            conn,
        };
        store.migrate_and_seed()?;
        Ok(store)
    }

    /// Insert or replace a conversation binding by key.
    pub fn upsert_binding(&self, binding: &ConversationBinding) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO conversation_bindings (conversation_key, thread_id, prompt_version)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(conversation_key) DO UPDATE SET
                   thread_id = excluded.thread_id,
                   prompt_version = excluded.prompt_version",
                params![binding.conversation_key, binding.thread_id, binding.prompt_version],
            )
            .context("upsert conversation binding")?;
        Ok(())
    }

    /// Read an existing binding by conversation key.
    pub fn binding(&self, conversation_key: &str) -> Result<Option<ConversationBinding>> {
        let mut stmt = self.conn.prepare(
            "SELECT conversation_key, thread_id, prompt_version
             FROM conversation_bindings
             WHERE conversation_key = ?1",
        )?;
        stmt.query_row((conversation_key,), |row| {
            Ok(ConversationBinding {
                conversation_key: row.get(0)?,
                thread_id: row.get(1)?,
                prompt_version: row.get(2)?,
            })
        })
        .optional()
        .context("query conversation binding")
    }

    /// Insert a new task row and return the generated id.
    pub fn insert_task(&self, binding: &ConversationBinding, status: TaskStatus) -> Result<String> {
        self.insert_task_with_source(binding, status, 0, 0)
    }

    /// Insert a new task row with owner/source metadata and return the
    /// generated id.
    pub fn insert_task_with_source(
        &self,
        binding: &ConversationBinding,
        status: TaskStatus,
        owner_sender_id: i64,
        source_message_id: i64,
    ) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();
        self.conn
            .execute(
                "INSERT INTO task_runs (task_id, conversation_key, thread_id, prompt_version, \
                 owner_sender_id, source_message_id, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%s', 'now'))",
                params![
                    task_id,
                    binding.conversation_key,
                    binding.thread_id,
                    binding.prompt_version,
                    owner_sender_id,
                    source_message_id,
                    status.as_str(),
                ],
            )
            .context("insert task record")?;
        Ok(task_id)
    }

    /// Update the status of an existing task row.
    pub fn update_task_status(&self, task_id: &str, status: TaskStatus) -> Result<()> {
        let updated = self
            .conn
            .execute("UPDATE task_runs SET status = ?1 WHERE task_id = ?2", params![
                status.as_str(),
                task_id
            ])
            .context("update task status")?;
        if updated == 1 {
            Ok(())
        } else {
            anyhow::bail!("task record {task_id} not found for status update");
        }
    }

    /// Return the latest task for a conversation, or `None` if absent.
    pub fn latest_task_for_conversation(
        &self,
        conversation_key: &str,
    ) -> Result<Option<TaskRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, status, owner_sender_id, source_message_id
               FROM task_runs
              WHERE conversation_key = ?1
              ORDER BY created_at DESC, rowid DESC
              LIMIT 1",
        )?;
        let record = stmt
            .query_row((conversation_key,), |row| {
                let task_id: String = row.get(0)?;
                let status_raw: String = row.get(1)?;
                Ok(TaskRecord {
                    task_id,
                    status: TaskStatus::from_storage_str(&status_raw)?,
                    owner_sender_id: row.get(2)?,
                    source_message_id: row.get(3)?,
                })
            })
            .optional()
            .context("query latest task")?;
        Ok(record)
    }

    /// Mark all tasks currently running as interrupted.
    pub fn mark_running_tasks_interrupted(&self) -> Result<usize> {
        let updated = self
            .conn
            .execute("UPDATE task_runs SET status = ?1 WHERE status = ?2", params![
                TaskStatus::Interrupted.as_str(),
                TaskStatus::Running.as_str()
            ])
            .context("mark running tasks interrupted")?;
        Ok(updated)
    }

    /// Check whether a system prompt version exists in the prompt-version
    /// registry.
    pub fn has_system_prompt_version(&self, version: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT 1
               FROM system_prompt_versions
              WHERE version = ?1
              LIMIT 1",
        )?;
        let present = stmt
            .query_row((version,), |_| Ok(true))
            .optional()
            .context("query system prompt version")?
            .unwrap_or(false);
        Ok(present)
    }

    /// Return the exact prompt text for a stored version.
    pub fn system_prompt_text_for(&self, version: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT prompt_text
               FROM system_prompt_versions
              WHERE version = ?1
              LIMIT 1",
        )?;
        let text = stmt
            .query_row((version,), |row| row.get(0))
            .optional()
            .context("query system prompt text")?;
        Ok(text)
    }

    fn migrate_and_seed(&mut self) -> Result<()> {
        let current_version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
            .context("read sqlite user_version")?;
        if current_version > CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported sqlite schema version {current_version} (max supported \
                 {CURRENT_SCHEMA_VERSION})"
            );
        }

        if current_version == 0 {
            let tx = self
                .conn
                .transaction()
                .context("start migration transaction")?;
            run_initial_migration(&tx).context("run initial migration")?;
            tx.execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
                .context("write sqlite schema version")?;
            tx.commit().context("commit migration")?;
        } else if current_version == 1 {
            let tx = self
                .conn
                .transaction()
                .context("start v1 to v2 migration transaction")?;
            migrate_v1_to_v2(&tx).context("migrate sqlite schema from v1 to v2")?;
            tx.execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
                .context("write sqlite schema version")?;
            tx.commit().context("commit v1 to v2 migration")?;
        } else if current_version == 2 {
            let tx = self
                .conn
                .transaction()
                .context("start v2 to v3 migration transaction")?;
            migrate_v2_to_v3(&tx).context("migrate sqlite schema from v2 to v3")?;
            tx.execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
                .context("write sqlite schema version")?;
            tx.commit().context("commit v2 to v3 migration")?;
        }

        self.seed_current_system_prompt_version()
            .context("seed current system prompt version")?;
        Ok(())
    }

    fn seed_current_system_prompt_version(&self) -> Result<()> {
        let existing = self
            .conn
            .query_row(
                "SELECT prompt_text FROM system_prompt_versions WHERE version = ?1",
                [SYSTEM_PROMPT_VERSION],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("read existing system prompt version row")?;

        if let Some(existing) = existing {
            if existing != SYSTEM_PROMPT_TEXT {
                anyhow::bail!(
                    "system prompt text changed for existing version {}",
                    SYSTEM_PROMPT_VERSION
                );
            }
            return Ok(());
        }

        self.conn
            .execute(
                "INSERT INTO system_prompt_versions (version, prompt_text, created_at)
                   VALUES (?1, ?2, strftime('%s', 'now'))",
                params![SYSTEM_PROMPT_VERSION, SYSTEM_PROMPT_TEXT],
            )
            .context("seed current system prompt version")?;
        Ok(())
    }
}

fn run_initial_migration(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE conversation_bindings (
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
        CREATE INDEX IF NOT EXISTS task_runs_by_conversation_created
            ON task_runs (conversation_key, created_at)",
    )
    .context("apply v2 schema")
}

fn migrate_v1_to_v2(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE conversation_bindings RENAME TO conversation_bindings_v1;
        CREATE TABLE conversation_bindings (
            conversation_key TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            prompt_version TEXT NOT NULL
        );
        INSERT INTO conversation_bindings (conversation_key, thread_id, prompt_version)
            SELECT conversation_key, CAST(thread_id AS TEXT), prompt_version
            FROM conversation_bindings_v1;
        DROP TABLE conversation_bindings_v1;

        ALTER TABLE task_runs RENAME TO task_runs_v1;
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
        INSERT INTO task_runs (task_id, conversation_key, thread_id, prompt_version, \
         owner_sender_id, source_message_id, status, created_at)
            SELECT task_id, conversation_key, CAST(thread_id AS TEXT), prompt_version, 0, 0, \
         status, created_at
            FROM task_runs_v1;
        DROP TABLE task_runs_v1;

        DROP INDEX IF EXISTS task_runs_by_conversation_created;
        CREATE INDEX task_runs_by_conversation_created
            ON task_runs (conversation_key, created_at);",
    )
    .context("rewrite v1 integer thread ids to text")
}

fn migrate_v2_to_v3(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE task_runs ADD COLUMN owner_sender_id INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE task_runs ADD COLUMN source_message_id INTEGER NOT NULL DEFAULT 0;",
    )
    .context("add task owner/source columns")
}
