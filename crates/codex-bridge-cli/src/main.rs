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
    api,
    codex_runtime::{CodexRuntime, CodexRuntimeConfig},
    config::RuntimeConfig,
    launcher, napcat, orchestrator,
    outbound::ReplyRequest,
    reply_context::load_active_reply_context,
    runtime::{load_admin_config, RuntimePaths},
    service::ServiceState,
    state_store::StateStore,
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
        Commands::Reply {
            text,
            image,
            file,
        } => reply_command(&config, &project_root()?, text, image, file).await,
        Commands::SendPrivate {
            user_id,
            text,
        } => post_local_json(
            &config,
            "/api/messages/private",
            json!({
                "user_id": user_id,
                "text": text,
            }),
        )
        .await
        .map(|_| ()),
        Commands::SendGroup {
            group_id,
            text,
        } => post_local_json(
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
    let state = ServiceState::with_control_and_reply_context(
        command_tx,
        control_tx,
        prepared.paths.reply_context_file.clone(),
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
    let codex = Arc::new(
        CodexRuntime::new(CodexRuntimeConfig::new(
            codex_repo_root(&project_root),
            project_root.clone(),
            prepared.paths.prompt_file.clone(),
            prepared.paths.codex_child_home_dir.clone(),
            prepared.paths.codex_home_dir.clone(),
        ))
        .await?,
    );
    info!("codex runtime ready");
    let orchestrator_config = orchestrator::OrchestratorConfig {
        queue_capacity: config.queue_capacity,
        repo_root: project_root.clone(),
        artifacts_dir: prepared.paths.artifacts_dir.clone(),
        prompt_file: prepared.paths.prompt_file.clone(),
        group_start_reaction_emoji_id: config.group_start_reaction_emoji_id.clone(),
        admin_user_id: admin_config.admin_user_id,
        pending_approval_capacity: config.pending_approval_capacity,
        approval_timeout_secs: config.approval_timeout_secs,
    };
    let orchestrator_task = tokio::spawn(async move {
        if let Err(error) =
            orchestrator::run(codex_state, control_rx, codex, store, orchestrator_config).await
        {
            eprintln!("orchestrator stopped: {error:#}");
        }
    });
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

async fn reply_command(
    config: &RuntimeConfig,
    project_root: &Path,
    text: Option<String>,
    image: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<()> {
    let paths = RuntimePaths::new(project_root, Option::<PathBuf>::None);
    let context = load_active_reply_context(&paths.reply_context_file)?;
    let payload = ReplyRequest {
        token: context.token,
        text,
        image,
        file,
    };
    post_local_json(config, "/api/reply", serde_json::to_value(payload)?)
        .await
        .map(|_| ())
}
