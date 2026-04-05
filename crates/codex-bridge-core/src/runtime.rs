//! Runtime path helpers and config writers for the foreground QQ bridge.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::config::RuntimeConfig;

/// Generated tokens written into runtime state files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTokens {
    /// WebUI token used for login.
    pub webui_token: String,
    /// Compatibility websocket token kept in the launcher env file.
    pub ws_token: String,
}

/// Derived filesystem paths for runtime files and logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// Rust project root (`codex-bridge/`).
    pub project_root: PathBuf,
    /// NapCat source repository root located under `deps/NapCatQQ`.
    pub napcat_repo_root: PathBuf,
    /// Root runtime directory.
    pub runtime_root: PathBuf,
    /// Writable artifact directory for codex-created outputs.
    pub artifacts_dir: PathBuf,
    /// Runtime config directory.
    pub config_dir: PathBuf,
    /// Runtime log directory.
    pub logs_dir: PathBuf,
    /// Runtime cache directory.
    pub cache_dir: PathBuf,
    /// Runtime process/state directory.
    pub run_dir: PathBuf,
    /// Persistent state database path.
    pub database_path: PathBuf,
    /// Environment file containing generated tokens.
    pub launcher_env: PathBuf,
    /// Runtime reply-context file for skill-facing reply commands.
    pub reply_context_file: PathBuf,
    /// First-party skills directory.
    pub skills_dir: PathBuf,
    /// Root `.agents` directory.
    pub agents_dir: PathBuf,
    /// `.agents/skills` symlink target path.
    pub agents_skills_link: PathBuf,
    /// Base QQ installation directory.
    pub qq_base: PathBuf,
    /// QQ executable path.
    pub qq_executable: PathBuf,
    /// QQ `resources/app` directory.
    pub resources_app_dir: PathBuf,
    /// QQ `resources/app/app_launcher` directory.
    pub app_launcher_dir: PathBuf,
    /// QQ package manifest path.
    pub qq_package_json: PathBuf,
    /// QQ load script path used to inject NapCat.
    pub qq_load_script: PathBuf,
    /// Current repository NapCat shell build output.
    pub built_shell_dir: PathBuf,
    /// Runtime PID file.
    pub pid_file: PathBuf,
}

impl RuntimePaths {
    /// Build the runtime path set relative to the Rust project root.
    pub fn new(project_root: &Path, qq_executable: Option<PathBuf>) -> Self {
        let runtime_root = project_root.join(".run/default");
        let run_dir = runtime_root.join("run");
        let napcat_repo_root = project_root.join("deps/NapCatQQ");
        let qq_executable = qq_executable.unwrap_or_else(default_qq_executable);
        let qq_base = qq_executable
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| qq_executable.clone());
        let resources_app_dir = qq_base.join("resources/app");
        Self {
            project_root: project_root.to_path_buf(),
            napcat_repo_root: napcat_repo_root.clone(),
            runtime_root: runtime_root.clone(),
            artifacts_dir: project_root.join(".run/artifacts"),
            config_dir: runtime_root.join("config"),
            logs_dir: runtime_root.join("logs"),
            cache_dir: runtime_root.join("cache"),
            database_path: runtime_root.join("state.sqlite3"),
            launcher_env: run_dir.join("launcher.env"),
            reply_context_file: run_dir.join("reply_context.json"),
            skills_dir: project_root.join("skills"),
            agents_dir: project_root.join(".agents"),
            agents_skills_link: project_root.join(".agents/skills"),
            run_dir,
            qq_base,
            qq_executable,
            resources_app_dir: resources_app_dir.clone(),
            app_launcher_dir: resources_app_dir.join("app_launcher"),
            qq_package_json: resources_app_dir.join("package.json"),
            qq_load_script: resources_app_dir.join("loadNapCat.js"),
            built_shell_dir: napcat_repo_root.join("packages/napcat-shell/dist"),
            pid_file: runtime_root.join("run/qq.pid"),
        }
    }
}

/// Prepare runtime directories, token files, and NapCat config files.
pub fn prepare_runtime_state<F, G>(
    paths: &RuntimePaths,
    config: &RuntimeConfig,
    webui_token_factory: F,
    ws_token_factory: G,
) -> Result<RuntimeTokens>
where
    F: FnOnce() -> String,
    G: FnOnce() -> String,
{
    fs::create_dir_all(&paths.runtime_root)?;
    fs::create_dir_all(&paths.config_dir)?;
    fs::create_dir_all(&paths.logs_dir)?;
    fs::create_dir_all(&paths.cache_dir)?;
    fs::create_dir_all(&paths.run_dir)?;
    fs::create_dir_all(&paths.artifacts_dir)?;
    if let Some(parent) = paths.database_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut env_values = read_env_file(&paths.launcher_env)?;
    if !env_values.contains_key("WEBUI_TOKEN") {
        env_values.insert("WEBUI_TOKEN".to_string(), webui_token_factory());
    }
    if !env_values.contains_key("WS_TOKEN") {
        env_values.insert("WS_TOKEN".to_string(), ws_token_factory());
    }

    write_env_file(&paths.launcher_env, &env_values)?;
    write_json_file(
        &paths.config_dir.join("webui.json"),
        &build_webui_config(config, env_values["WEBUI_TOKEN"].as_str()),
    )?;
    write_json_file(
        &paths.config_dir.join("onebot11.json"),
        &build_onebot_config(config, env_values["WS_TOKEN"].as_str()),
    )?;

    Ok(RuntimeTokens {
        webui_token: env_values["WEBUI_TOKEN"].clone(),
        ws_token: env_values["WS_TOKEN"].clone(),
    })
}

/// Prepare runtime state using fresh random tokens when none exist yet.
pub fn prepare_runtime_state_with_defaults(
    paths: &RuntimePaths,
    config: &RuntimeConfig,
) -> Result<RuntimeTokens> {
    prepare_runtime_state(
        paths,
        config,
        || Uuid::new_v4().simple().to_string(),
        || Uuid::new_v4().simple().to_string(),
    )
}

fn default_qq_executable() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Napcat/opt/QQ/qq")
}

fn read_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let mut values = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.insert(key.to_string(), value.to_string());
    }
    Ok(values)
}

fn write_env_file(path: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    let content = values
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect::<String>();
    fs::write(path, content)?;
    Ok(())
}

fn write_json_file(path: &Path, value: &serde_json::Value) -> Result<()> {
    let payload = serde_json::to_string_pretty(value)?;
    fs::write(path, format!("{payload}\n"))?;
    Ok(())
}

fn build_webui_config(config: &RuntimeConfig, token: &str) -> serde_json::Value {
    json!({
        "host": config.webui_host,
        "port": config.webui_port,
        "token": token,
        "loginRate": 10,
        "autoLoginAccount": "",
        "theme": {
            "fontMode": "system",
            "dark": {},
            "light": {},
        },
        "disableWebUI": false,
        "accessControlMode": "none",
        "ipWhitelist": [],
        "ipBlacklist": [],
        "enableXForwardedFor": false,
    })
}

fn build_onebot_config(config: &RuntimeConfig, token: &str) -> serde_json::Value {
    json!({
        "network": {
            "httpServers": [],
            "httpSseServers": [],
            "httpClients": [],
            "websocketServers": [
                {
                    "enable": true,
                    "name": "compatibility",
                    "host": config.websocket_host,
                    "port": config.websocket_port,
                    "reportSelfMessage": false,
                    "enableForcePushEvent": true,
                    "messagePostFormat": "array",
                    "token": token,
                    "debug": false,
                    "heartInterval": 30000,
                }
            ],
            "websocketClients": [],
            "plugins": [],
        },
        "musicSignUrl": "",
        "enableLocalFile2Url": false,
        "parseMultMsg": false,
        "imageDownloadProxy": "",
    })
}
