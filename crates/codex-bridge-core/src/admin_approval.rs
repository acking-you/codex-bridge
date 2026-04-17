//! Admin approval config parsing and pending-approval queue primitives.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use thiserror::Error;

use crate::message_router::TaskRequest;

/// Default admin QQ identifier written into freshly created runtime config.
pub const DEFAULT_ADMIN_USER_ID: i64 = 2_394_626_220;

/// Operator-owned admin approval configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminConfig {
    /// QQ identifier allowed to approve or bypass tasks.
    pub admin_user_id: i64,
}

impl AdminConfig {
    /// Parse admin config from a minimal TOML-like file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("read admin config {}", path.display()))?;
        Self::parse_contents(&contents)
    }

    /// Parse admin config from string contents.
    pub fn parse_contents(contents: &str) -> Result<Self> {
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            if key.trim() != "admin_user_id" {
                continue;
            }
            let admin_user_id = value
                .trim()
                .parse::<i64>()
                .context("parse admin_user_id as integer")?;
            if admin_user_id <= 0 {
                bail!("admin_user_id must be a positive QQ identifier");
            }
            return Ok(Self {
                admin_user_id,
            });
        }
        bail!("admin_user_id is missing from admin config");
    }
}

/// Render the default `admin.toml` template content.
pub fn default_admin_config_template() -> String {
    format!("# Codex Bridge admin approval config\nadmin_user_id = {DEFAULT_ADMIN_USER_ID}\n")
}

/// In-memory request waiting for explicit admin approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    /// Stable task identifier.
    pub task_id: String,
    /// Original task request preserved until approval or denial.
    pub task: TaskRequest,
    /// Insertion timestamp.
    pub created_at: Instant,
    /// Expiration timestamp.
    pub expires_at: Instant,
}

impl PendingApproval {
    /// Create a pending approval entry from a task and timeout.
    pub fn new(task_id: String, task: TaskRequest, created_at: Instant, timeout: Duration) -> Self {
        Self {
            task_id,
            task,
            created_at,
            expires_at: created_at + timeout,
        }
    }
}

/// Errors emitted by the pending approval pool.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PendingApprovalError {
    /// A request from the same conversation is already waiting for approval.
    #[error("conversation already waiting for admin approval")]
    ConversationAlreadyWaiting,
    /// The pool is full and cannot accept more waiting approvals.
    #[error("pending approval pool is full")]
    PoolFull,
}

/// Bounded in-memory pool holding tasks that still need admin approval.
#[derive(Debug, Default)]
pub struct PendingApprovalPool {
    by_task_id: HashMap<String, PendingApproval>,
    by_conversation: HashMap<String, String>,
    capacity: usize,
}

impl PendingApprovalPool {
    /// Create a new bounded pending-approval pool.
    pub fn new(capacity: usize) -> Self {
        Self {
            by_task_id: HashMap::new(),
            by_conversation: HashMap::new(),
            capacity,
        }
    }

    /// Insert a pending approval request.
    pub fn insert(&mut self, pending: PendingApproval) -> Result<(), PendingApprovalError> {
        if self
            .by_conversation
            .contains_key(&pending.task.conversation_key)
        {
            return Err(PendingApprovalError::ConversationAlreadyWaiting);
        }
        if self.by_task_id.len() >= self.capacity {
            return Err(PendingApprovalError::PoolFull);
        }
        self.by_conversation
            .insert(pending.task.conversation_key.clone(), pending.task_id.clone());
        self.by_task_id.insert(pending.task_id.clone(), pending);
        Ok(())
    }

    /// Return a waiting approval by task id without removing it.
    pub fn get(&self, task_id: &str) -> Option<&PendingApproval> {
        self.by_task_id.get(task_id)
    }

    /// Remove and return a waiting approval by task id.
    pub fn take(&mut self, task_id: &str) -> Option<PendingApproval> {
        let pending = self.by_task_id.remove(task_id)?;
        self.by_conversation.remove(&pending.task.conversation_key);
        Some(pending)
    }

    /// Remove and return one waiting group approval by source message.
    pub fn take_group_by_source_message(
        &mut self,
        group_id: i64,
        source_message_id: i64,
    ) -> Option<PendingApproval> {
        let task_id = self.by_task_id.iter().find_map(|(task_id, pending)| {
            (pending.task.is_group
                && pending.task.reply_target_id == group_id
                && pending.task.source_message_id == source_message_id)
                .then(|| task_id.clone())
        })?;
        self.take(&task_id)
    }

    /// Remove and return all expired approvals at `now`.
    pub fn take_expired(&mut self, now: Instant) -> Vec<PendingApproval> {
        let expired_ids = self
            .by_task_id
            .iter()
            .filter_map(
                |(task_id, pending)| {
                    if pending.expires_at <= now {
                        Some(task_id.clone())
                    } else {
                        None
                    }
                },
            )
            .collect::<Vec<_>>();
        expired_ids
            .into_iter()
            .filter_map(|task_id| self.take(&task_id))
            .collect()
    }
}
