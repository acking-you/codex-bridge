//! Reply-context registry tests.

use codex_bridge_core::reply_context::{
    load_active_reply_context, reply_context_file_for, ActiveReplyContext, ReplyRegistry,
};
use tempfile::TempDir;

#[test]
fn reply_context_token_can_send_multiple_times_until_revoked() {
    let tempdir = TempDir::new().expect("tempdir");
    let repo_root = tempdir.path().to_path_buf();
    let artifacts_dir = repo_root.join(".run/artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    let contexts_dir = repo_root.join(".run/default/run/contexts");
    let mut registry = ReplyRegistry::new(contexts_dir.clone());
    let context = ActiveReplyContext {
        token: "token-1".to_string(),
        conversation_key: "group:777".to_string(),
        is_group: true,
        reply_target_id: 777,
        source_message_id: 9901,
        source_sender_id: 42,
        source_sender_name: "alice".to_string(),
        repo_root: repo_root.clone(),
        artifacts_dir,
    };

    registry
        .activate(context.clone())
        .expect("activate reply context");

    let lane_file = reply_context_file_for(&contexts_dir, "group:777");
    assert_eq!(registry.resolve("token-1").expect("resolve once"), context);
    assert_eq!(registry.resolve("token-1").expect("resolve twice"), context);
    assert_eq!(
        load_active_reply_context(&lane_file)
            .expect("load context from disk")
            .token,
        "token-1"
    );
    assert!(!repo_root
        .join(".run/default/run/reply_context.json")
        .exists());

    registry
        .deactivate("token-1")
        .expect("deactivate reply context");
    assert!(registry.resolve("token-1").is_err());
    assert!(!lane_file.exists());
}

#[test]
fn reply_registry_supports_multiple_active_tokens_concurrently() {
    let tempdir = TempDir::new().expect("tempdir");
    let repo_root = tempdir.path().to_path_buf();
    let artifacts_dir = repo_root.join(".run/artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    let contexts_dir = repo_root.join(".run/default/run/contexts");
    let mut registry = ReplyRegistry::new(contexts_dir.clone());

    let make_ctx = |token: &str, key: &str, target: i64| ActiveReplyContext {
        token: token.to_string(),
        conversation_key: key.to_string(),
        is_group: false,
        reply_target_id: target,
        source_message_id: target,
        source_sender_id: target,
        source_sender_name: "user".to_string(),
        repo_root: repo_root.clone(),
        artifacts_dir: artifacts_dir.clone(),
    };

    registry
        .activate(make_ctx("tok-a", "private:1", 1))
        .expect("activate a");
    registry
        .activate(make_ctx("tok-b", "private:2", 2))
        .expect("activate b");

    assert_eq!(
        registry
            .resolve("tok-a")
            .expect("resolve a")
            .conversation_key,
        "private:1"
    );
    assert_eq!(
        registry
            .resolve("tok-b")
            .expect("resolve b")
            .conversation_key,
        "private:2"
    );

    registry.deactivate("tok-a").expect("deactivate a");
    assert!(registry.resolve("tok-a").is_err());
    assert_eq!(
        registry
            .resolve("tok-b")
            .expect("resolve b after a removed")
            .conversation_key,
        "private:2"
    );
    assert!(load_active_reply_context(reply_context_file_for(&contexts_dir, "private:2")).is_ok());
    assert!(!repo_root
        .join(".run/default/run/reply_context.json")
        .exists());
}
