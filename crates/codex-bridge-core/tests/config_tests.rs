//! Runtime configuration tests.

use std::path::PathBuf;

use codex_bridge_core::{config::RuntimeConfig, runtime::RuntimePaths};

#[test]
fn default_runtime_paths_live_under_project_run_dir() {
    let repo_root = PathBuf::from("/tmp/repo");
    let paths = RuntimePaths::new(&repo_root, None);
    assert_eq!(paths.runtime_root, repo_root.join(".run/default"));
    assert_eq!(paths.database_path, repo_root.join(".run/default/state.sqlite3"));
    assert_eq!(paths.launcher_env, repo_root.join(".run/default/run/launcher.env"));
}

#[test]
fn default_runtime_config_uses_formal_ws_defaults() {
    let config = RuntimeConfig::default();
    assert_eq!(config.api_bind, "127.0.0.1:36111");
    assert_eq!(config.webui_port, 6099);
    assert_eq!(config.websocket_host, "127.0.0.1");
    assert_eq!(config.websocket_port, 3012);
    assert_eq!(config.queue_capacity, 5);
}
