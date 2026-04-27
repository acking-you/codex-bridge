//! Admin approval config parsing and pending-approval queue primitives.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use thiserror::Error;

use crate::message_router::TaskRequest;

/// Default admin QQ identifier written into freshly created runtime config.
pub const DEFAULT_ADMIN_USER_ID: i64 = 2_394_626_220;

/// Operator-owned admin approval configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminConfig {
    /// QQ identifier allowed to approve or bypass tasks.
    pub admin_user_id: i64,
    /// QQ group ids that are wholesale trusted: every member of these
    /// groups can reach Codex via `@bot` without admin approval. Admin
    /// itself is always trusted regardless of this list.
    ///
    /// This is a looser permission than admin-level trust — it lets a
    /// known friendly group skip the salute-reaction approval dance. It
    /// does NOT let trusted-group members bypass host-level policy
    /// (deletion refusal, heavy-load refusal, etc.), which live in the
    /// bridge protocol prompt.
    pub trusted_group_ids: Vec<i64>,
}

/// Raw TOML shape used to deserialize `admin.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAdminConfig {
    admin_user_id: i64,
    #[serde(default)]
    trusted_group_ids: Vec<i64>,
}

impl AdminConfig {
    /// Parse admin config from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("read admin config {}", path.display()))?;
        Self::parse_contents(&contents)
    }

    /// Parse admin config from a TOML string.
    pub fn parse_contents(contents: &str) -> Result<Self> {
        let raw: RawAdminConfig = toml::from_str(contents).context("decode admin config TOML")?;
        if raw.admin_user_id <= 0 {
            bail!("admin_user_id must be a positive QQ identifier");
        }
        for id in &raw.trusted_group_ids {
            if *id <= 0 {
                bail!("trusted_group_ids entries must be positive QQ group ids");
            }
        }
        Ok(Self { admin_user_id: raw.admin_user_id, trusted_group_ids: raw.trusted_group_ids })
    }
}

/// Render the default `admin.toml` template content.
pub fn default_admin_config_template() -> String {
    format!(
        "# Codex Bridge admin approval config\nadmin_user_id = {DEFAULT_ADMIN_USER_ID}\n\n# QQ \
         group ids that are trusted wholesale: every member in these groups can\n# reach Codex \
         via @bot without admin approval. Leave empty ([]) to require\n# admin approval for every \
         non-admin group task (the default).\n#\n# Trusted-group members still follow host-level \
         policy (no deletion, no\n# heavy-load operations, etc.) — trust here only short-circuits \
         the admin\n# salute-reaction dance, not the bridge's own refusal \
         rules.\ntrusted_group_ids = []\n"
    )
}

#[cfg(test)]
mod admin_config_tests {
    use super::*;

    #[test]
    fn parses_admin_id_with_empty_trusted_groups_by_default() {
        let toml = "admin_user_id = 123";
        let config = AdminConfig::parse_contents(toml).expect("parse");
        assert_eq!(config.admin_user_id, 123);
        assert!(config.trusted_group_ids.is_empty());
    }

    #[test]
    fn parses_trusted_group_ids_array() {
        let toml = "admin_user_id = 123\ntrusted_group_ids = [555, 777]\n";
        let config = AdminConfig::parse_contents(toml).expect("parse");
        assert_eq!(config.admin_user_id, 123);
        assert_eq!(config.trusted_group_ids, vec![555, 777]);
    }

    #[test]
    fn rejects_non_positive_admin_id() {
        let err = AdminConfig::parse_contents("admin_user_id = 0").expect_err("reject 0");
        assert!(format!("{err:#}").contains("admin_user_id"));
    }

    #[test]
    fn rejects_non_positive_trusted_group_id() {
        let toml = "admin_user_id = 123\ntrusted_group_ids = [0]";
        let err = AdminConfig::parse_contents(toml).expect_err("reject zero group");
        assert!(format!("{err:#}").contains("trusted_group_ids"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let toml = "admin_user_id = 123\ncompletely_unknown = 1";
        AdminConfig::parse_contents(toml).expect_err("unknown field should fail");
    }

    #[test]
    fn default_template_round_trips() {
        let rendered = default_admin_config_template();
        let config = AdminConfig::parse_contents(&rendered).expect("parse template");
        assert_eq!(config.admin_user_id, DEFAULT_ADMIN_USER_ID);
        assert!(config.trusted_group_ids.is_empty());
    }
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
        Self { task_id, task, created_at, expires_at: created_at + timeout }
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
        Self { by_task_id: HashMap::new(), by_conversation: HashMap::new(), capacity }
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
