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

/// Registry holding zero or more concurrently active reply contexts.
///
/// Each active context is persisted only to its deterministic
/// per-conversation file at `contexts_dir/<sanitized_key>.json`. There is no
/// singleton mirror file because it is inherently racy under concurrent tasks
/// from different conversations.
#[derive(Debug)]
pub struct ReplyRegistry {
    contexts_dir: PathBuf,
    /// Active reply states keyed by their unique token.
    active: HashMap<String, ActiveReplyState>,
}

#[derive(Debug, Clone)]
struct ActiveReplyState {
    context: ActiveReplyContext,
    send_count: usize,
    per_conversation_file: PathBuf,
}

impl ReplyRegistry {
    /// Create a registry backed by the per-conversation directory. Paths are
    /// created on demand when contexts are activated.
    pub fn new(contexts_dir: PathBuf) -> Self {
        Self {
            contexts_dir,
            active: HashMap::new(),
        }
    }

    /// Activate a new reply context by writing the lane-scoped
    /// per-conversation file keyed on `conversation_key`.
    pub fn activate(&mut self, context: ActiveReplyContext) -> Result<()> {
        let token = context.token.clone();
        let per_conversation_file =
            reply_context_file_for(&self.contexts_dir, &context.conversation_key);
        write_context_to_file(&per_conversation_file, &context).with_context(|| {
            format!("write per-conversation reply context {}", per_conversation_file.display())
        })?;
        self.active.insert(token.clone(), ActiveReplyState {
            context,
            send_count: 0,
            per_conversation_file,
        });
        Ok(())
    }

    /// Resolve a reply token into its currently active context.
    pub fn resolve(&self, token: &str) -> Result<ActiveReplyContext> {
        match self.active.get(token) {
            Some(state) => Ok(state.context.clone()),
            None => bail!("reply token is not active"),
        }
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
        self.active
            .get(token)
            .map(|state| state.send_count)
            .unwrap_or(0)
    }

    /// Revoke a single reply context by token. Removes the matching
    /// per-conversation file.
    pub fn deactivate(&mut self, token: &str) -> Result<()> {
        if let Some(state) = self.active.remove(token) {
            if state.per_conversation_file.exists() {
                fs::remove_file(&state.per_conversation_file).with_context(|| {
                    format!(
                        "remove per-conversation reply context {}",
                        state.per_conversation_file.display()
                    )
                })?;
            }
        }
        Ok(())
    }
}

/// Compute the deterministic per-conversation reply-context filename.
///
/// Conversation keys carry a `:` separator (e.g. `group:123`,
/// `private:456`) that is not safe everywhere on disk, so `:` becomes
/// `_`; other characters are kept verbatim since they're always ASCII
/// alphanumeric under the current routing convention.
pub fn reply_context_file_for(contexts_dir: &Path, conversation_key: &str) -> PathBuf {
    let sanitized = conversation_key.replace(':', "_");
    contexts_dir.join(format!("{sanitized}.json"))
}

fn write_context_to_file(path: &Path, context: &ActiveReplyContext) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create reply context directory {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(context).context("serialize reply context")?;
    fs::write(path, payload).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Load the active reply context from one per-conversation context JSON file.
pub fn load_active_reply_context(path: impl AsRef<Path>) -> Result<ActiveReplyContext> {
    let path = path.as_ref();
    let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&raw).context("decode reply context")
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn sample_context(token: &str, conversation_key: &str) -> ActiveReplyContext {
        ActiveReplyContext {
            token: token.into(),
            conversation_key: conversation_key.into(),
            is_group: conversation_key.starts_with("group:"),
            reply_target_id: 1,
            source_message_id: 1,
            source_sender_id: 1,
            source_sender_name: "tester".into(),
            repo_root: PathBuf::from("/tmp"),
            artifacts_dir: PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn filename_sanitizes_conversation_key_separator() {
        let dir = Path::new("/tmp/contexts");
        assert_eq!(reply_context_file_for(dir, "group:111"), dir.join("group_111.json"));
        assert_eq!(reply_context_file_for(dir, "private:222"), dir.join("private_222.json"));
    }

    #[test]
    fn activate_writes_one_per_conversation_file() {
        let tmp = TempDir::new().expect("tmpdir");
        let contexts = tmp.path().join("contexts");
        let mut registry = ReplyRegistry::new(contexts.clone());

        registry
            .activate(sample_context("token-a", "group:111"))
            .expect("activate a");

        assert!(contexts.join("group_111.json").is_file());
    }

    #[test]
    fn concurrent_activations_keep_distinct_per_conversation_files() {
        let tmp = TempDir::new().expect("tmpdir");
        let contexts = tmp.path().join("contexts");
        let mut registry = ReplyRegistry::new(contexts.clone());

        registry
            .activate(sample_context("token-a", "group:111"))
            .expect("activate a");
        registry
            .activate(sample_context("token-b", "group:222"))
            .expect("activate b");

        let file_a = contexts.join("group_111.json");
        let file_b = contexts.join("group_222.json");
        assert!(file_a.is_file());
        assert!(file_b.is_file());

        let ctx_a = load_active_reply_context(&file_a).expect("read a");
        let ctx_b = load_active_reply_context(&file_b).expect("read b");
        assert_eq!(ctx_a.token, "token-a");
        assert_eq!(ctx_a.conversation_key, "group:111");
        assert_eq!(ctx_b.token, "token-b");
        assert_eq!(ctx_b.conversation_key, "group:222");
    }

    #[test]
    fn deactivate_removes_only_its_own_per_conversation_file() {
        let tmp = TempDir::new().expect("tmpdir");
        let contexts = tmp.path().join("contexts");
        let mut registry = ReplyRegistry::new(contexts.clone());

        registry
            .activate(sample_context("token-a", "group:111"))
            .expect("activate a");
        registry
            .activate(sample_context("token-b", "group:222"))
            .expect("activate b");

        registry.deactivate("token-a").expect("deactivate a");

        assert!(!contexts.join("group_111.json").exists());
        assert!(contexts.join("group_222.json").is_file());
    }
}
