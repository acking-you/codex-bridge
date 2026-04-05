//! Launcher and runtime state tests.

use std::path::PathBuf;

use codex_bridge_core::{
    config::RuntimeConfig,
    launcher::build_launch_command,
    runtime::{prepare_runtime_state, RuntimePaths},
};
use tempfile::TempDir;

#[test]
fn launch_command_uses_xvfb_run() {
    let command = build_launch_command(PathBuf::from("/tmp/QQ/qq").as_path());
    assert_eq!(command, vec![
        "xvfb-run".to_string(),
        "-a".to_string(),
        "/tmp/QQ/qq".to_string(),
        "--no-sandbox".to_string(),
    ]);
}

#[test]
fn prepare_runtime_state_writes_expected_files() {
    let tempdir = TempDir::new().expect("tempdir");
    let project_root = tempdir.path().join("codex-bridge");
    std::fs::create_dir_all(&project_root).expect("project root");
    let paths = RuntimePaths::new(&project_root, None);
    let config = RuntimeConfig::default();

    let tokens = prepare_runtime_state(
        &paths,
        &config,
        || "webui-token".to_string(),
        || "ws-token".to_string(),
    )
    .expect("prepare runtime state");

    assert_eq!(tokens.webui_token, "webui-token");
    assert_eq!(tokens.ws_token, "ws-token");
    assert!(paths.config_dir.join("webui.json").exists());
    assert!(paths.config_dir.join("onebot11.json").exists());
    assert!(paths.launcher_env.exists());
    assert_eq!(paths.napcat_repo_root, project_root.join("deps/NapCatQQ"));
    assert_eq!(
        paths.built_shell_dir,
        project_root.join("deps/NapCatQQ/packages/napcat-shell/dist")
    );
}

#[test]
fn prepare_runtime_state_enables_formal_websocket_server() {
    let tempdir = TempDir::new().expect("tempdir");
    let project_root = tempdir.path().join("codex-bridge");
    std::fs::create_dir_all(&project_root).expect("project root");
    let paths = RuntimePaths::new(&project_root, None);
    let config = RuntimeConfig::default();

    prepare_runtime_state(&paths, &config, || "webui-token".to_string(), || "ws-token".to_string())
        .expect("prepare runtime state");

    let onebot: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(paths.config_dir.join("onebot11.json"))
            .expect("read onebot config"),
    )
    .expect("parse onebot config");

    assert_eq!(onebot["network"]["websocketServers"][0]["enable"], serde_json::Value::Bool(true));
}
