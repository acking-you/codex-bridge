//! Runtime configuration tests.

use std::{fs, path::PathBuf};

use codex_bridge_core::{config::RuntimeConfig, runtime::RuntimePaths};
use tempfile::tempdir;

#[test]
fn default_runtime_paths_live_under_project_run_dir() {
    let project_root = PathBuf::from("/tmp/repo");
    let napcat_root = project_root.join("deps/NapCatQQ");
    let paths = RuntimePaths::new(&project_root, None);
    assert_eq!(paths.runtime_root, project_root.join(".run/default"));
    assert_eq!(paths.artifacts_dir, project_root.join(".run/artifacts"));
    assert_eq!(paths.database_path, project_root.join(".run/default/state.sqlite3"));
    assert_eq!(paths.launcher_env, project_root.join(".run/default/run/launcher.env"));
    assert_eq!(paths.reply_context_file, project_root.join(".run/default/run/reply_context.json"));
    assert_eq!(paths.skills_dir, project_root.join("skills"));
    assert_eq!(paths.agents_dir, project_root.join(".agents"));
    assert_eq!(paths.agents_skills_link, project_root.join(".agents/skills"));
    assert_eq!(paths.napcat_repo_root, napcat_root);
    assert_eq!(paths.built_shell_dir, napcat_root.join("packages/napcat-shell/dist"));
}

#[test]
fn default_runtime_config_uses_formal_ws_defaults() {
    let config = RuntimeConfig::default();
    assert_eq!(config.api_bind, "127.0.0.1:36111");
    assert_eq!(config.webui_port, 6099);
    assert_eq!(config.websocket_host, "127.0.0.1");
    assert_eq!(config.websocket_port, 3012);
    assert_eq!(config.queue_capacity, 5);
    assert_eq!(config.group_start_reaction_emoji_id, "282");
}

#[test]
fn prepare_runtime_state_creates_artifacts_dir() {
    let project_root = tempdir().unwrap();
    let qq_root = tempdir().unwrap();
    let paths = RuntimePaths::new(project_root.path(), Some(qq_root.path().join("qq")));
    let config = RuntimeConfig::default();

    let _tokens = codex_bridge_core::runtime::prepare_runtime_state(
        &paths,
        &config,
        || "webui".into(),
        || "ws".into(),
    )
    .unwrap();

    assert!(paths.runtime_root.is_dir());
    assert!(paths.artifacts_dir.is_dir());
    assert!(paths.config_dir.join("onebot11.json").is_file());
    let launcher_env = fs::read_to_string(&paths.launcher_env).unwrap();
    assert!(launcher_env.contains("WEBUI_TOKEN=webui"));
}
