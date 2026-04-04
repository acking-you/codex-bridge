//! Foreground QQ launcher helpers.

use std::{
    collections::HashMap,
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::Mutex,
};

use crate::{
    config::RuntimeConfig,
    runtime::{prepare_runtime_state_with_defaults, RuntimePaths, RuntimeTokens},
};

const AMD64_DEB_URL: &str =
    "https://dldir1.qq.com/qqfile/qq/QQNT/7516007c/linuxqq_3.2.25-45758_amd64.deb";
const AMD64_RPM_URL: &str =
    "https://dldir1.qq.com/qqfile/qq/QQNT/7516007c/linuxqq_3.2.25-45758_x86_64.rpm";
const ARM64_DEB_URL: &str =
    "https://dldir1.qq.com/qqfile/qq/QQNT/7516007c/linuxqq_3.2.25-45758_arm64.deb";
const ARM64_RPM_URL: &str =
    "https://dldir1.qq.com/qqfile/qq/QQNT/7516007c/linuxqq_3.2.25-45758_aarch64.rpm";

/// Fully prepared launcher state needed to run the foreground QQ process.
#[derive(Debug, Clone)]
pub struct PreparedLaunch {
    /// Filesystem paths for runtime/config state.
    pub paths: RuntimePaths,
    /// Generated runtime tokens used by WebUI auth.
    pub tokens: RuntimeTokens,
}

/// Build the foreground QQ launch command.
pub fn build_launch_command(qq_executable: &Path) -> Vec<String> {
    vec![
        "xvfb-run".to_string(),
        "-a".to_string(),
        qq_executable.display().to_string(),
        "--no-sandbox".to_string(),
    ]
}

/// Prepare QQ, runtime state, and the local NapCat shell injection.
pub async fn prepare_launch(project_root: &Path, config: &RuntimeConfig) -> Result<PreparedLaunch> {
    let paths = RuntimePaths::new(project_root, config.qq_executable.clone());
    ensure_required_commands()?;
    ensure_not_running(&paths.qq_executable)?;
    ensure_qq_installed(&paths).await?;
    ensure_workspace_built(&paths).await?;
    let tokens = prepare_runtime_state_with_defaults(&paths, config)?;
    patch_qq_resources(&paths)?;
    Ok(PreparedLaunch {
        paths,
        tokens,
    })
}

/// Launch QQ in the foreground, stream logs to stdout, and write a launcher
/// log.
pub async fn launch_qq_foreground(prepared: &PreparedLaunch, api_bind: &str) -> Result<()> {
    let log_path = prepared.paths.logs_dir.join("launcher.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open launcher log at {}", log_path.display()))?;
    let command = build_launch_command(&prepared.paths.qq_executable);

    let mut child = Command::new(&command[0]);
    child
        .args(&command[1..])
        .current_dir(&prepared.paths.qq_base)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(build_launch_env(&prepared.paths, prepared.tokens.webui_token.as_str()));
    let mut child = child.spawn().context("spawn foreground QQ process")?;
    let pid = child
        .id()
        .ok_or_else(|| anyhow!("foreground QQ process did not expose a PID"))?;
    fs::write(&prepared.paths.pid_file, format!("{pid}\n"))?;
    print_summary(prepared, pid, api_bind);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("QQ process stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("QQ process stderr is unavailable"))?;
    let log_file = std::sync::Arc::new(Mutex::new(log_file));
    let stdout_task = tokio::spawn(stream_reader(stdout, log_file.clone()));
    let stderr_task = tokio::spawn(stream_reader(stderr, log_file.clone()));

    let (wait_result, interrupted) = tokio::select! {
        result = child.wait() => (
            result.context("wait for QQ process")?,
            false,
        ),
        _ = tokio::signal::ctrl_c() => {
            child.start_kill().context("send kill signal to QQ process")?;
            (
                child.wait().await.context("wait for QQ process after ctrl-c")?,
                true,
            )
        }
    };

    stdout_task.abort();
    stderr_task.abort();
    prepared.paths.pid_file.unlink_if_exists()?;

    if interrupted || wait_result.success() {
        Ok(())
    } else {
        bail!("QQ exited with status: {wait_result}");
    }
}

fn ensure_required_commands() -> Result<()> {
    let required = ["node", "pnpm", "xvfb-run", "curl"];
    let missing = required
        .iter()
        .filter(|name| !command_exists(name))
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        bail!("missing required commands: {}", missing.join(", "))
    }
}

fn command_exists(name: &str) -> bool {
    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path_var).any(|path| path.join(name).exists())
}

fn ensure_not_running(qq_executable: &Path) -> Result<()> {
    let running = find_running_executable_pids(qq_executable)?;
    if running.is_empty() {
        Ok(())
    } else {
        bail!("QQ is already running for {}: {:?}", qq_executable.display(), running)
    }
}

fn find_running_executable_pids(qq_executable: &Path) -> Result<Vec<u32>> {
    let mut matches = Vec::new();
    let target = qq_executable
        .canonicalize()
        .unwrap_or_else(|_| qq_executable.to_path_buf());
    for entry in fs::read_dir("/proc").context("read /proc")? {
        let entry = entry?;
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        let exe_link = entry.path().join("exe");
        let Ok(resolved) = fs::read_link(exe_link) else {
            continue;
        };
        if resolved == target {
            matches.push(pid);
        }
    }
    matches.sort_unstable();
    Ok(matches)
}

async fn ensure_workspace_built(paths: &RuntimePaths) -> Result<()> {
    if !paths.repo_root.join("node_modules").exists() {
        run_checked(["pnpm", "install"], &paths.repo_root).await?;
    }
    run_checked(["pnpm", "build:webui"], &paths.repo_root).await?;
    run_checked(["pnpm", "build:plugin-builtin"], &paths.repo_root).await?;
    run_checked(["pnpm", "build:shell"], &paths.repo_root).await?;
    Ok(())
}

async fn ensure_qq_installed(paths: &RuntimePaths) -> Result<()> {
    if paths.qq_executable.exists() {
        return Ok(());
    }

    let selection = select_linuxqq_package()?;
    let temp_root = env::temp_dir().join(format!("my-qq-bot-install-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root)?;
    }
    fs::create_dir_all(&temp_root)?;
    let archive_name = Path::new(selection.url)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("failed to derive QQ archive name from {}", selection.url))?;
    let archive_path = temp_root.join(archive_name);
    run_checked(
        [
            "curl",
            "-L",
            selection.url,
            "-o",
            archive_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid archive path"))?,
        ],
        &paths.project_root,
    )
    .await?;
    if let Some(parent) = paths.qq_base.parent() {
        fs::create_dir_all(parent)?;
    }
    match selection.format {
        PackageFormat::Deb => {
            run_checked(
                [
                    "dpkg",
                    "-x",
                    archive_path
                        .to_str()
                        .ok_or_else(|| anyhow!("invalid archive path"))?,
                    paths
                        .qq_base
                        .parent()
                        .and_then(|path| path.to_str())
                        .ok_or_else(|| anyhow!("invalid QQ base parent"))?,
                ],
                &paths.project_root,
            )
            .await?;
        },
        PackageFormat::Rpm => {
            let archive = archive_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid archive path"))?;
            let target = paths
                .qq_base
                .parent()
                .and_then(|path| path.to_str())
                .ok_or_else(|| anyhow!("invalid QQ base parent"))?;
            let shell = format!("rpm2cpio '{archive}' | (cd '{target}' && cpio -idmv)");
            run_checked(["bash", "-lc", shell.as_str()], &paths.project_root).await?;
        },
    }

    if temp_root.exists() {
        fs::remove_dir_all(&temp_root)?;
    }
    if paths.qq_executable.exists() {
        Ok(())
    } else {
        bail!("QQ executable not found after install: {}", paths.qq_executable.display())
    }
}

fn select_linuxqq_package() -> Result<PackageSelection> {
    let arch = match env::consts::ARCH {
        "x86_64" => LinuxArch::Amd64,
        "aarch64" => LinuxArch::Arm64,
        other => bail!("unsupported architecture: {other}"),
    };
    if command_exists("dpkg") {
        return Ok(PackageSelection::deb(arch));
    }
    if command_exists("rpm2cpio") && command_exists("cpio") {
        return Ok(PackageSelection::rpm(arch));
    }
    bail!("missing package extraction tooling: need dpkg or rpm2cpio+cpio")
}

fn patch_qq_resources(paths: &RuntimePaths) -> Result<()> {
    let napcat_entry = paths.built_shell_dir.join("napcat.mjs");
    if !napcat_entry.exists() {
        bail!("missing shell build output: {}", napcat_entry.display());
    }

    fs::create_dir_all(&paths.resources_app_dir)?;
    let package_json = fs::read_to_string(&paths.qq_package_json)
        .with_context(|| format!("read {}", paths.qq_package_json.display()))?;
    let mut package_data: Value = serde_json::from_str(&package_json)?;
    package_data["main"] = Value::String("./loadNapCat.js".to_string());
    fs::write(
        &paths.qq_package_json,
        format!("{}\n", serde_json::to_string_pretty(&package_data)?),
    )?;
    fs::write(&paths.qq_load_script, render_load_script(&napcat_entry))?;
    Ok(())
}

fn render_load_script(napcat_entry: &Path) -> String {
    format!(
        "const {{ pathToFileURL }} = require('url');\n(async () => {{\n  await \
         import(pathToFileURL({:?}).href);\n}})();\n",
        napcat_entry.display().to_string()
    )
}

async fn run_checked<const N: usize>(command: [&str; N], cwd: &Path) -> Result<()> {
    let status = Command::new(command[0])
        .args(&command[1..])
        .current_dir(cwd)
        .status()
        .await
        .with_context(|| format!("run command: {}", command.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        bail!("command failed: {}", command.join(" "))
    }
}

fn build_launch_env(paths: &RuntimePaths, webui_token: &str) -> HashMap<String, String> {
    let mut values = env::vars().collect::<HashMap<_, _>>();
    values.insert("NAPCAT_WORKDIR".to_string(), paths.runtime_root.display().to_string());
    values.insert("NAPCAT_WEBUI_SECRET_KEY".to_string(), webui_token.to_string());
    values
}

fn print_summary(prepared: &PreparedLaunch, pid: u32, api_bind: &str) {
    println!("QQ executable: {}", prepared.paths.qq_executable.display());
    println!("Runtime workdir: {}", prepared.paths.runtime_root.display());
    println!("Log directory: {}", prepared.paths.logs_dir.display());
    println!("WebUI URL: http://127.0.0.1:{}/webui", 6099);
    println!("WebUI token: {}", prepared.tokens.webui_token);
    println!("WebUI token file: {}", prepared.paths.launcher_env.display());
    println!("Local API URL: http://{api_bind}");
    println!("QQ PID: {pid}");
    println!("Foreground mode is active. The QR code will be printed below in this terminal.");
}

async fn stream_reader<R>(reader: R, log_file: std::sync::Arc<Mutex<File>>) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        println!("{line}");
        let mut guard = log_file.lock().await;
        writeln!(guard, "{line}")?;
        guard.flush()?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum LinuxArch {
    Amd64,
    Arm64,
}

#[derive(Debug, Clone, Copy)]
enum PackageFormat {
    Deb,
    Rpm,
}

#[derive(Debug, Clone, Copy)]
struct PackageSelection {
    format: PackageFormat,
    url: &'static str,
}

impl PackageSelection {
    fn deb(arch: LinuxArch) -> Self {
        Self {
            format: PackageFormat::Deb,
            url: match arch {
                LinuxArch::Amd64 => AMD64_DEB_URL,
                LinuxArch::Arm64 => ARM64_DEB_URL,
            },
        }
    }

    fn rpm(arch: LinuxArch) -> Self {
        Self {
            format: PackageFormat::Rpm,
            url: match arch {
                LinuxArch::Amd64 => AMD64_RPM_URL,
                LinuxArch::Arm64 => ARM64_RPM_URL,
            },
        }
    }
}

trait PathCleanupExt {
    fn unlink_if_exists(&self) -> Result<()>;
}

impl PathCleanupExt for PathBuf {
    fn unlink_if_exists(&self) -> Result<()> {
        if self.exists() {
            fs::remove_file(self)?;
        }
        Ok(())
    }
}
