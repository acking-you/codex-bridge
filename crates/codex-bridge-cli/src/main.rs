//! Binary entrypoint for Codex Bridge.

use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use clap::Parser;
use codex_bridge_cli::{
    cli::{Cli, Commands},
    task_exit::background_task_exit_error,
};
use codex_bridge_core::{
    api, codex_runtime::CodexRuntimeConfig, config::RuntimeConfig, launcher,
    model_capabilities::ModelRegistry, napcat, orchestrator, runtime::load_admin_config,
    runtime_pool::RuntimePool, service::ServiceState, state_store::StateStore,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tracing::info;
use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str = "warn,codex_bridge_cli=info,codex_bridge_core=info";

#[tokio::main]
async fn main() -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let config = RuntimeConfig::default();
    match cli.command {
        Commands::Run => run_command(config).await,
        Commands::Status => get_local_json(&config, "/api/status").await.map(|_| ()),
        Commands::Queue => get_local_json(&config, "/api/queue").await.map(|_| ()),
        Commands::Cancel => post_local_json(&config, "/api/tasks/cancel", json!({}))
            .await
            .map(|_| ()),
        Commands::RetryLast => post_local_json(&config, "/api/tasks/retry-last", json!({}))
            .await
            .map(|_| ()),
        Commands::Reply { text, image, file } => {
            reply_command(&config, &project_root()?, text, image, file).await
        },
        Commands::SendPrivate { user_id, text } => post_local_json(
            &config,
            "/api/messages/private",
            json!({
                "user_id": user_id,
                "text": text,
            }),
        )
        .await
        .map(|_| ()),
        Commands::SendGroup { group_id, text } => post_local_json(
            &config,
            "/api/messages/group",
            json!({
                "group_id": group_id,
                "text": text,
            }),
        )
        .await
        .map(|_| ()),
        Commands::Friends => get_local_json(&config, "/api/friends").await.map(|_| ()),
        Commands::Groups => get_local_json(&config, "/api/groups").await.map(|_| ()),
    }
}

async fn run_command(config: RuntimeConfig) -> Result<()> {
    let project_root = project_root()?;
    let prepared = launcher::prepare_launch(&project_root, &config).await?;
    info!(
        project_root = %project_root.display(),
        api_bind = %config.api_bind,
        websocket_host = %config.websocket_host,
        websocket_port = config.websocket_port,
        "starting codex-bridge runtime"
    );
    let (command_tx, command_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(64);
    let state = ServiceState::with_control_and_reply_context_paths(
        command_tx,
        control_tx,
        prepared.paths.reply_contexts_dir.clone(),
    );

    let api_bind = config.api_bind.clone();
    let api_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = api::serve(api_bind.as_str(), api_state).await {
            eprintln!("local api stopped: {error:#}");
        }
    });

    let bridge_state = state.clone();
    let bridge_config = config.clone();
    let bridge_tokens = prepared.tokens.clone();
    let bridge_task = tokio::spawn(async move {
        napcat::run_bridge_loop(bridge_config, bridge_tokens, bridge_state, command_rx).await
    });

    let codex_state = state.clone();
    let store = Arc::new(Mutex::new(StateStore::open(&prepared.paths.database_path)?));
    let admin_config = load_admin_config(&prepared.paths.admin_config_file)?;

    // Wire the hot-reloadable capability pipeline:
    //   1. Point ServiceState at the TOML file on disk so /api/capability/reload
    //      can re-read it later.
    //   2. Share the prompt-block Arc with the Codex runtime so updates via
    //      set_capabilities / reload_capabilities are visible on the next
    //      thread/start or thread/resume.
    //   3. Load the initial registry and publish it (also rerenders the shared
    //      prompt block atomically).
    state.set_capabilities_file(prepared.paths.model_capabilities_file.clone());
    let mut codex_runtime_config = CodexRuntimeConfig::new(
        codex_repo_root(&project_root),
        project_root.clone(),
        prepared.paths.prompt_file.clone(),
        prepared.paths.codex_child_home_dir.clone(),
        prepared.paths.codex_home_dir.clone(),
    );
    codex_runtime_config.capabilities_block = state.capabilities_prompt_block_handle();
    codex_runtime_config.admin_user_id = admin_config.admin_user_id;
    codex_runtime_config.reply_contexts_dir = prepared.paths.reply_contexts_dir.clone();
    let initial_registry =
        Arc::new(ModelRegistry::load_from_file(&prepared.paths.model_capabilities_file)?);
    let initial_count = initial_registry.len();
    state.set_capabilities(initial_registry).await;
    info!(
        capability_count = initial_count,
        capabilities_file = %prepared.paths.model_capabilities_file.display(),
        "model capability registry loaded"
    );

    let codex = Arc::new(
        RuntimePool::spawn_from_config(
            &codex_runtime_config,
            &prepared.paths.runtime_root,
            config.runtime_pool_size,
        )
        .await?,
    );
    info!(runtime_pool_size = config.runtime_pool_size, "codex runtime pool ready");
    let orchestrator_config = orchestrator::OrchestratorConfig {
        queue_capacity: config.queue_capacity,
        lane_pending_capacity: config.lane_pending_capacity,
        runtime_pool_size: config.runtime_pool_size,
        repo_root: project_root.clone(),
        artifacts_dir: prepared.paths.artifacts_dir.clone(),
        prompt_file: prepared.paths.prompt_file.clone(),
        group_start_reaction_emoji_id: config.group_start_reaction_emoji_id.clone(),
        admin_user_id: admin_config.admin_user_id,
        trusted_group_ids: admin_config.trusted_group_ids.clone(),
        pending_approval_capacity: config.pending_approval_capacity,
        approval_timeout_secs: config.approval_timeout_secs,
    };
    let orchestrator_task = tokio::spawn(orchestrator_supervisor(
        codex_state,
        control_rx,
        codex,
        store,
        orchestrator_config,
    ));
    let launcher_task = launcher::launch_qq_foreground(&prepared, config.api_bind.as_str());
    tokio::pin!(launcher_task);

    let result = tokio::select! {
        result = &mut launcher_task => result,
        result = bridge_task => {
            background_task_exit_error(
                "bridge runtime",
                result.map_err(anyhow::Error::from)?,
            )
        }
    };

    orchestrator_task.abort();
    result
}

fn project_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(parent) = manifest_dir.parent() else {
        anyhow::bail!("failed to derive crates directory from {}", manifest_dir.display());
    };
    let Some(project_root) = parent.parent() else {
        anyhow::bail!("failed to derive project root from {}", parent.display());
    };
    Ok(project_root.to_path_buf())
}

fn codex_repo_root(project_root: &Path) -> PathBuf {
    env::var_os("CODEX_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join("deps/codex/codex-rs"))
}

async fn get_local_json(config: &RuntimeConfig, path: &str) -> Result<Value> {
    let url = local_url(config, path);
    let response = reqwest::Client::new().get(url).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("local api request failed: {status} {body}");
    }
    let value: Value = serde_json::from_str(&body)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(value)
}

async fn post_local_json(config: &RuntimeConfig, path: &str, payload: Value) -> Result<Value> {
    let url = local_url(config, path);
    let response = reqwest::Client::new()
        .post(url)
        .json(&payload)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("local api request failed: {status} {body}");
    }
    let value: Value = serde_json::from_str(&body)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(value)
}

fn local_url(config: &RuntimeConfig, path: &str) -> String {
    format!("http://{}{}", config.api_bind, path)
}

async fn orchestrator_supervisor(
    state: ServiceState,
    mut control_rx: mpsc::Receiver<codex_bridge_core::service::ServiceCommand>,
    codex: Arc<dyn codex_bridge_core::codex_runtime::CodexExecutor>,
    store: Arc<Mutex<StateStore>>,
    config: orchestrator::OrchestratorConfig,
) {
    use std::time::Duration;

    let mut backoff_ms: u64 = 500;
    const MAX_BACKOFF_MS: u64 = 30_000;

    loop {
        let (forward_tx, forward_rx) =
            mpsc::channel::<codex_bridge_core::service::ServiceCommand>(128);

        let forwarder = tokio::spawn(async move {
            while let Some(cmd) = control_rx.recv().await {
                if forward_tx.send(cmd).await.is_err() {
                    break;
                }
            }
            control_rx
        });

        let result = orchestrator::run(
            state.clone(),
            forward_rx,
            codex.clone(),
            store.clone(),
            config.clone(),
        )
        .await;

        control_rx = match forwarder.await {
            Ok(rx) => rx,
            Err(join_err) => {
                eprintln!("orchestrator supervisor: forwarder join failed: {join_err:#}");
                return;
            },
        };

        match result {
            Ok(()) => {
                eprintln!("orchestrator exited cleanly; supervisor stopping");
                return;
            },
            Err(error) => {
                eprintln!("orchestrator returned error, restarting in {backoff_ms}ms: {error:#}");
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            },
        }
    }
}

async fn reply_command(
    _config: &RuntimeConfig,
    _project_root: &Path,
    _text: Option<String>,
    _image: Option<PathBuf>,
    _file: Option<PathBuf>,
) -> Result<()> {
    anyhow::bail!(
        "codex-bridge reply no longer infers a singleton reply context; use the reply-current \
         skill with --context-file"
    )
}
