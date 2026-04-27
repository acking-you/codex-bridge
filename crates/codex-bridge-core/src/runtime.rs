//! Runtime path helpers and config writers for the foreground QQ bridge.

use std::{
    collections::BTreeMap,
    env, fs,
    os::unix::fs as unix_fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::{
    admin_approval::{default_admin_config_template, AdminConfig},
    config::RuntimeConfig,
    system_prompt::ensure_persona_file,
};

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
    /// Runtime-owned admin approval config file.
    pub admin_config_file: PathBuf,
    /// Runtime-owned model capabilities config file (loaded into the
    /// [`crate::model_capabilities::ModelRegistry`] on boot).
    pub model_capabilities_file: PathBuf,
    /// Example / template counterpart of
    /// [`Self::model_capabilities_file`], seeded on first boot so the
    /// operator knows which keys the real file can declare.
    pub model_capabilities_example_file: PathBuf,
    /// Runtime log directory.
    pub logs_dir: PathBuf,
    /// Runtime cache directory.
    pub cache_dir: PathBuf,
    /// Runtime prompt directory.
    pub prompt_dir: PathBuf,
    /// Runtime-owned operator-editable system prompt file.
    pub prompt_file: PathBuf,
    /// Runtime process/state directory.
    pub run_dir: PathBuf,
    /// Persistent state database path.
    pub database_path: PathBuf,
    /// Environment file containing generated tokens.
    pub launcher_env: PathBuf,
    /// Directory holding per-conversation reply-context files. Each
    /// active task writes `<conversation_key>.json` here; Codex is told
    /// the absolute path in its developer_instructions so concurrent
    /// tasks never race on the legacy singleton mirror.
    pub reply_contexts_dir: PathBuf,
    /// First-party skills directory.
    pub skills_dir: PathBuf,
    /// Root `.agents` directory.
    pub agents_dir: PathBuf,
    /// `.agents/skills` symlink target path.
    pub agents_skills_link: PathBuf,
    /// Isolated HOME directory for the child codex app-server process.
    pub codex_child_home_dir: PathBuf,
    /// Isolated CODEX_HOME directory for the child codex app-server process.
    pub codex_home_dir: PathBuf,
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
            admin_config_file: runtime_root.join("config/admin.toml"),
            model_capabilities_file: runtime_root.join("config/model_capabilities.toml"),
            model_capabilities_example_file: runtime_root
                .join("config/model_capabilities.toml.example"),
            logs_dir: runtime_root.join("logs"),
            cache_dir: runtime_root.join("cache"),
            prompt_dir: runtime_root.join("prompt"),
            prompt_file: runtime_root.join("prompt/persona.md"),
            database_path: runtime_root.join("state.sqlite3"),
            launcher_env: run_dir.join("launcher.env"),
            reply_contexts_dir: run_dir.join("contexts"),
            skills_dir: project_root.join("skills"),
            agents_dir: project_root.join(".agents"),
            agents_skills_link: project_root.join(".agents/skills"),
            codex_child_home_dir: runtime_root.join("home"),
            codex_home_dir: runtime_root.join("codex-home"),
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

/// Return the slot-specific runtime directory under the shared runtime root.
pub fn runtime_slot_dir(runtime_root: &Path, slot_id: usize) -> PathBuf {
    runtime_root.join("slots").join(format!("slot-{slot_id}"))
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
    fs::create_dir_all(&paths.prompt_dir)?;
    fs::create_dir_all(&paths.run_dir)?;
    fs::create_dir_all(&paths.reply_contexts_dir)?;
    fs::create_dir_all(&paths.artifacts_dir)?;
    fs::create_dir_all(&paths.skills_dir)?;
    fs::create_dir_all(&paths.agents_dir)?;
    fs::create_dir_all(&paths.codex_child_home_dir)?;
    fs::create_dir_all(&paths.codex_home_dir)?;
    if let Some(parent) = paths.database_path.parent() {
        fs::create_dir_all(parent)?;
    }
    ensure_skills_symlink(paths)?;
    ensure_persona_file(&paths.prompt_file)?;
    warn_about_legacy_system_prompt(paths);
    ensure_admin_config_file(&paths.admin_config_file)?;
    ensure_model_capabilities_example(&paths.model_capabilities_example_file)?;
    let _admin_config = AdminConfig::from_file(&paths.admin_config_file)?;
    sync_codex_config(paths)?;
    seed_codex_auth(paths)?;

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

fn ensure_skills_symlink(paths: &RuntimePaths) -> Result<()> {
    if let Ok(link_metadata) = fs::symlink_metadata(&paths.agents_skills_link) {
        if link_metadata.file_type().is_symlink() {
            let target = fs::read_link(&paths.agents_skills_link)?;
            if target == paths.skills_dir {
                return Ok(());
            }
        }
        if link_metadata.is_dir() {
            fs::remove_dir_all(&paths.agents_skills_link)?;
        } else {
            fs::remove_file(&paths.agents_skills_link)?;
        }
    }
    unix_fs::symlink(&paths.skills_dir, &paths.agents_skills_link)?;
    Ok(())
}

fn ensure_admin_config_file(path: &Path) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, default_admin_config_template())?;
    Ok(())
}

/// Log a one-shot warning when a legacy monolithic `system_prompt.md`
/// is still sitting next to the new `persona.md`. Earlier versions of
/// the bridge stored the full prompt (identity + bridge protocol +
/// permissions + ...) in one file; since the layered refactor, only
/// `persona.md` is read and the bridge protocol / admin context /
/// capabilities sections are assembled in code. The legacy file is
/// harmless but misleading — operators should delete it once they
/// confirm the new persona file behaves correctly.
fn warn_about_legacy_system_prompt(paths: &RuntimePaths) {
    let legacy = paths.prompt_dir.join("system_prompt.md");
    if legacy.is_file() {
        tracing::warn!(
            legacy_file = %legacy.display(),
            active_file = %paths.prompt_file.display(),
            "legacy monolithic system_prompt.md is no longer read; edit persona.md instead \
             and delete the legacy file to stop seeing this warning",
        );
    }
}

/// Default template shipped as `model_capabilities.toml.example`. The
/// real `model_capabilities.toml` is created by the operator by copying
/// the example and filling in the api_key — it is gitignored.
const DEFAULT_MODEL_CAPABILITIES_TEMPLATE: &str =
    include_str!("../assets/model_capabilities.toml.example");

fn ensure_model_capabilities_example(path: &Path) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULT_MODEL_CAPABILITIES_TEMPLATE)?;
    Ok(())
}

/// Load the runtime-owned admin approval config.
pub fn load_admin_config(path: &Path) -> Result<AdminConfig> {
    AdminConfig::from_file(path)
}

fn sync_codex_config(paths: &RuntimePaths) -> Result<()> {
    let Some(source_codex_home) = source_codex_home_dir() else {
        return Ok(());
    };
    let source_config = source_codex_home.join("config.toml");
    if !source_config.is_file() {
        return Ok(());
    }
    let destination_config = paths.codex_home_dir.join("config.toml");
    if destination_config.exists() {
        return Ok(());
    }
    fs::copy(source_config, destination_config)?;
    Ok(())
}

fn seed_codex_auth(paths: &RuntimePaths) -> Result<()> {
    let source_auth = source_codex_home_dir().map(|home| home.join("auth.json"));
    let destination_auth = paths.codex_home_dir.join("auth.json");
    if destination_auth.exists() {
        return Ok(());
    }
    let Some(source_auth) = source_auth else {
        return Ok(());
    };
    if source_auth.is_file() {
        fs::copy(&source_auth, &destination_auth)?;
    }
    Ok(())
}

fn source_codex_home_dir() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
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
