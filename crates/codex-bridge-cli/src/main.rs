//! Binary entrypoint for Codex Bridge.

use std::{env, path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::Parser;
use codex_bridge_cli::cli::{Cli, Commands};
use codex_bridge_core::{
    api,
    codex_runtime::{CodexRuntime, CodexRuntimeConfig},
    config::RuntimeConfig,
    launcher, napcat, orchestrator,
    service::ServiceState,
    state_store::StateStore,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
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
    let (command_tx, command_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(64);
    let state = ServiceState::with_control(command_tx, control_tx);

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
    tokio::spawn(async move {
        if let Err(error) =
            napcat::run_bridge_loop(bridge_config, bridge_tokens, bridge_state, command_rx).await
        {
            eprintln!("bridge runtime stopped: {error:#}");
        }
    });

    let codex_state = state.clone();
    let _store = Arc::new(Mutex::new(StateStore::open(&prepared.paths.database_path)?));
    let (event_tx, event_rx) = mpsc::channel(256);
    let mut event_sub = codex_state.subscribe_events();
    tokio::spawn(async move {
        while let Ok(event) = event_sub.recv().await {
            let _ = event_tx.send(event).await;
        }
    });
    let codex = Arc::new(
        CodexRuntime::new(CodexRuntimeConfig::new(codex_repo_root(), project_root.clone())).await?,
    );
    tokio::spawn(async move {
        if let Err(error) =
            orchestrator::run(codex_state, event_rx, control_rx, codex.as_ref()).await
        {
            eprintln!("orchestrator stopped: {error:#}");
        }
    });

    launcher::launch_qq_foreground(&prepared, config.api_bind.as_str()).await
}

fn project_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(project_root) = manifest_dir.parent() else {
        anyhow::bail!("failed to derive project root from {}", manifest_dir.display());
    };
    Ok(project_root.to_path_buf())
}

fn codex_repo_root() -> PathBuf {
    env::var_os("CODEX_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/ts_user/rust_pro/codex/codex-rs"))
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
