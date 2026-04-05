//! Workspace permission-shaping tests.

use std::{
    fs,
    path::{Path, PathBuf},
};

use codex_bridge_core::workspace_guard::WorkspaceGuard;
use tempfile::TempDir;

struct WorkspaceFixture {
    tempdir: TempDir,
}

impl WorkspaceFixture {
    fn tracked_tree() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".run/artifacts")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
        fs::write(root.join(".run/artifacts/existing.txt"), "artifact\n").unwrap();
        Self {
            tempdir,
        }
    }

    fn repo_root(&self) -> &Path {
        self.tempdir.path()
    }

    fn artifacts_root(&self) -> PathBuf {
        self.tempdir.path().join(".run/artifacts")
    }

    fn can_write_existing(&self, relative: &str) -> bool {
        fs::OpenOptions::new()
            .write(true)
            .open(self.tempdir.path().join(relative))
            .is_ok()
    }

    fn can_create(&self, relative: &str) -> bool {
        let path = self.tempdir.path().join(relative);
        let result = fs::write(&path, "created\n");
        if result.is_ok() {
            let _ = fs::remove_file(path);
            true
        } else {
            false
        }
    }
}

#[test]
fn workspace_guard_blocks_new_files_outside_artifacts() {
    let fixture = WorkspaceFixture::tracked_tree();
    let guard = WorkspaceGuard::new(fixture.repo_root(), fixture.artifacts_root());
    let lease = guard.apply().unwrap();

    assert!(fixture.can_write_existing("src/lib.rs"));
    assert!(!fixture.can_create("src/new_file.rs"));
    assert!(fixture.can_create(".run/artifacts/output.md"));

    lease.restore().unwrap();
}

#[test]
fn workspace_guard_restore_reverts_directory_permissions() {
    let fixture = WorkspaceFixture::tracked_tree();
    let guard = WorkspaceGuard::new(fixture.repo_root(), fixture.artifacts_root());
    let lease = guard.apply().unwrap();
    assert!(!fixture.can_create("src/temp.txt"));

    lease.restore().unwrap();
    assert!(fixture.can_create("src/temp.txt"));
}
