//! Reply-context registry tests.

use codex_bridge_core::reply_context::{
    load_active_reply_context, ActiveReplyContext, ReplyRegistry,
};
use tempfile::TempDir;

#[test]
fn reply_context_token_can_send_multiple_times_until_revoked() {
    let tempdir = TempDir::new().expect("tempdir");
    let repo_root = tempdir.path().to_path_buf();
    let artifacts_dir = repo_root.join(".run/artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");
    let context_file = repo_root.join(".run/default/run/reply_context.json");
    let mut registry = ReplyRegistry::new(context_file.clone());
    let context = ActiveReplyContext {
        token: "token-1".to_string(),
        conversation_key: "group:777".to_string(),
        is_group: true,
        reply_target_id: 777,
        source_message_id: 9901,
        source_sender_id: 42,
        source_sender_name: "alice".to_string(),
        repo_root,
        artifacts_dir,
    };

    registry
        .activate(context.clone())
        .expect("activate reply context");

    assert_eq!(registry.resolve("token-1").expect("resolve once"), context);
    assert_eq!(registry.resolve("token-1").expect("resolve twice"), context);
    assert_eq!(
        load_active_reply_context(&context_file)
            .expect("load context from disk")
            .token,
        "token-1"
    );

    registry.deactivate().expect("deactivate reply context");
    assert!(registry.resolve("token-1").is_err());
    assert!(load_active_reply_context(&context_file).is_err());
}
