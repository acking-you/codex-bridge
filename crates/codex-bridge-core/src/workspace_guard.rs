//! Filesystem permission shaping for the repository workspace.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

/// Active guard that narrows directory creation to the artifact root while
/// keeping existing files writable.
#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    repo_root: PathBuf,
    artifacts_root: PathBuf,
}

impl WorkspaceGuard {
    /// Build a guard for the repository root and writable artifacts root.
    pub fn new(repo_root: impl Into<PathBuf>, artifacts_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            artifacts_root: artifacts_root.into(),
        }
    }

    /// Apply permission shaping and return a restore handle.
    pub fn apply(&self) -> Result<WorkspaceLease> {
        let mut original_modes = Vec::new();
        self.walk_and_apply(&self.repo_root, &mut original_modes)?;
        Ok(WorkspaceLease {
            original_modes,
        })
    }

    fn walk_and_apply(&self, path: &Path, original_modes: &mut Vec<(PathBuf, u32)>) -> Result<()> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("read metadata for {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Ok(());
        }

        let current_mode = metadata.permissions().mode();
        original_modes.push((path.to_path_buf(), current_mode));

        let target_mode = if metadata.is_dir() {
            directory_mode(current_mode, self.path_is_writable_dir(path))
        } else {
            file_mode(current_mode)
        };

        if current_mode != target_mode {
            fs::set_permissions(path, fs::Permissions::from_mode(target_mode))
                .with_context(|| format!("set permissions on {}", path.display()))?;
        }

        if metadata.is_dir() {
            for entry in
                fs::read_dir(path).with_context(|| format!("read directory {}", path.display()))?
            {
                let entry = entry?;
                self.walk_and_apply(&entry.path(), original_modes)?;
            }
        }

        Ok(())
    }

    fn path_is_writable_dir(&self, path: &Path) -> bool {
        path.starts_with(&self.artifacts_root)
    }
}

/// Handle used to restore original workspace permissions.
#[derive(Debug)]
pub struct WorkspaceLease {
    original_modes: Vec<(PathBuf, u32)>,
}

impl WorkspaceLease {
    /// Restore original permissions in reverse traversal order.
    pub fn restore(self) -> Result<()> {
        for (path, mode) in self.original_modes.into_iter().rev() {
            if !path.exists() {
                continue;
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                .with_context(|| format!("restore permissions on {}", path.display()))?;
        }
        Ok(())
    }
}

fn directory_mode(current_mode: u32, writable: bool) -> u32 {
    let readable_executable = current_mode | 0o500;
    if writable {
        readable_executable | 0o200
    } else {
        readable_executable & !0o222
    }
}

fn file_mode(current_mode: u32) -> u32 {
    current_mode | 0o200
}
