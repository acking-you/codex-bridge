//! Active reply-context registry for skill-driven QQ replies.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Active reply context for a single running task.
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

/// Registry holding zero or more concurrently active reply contexts. The
/// on-disk mirror file always shows the most recently activated context so
/// legacy skills that read the singleton path keep working when only one
/// task is in flight.
#[derive(Debug)]
pub struct ReplyRegistry {
    context_file: PathBuf,
    /// Active reply states keyed by their unique token.
    active: HashMap<String, ActiveReplyState>,
    /// Token of the context that is currently mirrored to the singleton file.
    mirrored_token: Option<String>,
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
            active: HashMap::new(),
            mirrored_token: None,
        }
    }

    /// Activate a new reply context (allowing multiple to coexist) and
    /// mirror it as the most recently activated one to disk.
    pub fn activate(&mut self, context: ActiveReplyContext) -> Result<()> {
        let token = context.token.clone();
        self.active.insert(
            token.clone(),
            ActiveReplyState {
                context,
                send_count: 0,
            },
        );
        self.mirrored_token = Some(token);
        self.persist()
    }

    /// Resolve a reply token into its currently active context.
    pub fn resolve(&self, token: &str) -> Result<ActiveReplyContext> {
        match self.active.get(token) {
            Some(state) => Ok(state.context.clone()),
            None => bail!("reply token is not active"),
        }
    }

    /// Return the most recently activated context, if any.
    pub fn current(&self) -> Option<ActiveReplyContext> {
        self.mirrored_token
            .as_ref()
            .and_then(|token| self.active.get(token))
            .map(|state| state.context.clone())
    }

    /// Mark one successful skill-driven reply send against the given token.
    pub fn mark_sent(&mut self, token: &str) -> Result<usize> {
        let Some(state) = self.active.get_mut(token) else {
            bail!("reply token is not active");
        };
        state.send_count += 1;
        Ok(state.send_count)
    }

    /// Return how many successful reply sends happened for the given token.
    pub fn send_count_for(&self, token: &str) -> usize {
        self.active.get(token).map(|state| state.send_count).unwrap_or(0)
    }

    /// Revoke a single reply context by token. The mirror file is updated to
    /// point at any other still-active context, or removed when none remain.
    pub fn deactivate(&mut self, token: &str) -> Result<()> {
        self.active.remove(token);
        if self.mirrored_token.as_deref() == Some(token) {
            self.mirrored_token = self.active.keys().next().cloned();
        }
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.context_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create reply context directory {}", parent.display()))?;
        }

        let mirrored = self
            .mirrored_token
            .as_ref()
            .and_then(|token| self.active.get(token));

        match mirrored {
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
