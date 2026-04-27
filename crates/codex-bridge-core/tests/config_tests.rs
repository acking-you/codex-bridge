//! Runtime configuration tests.

use std::{
    env, fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use codex_bridge_core::{config::RuntimeConfig, runtime::RuntimePaths};
use tempfile::tempdir;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn default_runtime_paths_live_under_project_run_dir() {
    let project_root = PathBuf::from("/tmp/repo");
    let napcat_root = project_root.join("deps/NapCatQQ");
    let paths = RuntimePaths::new(&project_root, None);
    assert_eq!(paths.runtime_root, project_root.join(".run/default"));
    assert_eq!(paths.artifacts_dir, project_root.join(".run/artifacts"));
    assert_eq!(paths.prompt_dir, project_root.join(".run/default/prompt"));
    assert_eq!(paths.prompt_file, project_root.join(".run/default/prompt/persona.md"));
    assert_eq!(paths.admin_config_file, project_root.join(".run/default/config/admin.toml"));
    assert_eq!(paths.database_path, project_root.join(".run/default/state.sqlite3"));
    assert_eq!(paths.launcher_env, project_root.join(".run/default/run/launcher.env"));
    assert_eq!(paths.skills_dir, project_root.join("skills"));
    assert_eq!(paths.agents_dir, project_root.join(".agents"));
    assert_eq!(paths.agents_skills_link, project_root.join(".agents/skills"));
    assert_eq!(paths.codex_child_home_dir, project_root.join(".run/default/home"));
    assert_eq!(paths.codex_home_dir, project_root.join(".run/default/codex-home"));
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
    assert_eq!(config.pending_approval_capacity, 32);
    assert_eq!(config.approval_timeout_secs, 900);
    assert_eq!(config.group_start_reaction_emoji_id, "282");
    assert_eq!(config.runtime_pool_size, 2);
    assert_eq!(config.lane_pending_capacity, 5);
    assert_eq!(config.history_page_size, 50);
    assert_eq!(config.history_max_pages, 4);
    assert_eq!(config.max_turn_wall_time_secs, 900);
    assert_eq!(config.stalled_turn_timeout_secs, 120);
    assert_eq!(config.slot_restart_backoff_ms, 500);
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
    assert!(paths.skills_dir.is_dir());
    assert!(paths.codex_child_home_dir.is_dir());
    assert!(paths.codex_home_dir.is_dir());
    assert!(paths.prompt_dir.is_dir());
    assert!(paths.prompt_file.is_file());
    assert!(paths.admin_config_file.is_file());
    let symlink_metadata = fs::symlink_metadata(&paths.agents_skills_link).unwrap();
    assert!(symlink_metadata.file_type().is_symlink());
    assert_eq!(fs::read_link(&paths.agents_skills_link).unwrap(), paths.skills_dir);
    assert!(paths.config_dir.join("onebot11.json").is_file());
    let launcher_env = fs::read_to_string(&paths.launcher_env).unwrap();
    assert!(launcher_env.contains("WEBUI_TOKEN=webui"));
    let prompt = fs::read_to_string(&paths.prompt_file).unwrap();
    assert!(prompt.contains("# Identity"));
    assert!(prompt.contains("# Voice and vitality"));
    let admin_config = fs::read_to_string(&paths.admin_config_file).unwrap();
    assert!(admin_config.contains("admin_user_id = 2394626220"));
}

#[test]
fn prepare_runtime_state_copies_codex_config_toml_into_isolated_home() {
    let _guard = env_lock().lock().unwrap();
    let project_root = tempdir().unwrap();
    let qq_root = tempdir().unwrap();
    let source_codex_home = tempdir().unwrap();
    let source_config = source_codex_home.path().join("config.toml");
    fs::write(
        &source_config,
        "model_provider = \"staticflow\"\n[model_providers.staticflow]\nbase_url = \"http://127.0.0.1:39080/api/llm-gateway/v1\"\n",
    )
    .unwrap();

    let previous_codex_home = env::var_os("CODEX_HOME");
    env::set_var("CODEX_HOME", source_codex_home.path());

    let paths = RuntimePaths::new(project_root.path(), Some(qq_root.path().join("qq")));
    let config = RuntimeConfig::default();

    let result = codex_bridge_core::runtime::prepare_runtime_state(
        &paths,
        &config,
        || "webui".into(),
        || "ws".into(),
    );

    match previous_codex_home {
        Some(value) => env::set_var("CODEX_HOME", value),
        None => env::remove_var("CODEX_HOME"),
    }

    result.unwrap();

    let isolated_config = paths.codex_home_dir.join("config.toml");
    assert!(isolated_config.is_file());
    assert_eq!(
        fs::read_to_string(isolated_config).unwrap(),
        fs::read_to_string(source_config).unwrap()
    );
}

#[test]
fn prepare_runtime_state_keeps_existing_isolated_codex_config_toml() {
    let _guard = env_lock().lock().unwrap();
    let project_root = tempdir().unwrap();
    let qq_root = tempdir().unwrap();
    let source_codex_home = tempdir().unwrap();
    let source_config = source_codex_home.path().join("config.toml");
    fs::write(&source_config, "model = \"from-source\"\n").unwrap();

    let previous_codex_home = env::var_os("CODEX_HOME");
    env::set_var("CODEX_HOME", source_codex_home.path());

    let paths = RuntimePaths::new(project_root.path(), Some(qq_root.path().join("qq")));
    fs::create_dir_all(&paths.codex_home_dir).unwrap();
    let isolated_config = paths.codex_home_dir.join("config.toml");
    fs::write(&isolated_config, "model = \"existing-runtime\"\n").unwrap();

    let config = RuntimeConfig::default();
    let result = codex_bridge_core::runtime::prepare_runtime_state(
        &paths,
        &config,
        || "webui".into(),
        || "ws".into(),
    );

    match previous_codex_home {
        Some(value) => env::set_var("CODEX_HOME", value),
        None => env::remove_var("CODEX_HOME"),
    }

    result.unwrap();

    assert_eq!(fs::read_to_string(isolated_config).unwrap(), "model = \"existing-runtime\"\n");
}
