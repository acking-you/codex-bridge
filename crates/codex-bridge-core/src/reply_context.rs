//! Active reply-context registry for skill-driven QQ replies.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Active reply context for the single running task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveReplyContext {
    /// One-time token bound to the current running task.
    pub token: String,
    /// Conversation key currently being served.
    pub conversation_key: String,
    /// Whether the active conversation is a group chat.
    pub is_group: bool,
    /// QQ target identifier used for the actual send action.
    pub reply_target_id: i64,
    /// Original QQ message id that triggered the task.
    pub source_message_id: i64,
    /// QQ identifier of the user who triggered the task.
    pub source_sender_id: i64,
    /// Display name of the user who triggered the task.
    pub source_sender_name: String,
    /// Repository root the task is allowed to modify.
    pub repo_root: PathBuf,
    /// Artifact root for newly created files and outbound attachments.
    pub artifacts_dir: PathBuf,
}

/// Registry for the single active reply context.
#[derive(Debug)]
pub struct ReplyRegistry {
    context_file: PathBuf,
    active: Option<ActiveReplyState>,
}

#[derive(Debug, Clone)]
struct ActiveReplyState {
    context: ActiveReplyContext,
    send_count: usize,
}

impl ReplyRegistry {
    /// Create a registry backed by the given JSON file.
    pub fn new(context_file: PathBuf) -> Self {
        Self {
            context_file,
            active: None,
        }
    }

    /// Activate a new reply context and mirror it to disk.
    pub fn activate(&mut self, context: ActiveReplyContext) -> Result<()> {
        self.active = Some(ActiveReplyState {
            context,
            send_count: 0,
        });
        self.persist()
    }

    /// Resolve the currently active reply token.
    pub fn resolve(&self, token: &str) -> Result<ActiveReplyContext> {
        let Some(state) = &self.active else {
            bail!("reply token is not active");
        };
        if state.context.token != token {
            bail!("reply token is not active");
        }
        Ok(state.context.clone())
    }

    /// Return the currently active reply context, if present.
    pub fn current(&self) -> Option<ActiveReplyContext> {
        self.active.as_ref().map(|state| state.context.clone())
    }

    /// Mark one successful skill-driven reply send.
    pub fn mark_sent(&mut self, token: &str) -> Result<usize> {
        let Some(state) = self.active.as_mut() else {
            bail!("reply token is not active");
        };
        if state.context.token != token {
            bail!("reply token is not active");
        }
        state.send_count += 1;
        Ok(state.send_count)
    }

    /// Return how many successful reply sends happened for the active task.
    pub fn current_send_count(&self) -> usize {
        self.active
            .as_ref()
            .map(|state| state.send_count)
            .unwrap_or(0)
    }

    /// Revoke the active reply context and remove the on-disk mirror.
    pub fn deactivate(&mut self) -> Result<()> {
        self.active = None;
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.context_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create reply context directory {}", parent.display()))?;
        }

        match &self.active {
            Some(state) => {
                let payload =
                    serde_json::to_vec_pretty(&state.context).context("serialize reply context")?;
                fs::write(&self.context_file, payload)
                    .with_context(|| format!("write {}", self.context_file.display()))?;
            },
            None => {
                if self.context_file.exists() {
                    fs::remove_file(&self.context_file)
                        .with_context(|| format!("remove {}", self.context_file.display()))?;
                }
            },
        }

        Ok(())
    }
}

/// Load the active reply context from the runtime mirror file.
pub fn load_active_reply_context(path: impl AsRef<Path>) -> Result<ActiveReplyContext> {
    let path = path.as_ref();
    let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&raw).context("decode reply context")
}
