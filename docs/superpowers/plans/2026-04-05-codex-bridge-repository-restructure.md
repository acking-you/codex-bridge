# Codex Bridge Repository Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a standalone `codex-bridge` repository that preserves the current Rust bridge history, pushes NapCat dependency changes to `acking-you/NapCatQQ`, and consumes both NapCat and Codex through `deps/` Git submodules.

**Architecture:** Treat the current repository as the migration source only. First extract and publish NapCat-specific changes on a clean fork branch, then seed a new `codex-bridge` repository from the `my_qq_bot` path history, reshape that repository into a `crates/`-based Rust workspace, and finally add `deps/NapCatQQ` plus `deps/codex` submodules before rewiring runtime paths.

**Tech Stack:** Git worktrees, `git subtree split`, Git submodules, Rust workspace manifests, existing `cargo`/`pnpm`/`python3` verification commands, current `my_qq_bot` launcher/orchestrator code.

**Assumptions:** `git@github.com:acking-you/codex-bridge.git` exists before Task 2 and is empty (no auto-generated README commit). The new local repository root is `/home/ts_user/llm_pro/codex-bridge`. The current source repository stays at `/home/ts_user/llm_pro/NapCatQQ`.

---

## File Map

- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-develop/config/onebot11.json`
  Responsibility: NapCat-side default WS config that belongs in the NapCat fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-onebot/config/config.ts`
  Responsibility: NapCat-side WebSocket server defaults that belong in the NapCat fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-onebot/index.ts`
  Responsibility: NapCat-side startup/logging behavior that belongs in the NapCat fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-onebot/network/websocket-server.ts`
  Responsibility: NapCat-side WebSocket lifecycle fix that belongs in the NapCat fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-test/schema.test.ts`
  Responsibility: NapCat-side regression coverage that belongs in the NapCat fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-test/vitest.config.ts`
  Responsibility: NapCat-side test harness config for the fork branch.
- Create: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-test/websocketServerAdapter.test.ts`
  Responsibility: NapCat-side regression test for WebSocket adapter startup semantics.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-webui-backend/src/onebot/config.ts`
  Responsibility: NapCat-side WebUI config wiring that belongs in the fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-webui-frontend/src/components/network_edit/ws_server.tsx`
  Responsibility: NapCat-side frontend default port display that belongs in the fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-webui-frontend/src/pages/dashboard/debug/websocket/index.tsx`
  Responsibility: NapCat-side frontend WebSocket UI text that belongs in the fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/scripts/run_napcat_linux.py`
  Responsibility: NapCat-side Linux launcher helper that belongs in the fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/tests/scripts/test_run_napcat_linux.py`
  Responsibility: NapCat-side launcher regression coverage that belongs in the fork branch.
- Modify: `/home/ts_user/llm_pro/NapCatQQ/.git/config`
  Responsibility: local Git remote metadata for the NapCat fork remote and temporary worktree branch.
- Create: `/home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork/`
  Responsibility: clean worktree used to publish a dependency-only NapCat branch.
- Create: `/home/ts_user/llm_pro/codex-bridge/`
  Responsibility: standalone product repository that becomes the new main home of the bridge.
- Modify: `/home/ts_user/llm_pro/codex-bridge/Cargo.toml`
  Responsibility: new root workspace manifest and dependency path wiring.
- Modify: `/home/ts_user/llm_pro/codex-bridge/Cargo.lock`
  Responsibility: lockfile refreshed after rename and submodule path rewiring.
- Modify: `/home/ts_user/llm_pro/codex-bridge/README.md`
  Responsibility: product-facing rename, `deps/` layout, and new usage docs.
- Modify: `/home/ts_user/llm_pro/codex-bridge/Makefile`
  Responsibility: top-level developer commands updated to `codex-bridge`.
- Modify: `/home/ts_user/llm_pro/codex-bridge/rust-toolchain.toml`
  Responsibility: preserved at the new repository root without functional changes.
- Modify: `/home/ts_user/llm_pro/codex-bridge/rustfmt.toml`
  Responsibility: preserved at the new repository root without functional changes.
- Create: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/`
  Responsibility: renamed first-party runtime/orchestrator crate.
- Create: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/`
  Responsibility: renamed first-party CLI crate.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/Cargo.toml`
  Responsibility: renamed core package manifest.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/Cargo.toml`
  Responsibility: renamed CLI package manifest and core dependency path.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/cli.rs`
  Responsibility: CLI binary identity and help text renamed to `codex-bridge`.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/main.rs`
  Responsibility: new project-root traversal and default Codex submodule path.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/runtime.rs`
  Responsibility: runtime paths updated for `deps/NapCatQQ` ownership.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/launcher.rs`
  Responsibility: NapCat build/start commands run from the submodule root.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/codex_runtime.rs`
  Responsibility: Codex client name updated to `codex-bridge`.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/tests/config_tests.rs`
  Responsibility: runtime-path coverage for submodule layout.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/tests/launcher_tests.rs`
  Responsibility: runtime state tests updated for the renamed root and path model.
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/tests/cli_tests.rs`
  Responsibility: CLI rename coverage.
- Create: `/home/ts_user/llm_pro/codex-bridge/.gitmodules`
  Responsibility: records `deps/NapCatQQ` and `deps/codex` submodules.
- Create: `/home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ`
  Responsibility: NapCat source dependency pinned to the fork branch commit.
- Create: `/home/ts_user/llm_pro/codex-bridge/deps/codex`
  Responsibility: Codex source dependency pinned to the fork commit.
- Create: `/home/ts_user/llm_pro/codex-bridge/docs/superpowers/specs/2026-04-05-codex-bridge-repository-restructure-design.md`
  Responsibility: imported repository-restructure spec in the new repository.
- Create: `/home/ts_user/llm_pro/codex-bridge/docs/superpowers/plans/2026-04-05-codex-bridge-repository-restructure.md`
  Responsibility: imported implementation plan in the new repository.

### Task 1: Publish NapCat dependency changes to the fork branch

**Files:**
- Modify: `/home/ts_user/llm_pro/NapCatQQ/.git/config`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-develop/config/onebot11.json`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-onebot/config/config.ts`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-onebot/index.ts`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-onebot/network/websocket-server.ts`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-test/schema.test.ts`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-test/vitest.config.ts`
- Create: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-test/websocketServerAdapter.test.ts`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-webui-backend/src/onebot/config.ts`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-webui-frontend/src/components/network_edit/ws_server.tsx`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/packages/napcat-webui-frontend/src/pages/dashboard/debug/websocket/index.tsx`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/scripts/run_napcat_linux.py`
- Modify: `/home/ts_user/llm_pro/NapCatQQ/tests/scripts/test_run_napcat_linux.py`

- [ ] **Step 1: Verify the NapCat fork remote and publish branch do not already exist**

Run:

```bash
git -C /home/ts_user/llm_pro/NapCatQQ remote get-url acking-napcat
git -C /home/ts_user/llm_pro/NapCatQQ show-ref --verify refs/heads/acking/qqbot-bootstrap
```

Expected: both commands fail because the remote and local publish branch do not exist yet.

- [ ] **Step 2: Add the fork remote and build a dependency-only patch from the current working tree**

Run:

```bash
git -C /home/ts_user/llm_pro/NapCatQQ remote add acking-napcat git@github.com:acking-you/NapCatQQ.git
git -C /home/ts_user/llm_pro/NapCatQQ fetch acking-napcat
git -C /home/ts_user/llm_pro/NapCatQQ diff -- \
  packages/napcat-develop/config/onebot11.json \
  packages/napcat-onebot/config/config.ts \
  packages/napcat-onebot/index.ts \
  packages/napcat-onebot/network/websocket-server.ts \
  packages/napcat-test/schema.test.ts \
  packages/napcat-test/vitest.config.ts \
  packages/napcat-webui-backend/src/onebot/config.ts \
  packages/napcat-webui-frontend/src/components/network_edit/ws_server.tsx \
  packages/napcat-webui-frontend/src/pages/dashboard/debug/websocket/index.tsx \
  scripts/run_napcat_linux.py \
  tests/scripts/test_run_napcat_linux.py \
  packages/napcat-test/websocketServerAdapter.test.ts \
  > /tmp/napcat-bootstrap.patch
git -C /home/ts_user/llm_pro/NapCatQQ diff -- packages/napcat-onebot/action/stream/test_upload_stream.py
```

Expected: the patch file is created from the approved dependency file set. The last diff is reviewed explicitly; if it is unrelated to the WebSocket/launcher work, leave it out of the fork branch and do not stage it later.

- [ ] **Step 3: Create a clean worktree from upstream NapCat and apply only the dependency patch**

Run:

```bash
mkdir -p /home/ts_user/llm_pro/NapCatQQ/.worktrees
git -C /home/ts_user/llm_pro/NapCatQQ worktree add \
  /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork \
  -b acking/qqbot-bootstrap \
  origin/main
git -C /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork apply --index /tmp/napcat-bootstrap.patch
```

Expected: the worktree is based on `origin/main` and contains only the intended NapCat changes, not the `my_qq_bot` changes from the source repository.

- [ ] **Step 4: Run NapCat verification in the clean worktree**

Run:

```bash
cd /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork && pnpm --filter napcat-onebot run typecheck
cd /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork && pnpm --filter napcat-test exec vitest run websocketServerAdapter.test.ts schema.test.ts
cd /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork && python3 -m unittest discover -s tests/scripts -p 'test_run_napcat_linux.py' -v
```

Expected: all three commands pass in the clean NapCat worktree.

- [ ] **Step 5: Commit and push the NapCat fork branch**

Run:

```bash
git -C /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork commit -m "fix: clarify websocket server startup and defaults"
git -C /home/ts_user/llm_pro/NapCatQQ/.worktrees/napcat-fork push -u acking-napcat acking/qqbot-bootstrap
git ls-remote --heads git@github.com:acking-you/NapCatQQ.git acking/qqbot-bootstrap
```

Expected: the commit is pushed and `ls-remote` prints the branch hash for `acking/qqbot-bootstrap`.

### Task 2: Seed a standalone `codex-bridge` repository from `my_qq_bot` history

**Files:**
- Modify: `/home/ts_user/llm_pro/NapCatQQ/.git/refs/heads/codex-bridge-history`
- Create: `/home/ts_user/llm_pro/codex-bridge/`
- Create: `/home/ts_user/llm_pro/codex-bridge/docs/superpowers/specs/2026-04-05-codex-bridge-repository-restructure-design.md`
- Create: `/home/ts_user/llm_pro/codex-bridge/docs/superpowers/plans/2026-04-05-codex-bridge-repository-restructure.md`

- [ ] **Step 1: Verify the split-history branch and target repository clone do not already exist**

Run:

```bash
git -C /home/ts_user/llm_pro/NapCatQQ show-ref --verify refs/heads/codex-bridge-history
test -d /home/ts_user/llm_pro/codex-bridge/.git
```

Expected: both commands fail because the subtree branch and new clone do not exist yet.

- [ ] **Step 2: Split the `my_qq_bot` history into its own branch and clone the empty target repository**

Run:

```bash
git -C /home/ts_user/llm_pro/NapCatQQ subtree split --prefix=my_qq_bot -b codex-bridge-history
git clone git@github.com:acking-you/codex-bridge.git /home/ts_user/llm_pro/codex-bridge
git -C /home/ts_user/llm_pro/codex-bridge remote add source /home/ts_user/llm_pro/NapCatQQ
git -C /home/ts_user/llm_pro/codex-bridge fetch source codex-bridge-history
git -C /home/ts_user/llm_pro/codex-bridge switch -C main FETCH_HEAD
```

Expected: the new repository root now contains the former `my_qq_bot` tree as its top-level working tree, with the original path history preserved.

- [ ] **Step 3: Import the approved restructure docs into the new repository**

Run:

```bash
mkdir -p /home/ts_user/llm_pro/codex-bridge/docs/superpowers/specs
mkdir -p /home/ts_user/llm_pro/codex-bridge/docs/superpowers/plans
cp /home/ts_user/llm_pro/NapCatQQ/docs/superpowers/specs/2026-04-05-codex-bridge-repository-restructure-design.md \
  /home/ts_user/llm_pro/codex-bridge/docs/superpowers/specs/2026-04-05-codex-bridge-repository-restructure-design.md
cp /home/ts_user/llm_pro/NapCatQQ/docs/superpowers/plans/2026-04-05-codex-bridge-repository-restructure.md \
  /home/ts_user/llm_pro/codex-bridge/docs/superpowers/plans/2026-04-05-codex-bridge-repository-restructure.md
```

Expected: the new repository contains the current approved spec and plan under its own `docs/` tree.

- [ ] **Step 4: Verify the imported repository root is the old `my_qq_bot` tree**

Run:

```bash
ls -1 /home/ts_user/llm_pro/codex-bridge
```

Expected: the output includes `Cargo.toml`, `Cargo.lock`, `Makefile`, `README.md`, `qqbot-core`, `qqbot-cli`, `rust-toolchain.toml`, and `rustfmt.toml`.

- [ ] **Step 5: Commit the imported docs and push the seeded main branch**

Run:

```bash
git -C /home/ts_user/llm_pro/codex-bridge add docs/superpowers/specs/2026-04-05-codex-bridge-repository-restructure-design.md \
  docs/superpowers/plans/2026-04-05-codex-bridge-repository-restructure.md
git -C /home/ts_user/llm_pro/codex-bridge commit -m "docs: import codex-bridge migration docs"
git -C /home/ts_user/llm_pro/codex-bridge push -u origin main
```

Expected: the new repository has a pushed `main` branch with preserved bridge history plus the imported migration docs.

### Task 3: Reshape the workspace into `crates/` and rename the first-party packages

**Files:**
- Modify: `/home/ts_user/llm_pro/codex-bridge/Cargo.toml`
- Modify: `/home/ts_user/llm_pro/codex-bridge/README.md`
- Modify: `/home/ts_user/llm_pro/codex-bridge/Makefile`
- Create: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/`
- Create: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/Cargo.toml`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/Cargo.toml`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/cli.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/main.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/tests/cli_tests.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/*.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/tests/*.rs`

- [ ] **Step 1: Verify the target `crates/` layout does not exist yet**

Run:

```bash
test -d /home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core
test -d /home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli
```

Expected: both commands fail because the repository still has `qqbot-core` and `qqbot-cli` at the root.

- [ ] **Step 2: Move the crates and replace the root workspace manifest**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
mkdir -p crates
mv qqbot-core crates/codex-bridge-core
mv qqbot-cli crates/codex-bridge-cli
cat > Cargo.toml <<'EOF'
[workspace]
resolver = "2"
members = ["crates/codex-bridge-core", "crates/codex-bridge-cli"]

[workspace.dependencies]
anyhow = "1.0"
axum = { version = "0.8", features = ["ws"] }
codex-app-server-protocol = { path = "/home/ts_user/rust_pro/codex/codex-rs/app-server-protocol" }
codex-utils-absolute-path = { path = "/home/ts_user/rust_pro/codex/codex-rs/utils/absolute-path" }
clap = { version = "4.5", features = ["derive"] }
async-trait = "0.1"
futures-util = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.10"
thiserror = "2.0"
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.28"
rusqlite = { version = "0.33", features = ["bundled"] }
tower = { version = "0.5", features = ["util"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
url = "2.5"
uuid = { version = "1", features = ["v4"] }

[workspace.lints.rust]
missing_docs = "deny"

[workspace.lints.clippy]
allow_attributes_without_reason = "deny"
dbg_macro = "deny"
empty_docs = "deny"
tabs_in_doc_comments = "deny"
todo = "deny"
undocumented_unsafe_blocks = "deny"
unimplemented = "deny"

[workspace.lints.rustdoc]
bare_urls = "deny"
broken_intra_doc_links = "deny"
invalid_codeblock_attributes = "deny"
invalid_html_tags = "deny"
invalid_rust_codeblocks = "deny"
missing_crate_level_docs = "deny"
private_intra_doc_links = "deny"
unescaped_backticks = "deny"
EOF
cat > crates/codex-bridge-core/Cargo.toml <<'EOF'
[package]
name = "codex-bridge-core"
version = "0.1.0"
edition = "2021"
publish = false

[lints]
workspace = true

[dependencies]
anyhow = { workspace = true }
async-trait = { workspace = true }
codex-app-server-protocol = { workspace = true }
codex-utils-absolute-path = { workspace = true }
axum = { workspace = true }
futures-util = { workspace = true }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
rusqlite = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tokio-tungstenite = { workspace = true }
tower = { workspace = true }
tracing = { workspace = true }
url = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
tempfile = "3.23"
EOF
cat > crates/codex-bridge-cli/Cargo.toml <<'EOF'
[package]
name = "codex-bridge-cli"
version = "0.1.0"
edition = "2021"
publish = false

[lints]
workspace = true

[dependencies]
anyhow = { workspace = true }
clap = { workspace = true }
reqwest = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
codex-bridge-core = { path = "../codex-bridge-core" }
EOF
```

Expected: the repository now has a `crates/` workspace, but source identifiers and docs still use the old `qqbot-*` names.

- [ ] **Step 3: Replace first-party package, module, and product identifiers**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
rg -l 'qqbot_core' crates | xargs perl -0pi -e 's/qqbot_core/codex_bridge_core/g'
rg -l 'qqbot_cli' crates | xargs perl -0pi -e 's/qqbot_cli/codex_bridge_cli/g'
rg -l 'qqbot-core' crates README.md Makefile | xargs perl -0pi -e 's/qqbot-core/codex-bridge-core/g'
rg -l 'qqbot-cli' crates/codex-bridge-cli/src crates/codex-bridge-cli/tests README.md Makefile | xargs perl -0pi -e 's/qqbot-cli/codex-bridge/g'
rg -l 'my_qq_bot|My QQ Bot' crates README.md | xargs perl -0pi -e 's/my_qq_bot/codex-bridge/g; s/My QQ Bot/Codex Bridge/g'
```

Expected: source imports now use `codex_bridge_core` and docs/binary-facing text now say `codex-bridge`.

- [ ] **Step 4: Run narrow workspace checks after the rename**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge && cargo test -p codex-bridge-cli --test cli_tests -- --nocapture
cd /home/ts_user/llm_pro/codex-bridge && cargo test -p codex-bridge-core --test config_tests -- --nocapture
```

Expected: both command groups pass under the renamed workspace.

- [ ] **Step 5: Commit the workspace reshape**

Run:

```bash
git -C /home/ts_user/llm_pro/codex-bridge add Cargo.toml README.md Makefile crates
git -C /home/ts_user/llm_pro/codex-bridge commit -m "refactor: rename workspace to codex bridge"
```

Expected: the new repository now uses `codex-bridge` naming and `crates/` layout.

### Task 4: Add `deps/` submodules and switch Codex dependencies to relative paths

**Files:**
- Create: `/home/ts_user/llm_pro/codex-bridge/.gitmodules`
- Create: `/home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ`
- Create: `/home/ts_user/llm_pro/codex-bridge/deps/codex`
- Modify: `/home/ts_user/llm_pro/codex-bridge/Cargo.toml`

- [ ] **Step 1: Verify the new repository does not already have submodules**

Run:

```bash
test -f /home/ts_user/llm_pro/codex-bridge/.gitmodules
test -d /home/ts_user/llm_pro/codex-bridge/deps/NapCatQQ
test -d /home/ts_user/llm_pro/codex-bridge/deps/codex
```

Expected: all three commands fail because the repository has no `deps/` submodules yet.

- [ ] **Step 2: Add the NapCat and Codex submodules**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge
mkdir -p deps
git submodule add git@github.com:acking-you/NapCatQQ.git deps/NapCatQQ
git -C deps/NapCatQQ fetch origin acking/qqbot-bootstrap
git -C deps/NapCatQQ checkout FETCH_HEAD
git submodule add git@github.com:acking-you/codex.git deps/codex
```

Expected: `.gitmodules` is created and both submodules exist in detached-head or pinned-checkout form.

- [ ] **Step 3: Replace absolute Codex path dependencies with submodule-relative paths**

Update `/home/ts_user/llm_pro/codex-bridge/Cargo.toml` so these lines become:

```toml
codex-app-server-protocol = { path = "deps/codex/codex-rs/app-server-protocol" }
codex-utils-absolute-path = { path = "deps/codex/codex-rs/utils/absolute-path" }
```

- [ ] **Step 4: Run metadata checks against the new dependency layout**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge && git submodule status
cd /home/ts_user/llm_pro/codex-bridge && cargo metadata --format-version 1 > /tmp/codex-bridge-metadata.json
```

Expected: `git submodule status` shows both `deps/NapCatQQ` and `deps/codex`, and `cargo metadata` succeeds using the relative `deps/codex` path dependencies.

- [ ] **Step 5: Commit the submodule layout**

Run:

```bash
git -C /home/ts_user/llm_pro/codex-bridge add .gitmodules Cargo.toml deps/NapCatQQ deps/codex
git -C /home/ts_user/llm_pro/codex-bridge commit -m "build: add napcat and codex source dependencies"
```

Expected: the repository now encodes both dependency source trees through submodule pointers instead of inline source or absolute paths.

### Task 5: Rewire runtime, launcher, and default paths for the new repository layout

**Files:**
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/runtime.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/launcher.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/codex_runtime.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/main.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/tests/config_tests.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/tests/launcher_tests.rs`

- [ ] **Step 1: Write the failing tests for submodule-owned runtime paths**

Append to `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/tests/config_tests.rs`:

```rust
#[test]
fn runtime_paths_point_to_dependency_submodules() {
    let project_root = PathBuf::from("/tmp/codex-bridge");
    let paths = RuntimePaths::new(&project_root, None);
    assert_eq!(paths.project_root, project_root);
    assert_eq!(paths.napcat_repo_root, PathBuf::from("/tmp/codex-bridge/deps/NapCatQQ"));
    assert_eq!(
        paths.built_shell_dir,
        PathBuf::from("/tmp/codex-bridge/deps/NapCatQQ/packages/napcat-shell/dist")
    );
}
```

- [ ] **Step 2: Run the new path test and a grep check to verify the old hard-coded layout is still present**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge && cargo test -p codex-bridge-core --test config_tests -- --nocapture
cd /home/ts_user/llm_pro/codex-bridge && rg -n "/home/ts_user/rust_pro/codex|my_qq_bot|packages/napcat-shell/dist" crates/codex-bridge-core/src crates/codex-bridge-cli/src
```

Expected: the test fails because `RuntimePaths` does not yet expose `napcat_repo_root`, and the grep command finds the old hard-coded Codex path and legacy layout assumptions.

- [ ] **Step 3: Update runtime path derivation and launcher defaults**

Apply these code changes.

In `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/runtime.rs`, make `RuntimePaths` include the NapCat submodule root:

```rust
pub struct RuntimePaths {
    pub project_root: PathBuf,
    pub napcat_repo_root: PathBuf,
    pub runtime_root: PathBuf,
    pub config_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub run_dir: PathBuf,
    pub database_path: PathBuf,
    pub launcher_env: PathBuf,
    pub qq_base: PathBuf,
    pub qq_executable: PathBuf,
    pub resources_app_dir: PathBuf,
    pub app_launcher_dir: PathBuf,
    pub qq_package_json: PathBuf,
    pub qq_load_script: PathBuf,
    pub built_shell_dir: PathBuf,
    pub pid_file: PathBuf,
}

impl RuntimePaths {
    pub fn new(project_root: &Path, qq_executable: Option<PathBuf>) -> Self {
        let runtime_root = project_root.join(".run/default");
        let run_dir = runtime_root.join("run");
        let napcat_repo_root = project_root.join("deps/NapCatQQ");
        // keep the existing QQ path derivation
        Self {
            project_root: project_root.to_path_buf(),
            napcat_repo_root: napcat_repo_root.clone(),
            runtime_root: runtime_root.clone(),
            config_dir: runtime_root.join("config"),
            logs_dir: runtime_root.join("logs"),
            cache_dir: runtime_root.join("cache"),
            run_dir,
            database_path: runtime_root.join("state.sqlite3"),
            launcher_env: runtime_root.join("run/launcher.env"),
            built_shell_dir: napcat_repo_root.join("packages/napcat-shell/dist"),
            pid_file: runtime_root.join("run/qq.pid"),
            // keep the remaining existing fields unchanged
        }
    }
}
```

In `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/launcher.rs`, build NapCat from the submodule root:

```rust
async fn ensure_workspace_built(paths: &RuntimePaths) -> Result<()> {
    if !paths.napcat_repo_root.join("node_modules").exists() {
        run_checked(["pnpm", "install"], &paths.napcat_repo_root).await?;
    }
    run_checked(["pnpm", "build:webui"], &paths.napcat_repo_root).await?;
    run_checked(["pnpm", "build:plugin-builtin"], &paths.napcat_repo_root).await?;
    run_checked(["pnpm", "build:shell"], &paths.napcat_repo_root).await?;
    Ok(())
}
```

In `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-core/src/codex_runtime.rs`, rename the client identity:

```rust
const DEFAULT_CLIENT_NAME: &str = "codex-bridge";
```

In `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/main.rs`, make the repository root and default Codex path come from the new layout:

```rust
const DEFAULT_LOG_FILTER: &str = "warn,codex_bridge_cli=info,codex_bridge_core=info";

fn project_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(project_root) = manifest_dir.parent().and_then(|path| path.parent()) else {
        anyhow::bail!("failed to derive project root from {}", manifest_dir.display());
    };
    Ok(project_root.to_path_buf())
}

fn codex_repo_root(project_root: &Path) -> PathBuf {
    env::var_os("CODEX_BRIDGE_CODEX_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join("deps/codex/codex-rs"))
}
```

Also update the `CodexRuntimeConfig::new(codex_repo_root(&project_root), project_root.clone())`
call site so the runtime uses the submodule-backed default Codex path.

- [ ] **Step 4: Re-run the path tests and grep checks**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge && cargo test -p codex-bridge-core --test config_tests --test launcher_tests -- --nocapture
cd /home/ts_user/llm_pro/codex-bridge && rg -n "/home/ts_user/rust_pro/codex|my_qq_bot" crates/codex-bridge-core/src crates/codex-bridge-cli/src
```

Expected: the tests pass, and the grep command returns no matches.

- [ ] **Step 5: Commit the path rewiring**

Run:

```bash
git -C /home/ts_user/llm_pro/codex-bridge add \
  crates/codex-bridge-core/src/runtime.rs \
  crates/codex-bridge-core/src/launcher.rs \
  crates/codex-bridge-core/src/codex_runtime.rs \
  crates/codex-bridge-cli/src/main.rs \
  crates/codex-bridge-core/tests/config_tests.rs \
  crates/codex-bridge-core/tests/launcher_tests.rs
git -C /home/ts_user/llm_pro/codex-bridge commit -m "refactor: rewire runtime paths to dependency submodules"
```

Expected: the new repository no longer depends on the old absolute Codex path or the old `my_qq_bot` directory shape.

### Task 6: Update top-level docs and run full repository verification

**Files:**
- Modify: `/home/ts_user/llm_pro/codex-bridge/README.md`
- Modify: `/home/ts_user/llm_pro/codex-bridge/Makefile`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/cli.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/tests/cli_tests.rs`
- Modify: `/home/ts_user/llm_pro/codex-bridge/docs/superpowers/specs/2026-04-05-codex-bridge-repository-restructure-design.md`
- Modify: `/home/ts_user/llm_pro/codex-bridge/docs/superpowers/plans/2026-04-05-codex-bridge-repository-restructure.md`

- [ ] **Step 1: Run a failing grep check for stale product names and old repository references**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge && rg -n "my_qq_bot|qqbot-core|qqbot-cli|NapCatQQ/README|/home/ts_user/llm_pro/NapCatQQ" README.md Makefile crates docs
```

Expected: the grep returns matches that still refer to the old product name or source repository path.

- [ ] **Step 2: Update the top-level product docs and command names**

Apply these edits.

In `/home/ts_user/llm_pro/codex-bridge/crates/codex-bridge-cli/src/cli.rs`:

```rust
/// Top-level CLI definition for the `codex-bridge` binary.
#[derive(Debug, Parser)]
#[command(name = "codex-bridge", version, about = "Codex-powered bridge runtime")]
pub struct Cli {
```

In `/home/ts_user/llm_pro/codex-bridge/Makefile`:

```make
.PHONY: fmt lint test run

fmt:
	@cargo fmt --all

lint:
	@cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	@cargo test --workspace

run:
	@cargo run -p codex-bridge-cli -- run
```

In `/home/ts_user/llm_pro/codex-bridge/README.md`, update:

- the title to `Codex Bridge`,
- every `my_qq_bot` path example to `codex-bridge`,
- every `cargo run -p qqbot-cli` example to `cargo run -p codex-bridge-cli`,
- the dependency layout section to describe `deps/NapCatQQ` and `deps/codex`.

- [ ] **Step 3: Run the full repository verification suite**

Run:

```bash
cd /home/ts_user/llm_pro/codex-bridge && cargo fmt --all --check
cd /home/ts_user/llm_pro/codex-bridge && cargo test -p codex-bridge-core -- --nocapture
cd /home/ts_user/llm_pro/codex-bridge && cargo test -p codex-bridge-cli -- --nocapture
cd /home/ts_user/llm_pro/codex-bridge && cargo clippy --workspace --all-targets --all-features -- -D warnings
cd /home/ts_user/llm_pro/codex-bridge && git submodule status
```

Expected: all Rust checks pass and both submodules are pinned in `git submodule status`.

- [ ] **Step 4: Commit the finalized `codex-bridge` repository and push**

Run:

```bash
git -C /home/ts_user/llm_pro/codex-bridge add README.md Makefile crates docs Cargo.toml Cargo.lock .gitmodules deps
git -C /home/ts_user/llm_pro/codex-bridge commit -m "feat: establish codex bridge repository layout"
git -C /home/ts_user/llm_pro/codex-bridge push
```

Expected: the new repository has a pushed commit containing the final `codex-bridge` structure, naming, submodules, and verified runtime path rewiring.
