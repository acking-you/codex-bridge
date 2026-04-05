# Codex Bridge Repository Restructure Design

## Goal

Split the current mixed-purpose repository into a clean top-level project named
`codex-bridge`, where:

- `codex-bridge` is the primary repository and product identity,
- NapCat source lives in `deps/NapCatQQ` as a Git submodule,
- Codex source lives in `deps/codex` as a Git submodule,
- the Rust bridge/orchestrator remains the main codebase,
- existing `my_qq_bot` history is preserved instead of being replaced by a
  one-time code snapshot.

The result must make repository ownership obvious: `codex-bridge` owns bot
orchestration, while NapCat and Codex remain upstream-dependent source trees.

## Non-Goals

- Keep NapCat as the top-level repository identity.
- Reimplement NapCat or Codex inside the main repository.
- Preserve the current mixed top-level layout.
- Rename or reshape NapCat/Codex submodules away from their upstream names.
- Redesign the local HTTP API in the same migration.
- Introduce a polyglot monorepo toolchain for code that does not belong to the
  bridge itself.

## Current Problem

The current repository mixes three responsibilities:

1. NapCat source and local NapCat modifications.
2. The Rust bot/orchestrator currently living under `my_qq_bot/`.
3. A future Codex source dependency that should not live inline in the main
   repository.

That structure creates the wrong ownership model:

- upstream dependency updates become harder to reason about,
- it is unclear which changes belong to the product versus the dependency,
- the directory layout still implies a QQ-specific experiment rather than a
  Codex-driven bridge runtime.

## Design Principles

### One Repository, One Product

`codex-bridge` is the product. NapCat and Codex are dependencies.

### Preserve Real Ownership Boundaries

- Behavior changes to NapCat belong in the NapCat fork.
- Behavior changes to Codex app-server belong in the Codex fork.
- Integration, orchestration, queueing, policy, persistence, and local API
  belong in `codex-bridge`.

### Preserve History Where It Matters

The Rust bridge history should move forward with the new repository instead of
being flattened into a one-time import.

### KISS/YAGNI

The new repository should stay small and obvious:

- one Rust workspace,
- one main CLI,
- two source submodules under `deps/`,
- no extra layering unless required by real ownership boundaries.

## Repository Model

The new top-level repository is a standalone GitHub repository named
`codex-bridge`.

### Top-Level Layout

```text
codex-bridge/
  Cargo.toml
  Cargo.lock
  README.md
  crates/
    codex-bridge-core/
    codex-bridge-cli/
  deps/
    NapCatQQ/
    codex/
  scripts/
  docs/
```

### Ownership By Directory

- `crates/`: all first-party Rust code.
- `deps/NapCatQQ`: NapCat source fork as a Git submodule.
- `deps/codex`: Codex source fork as a Git submodule.
- `scripts/`: thin first-party automation for build, launch, and local setup.
- `docs/`: first-party architecture, specs, plans, and usage docs.

## Naming

### Product Identity

- Repository name: `codex-bridge`
- Project title: `Codex Bridge`
- Primary description: `A transport-agnostic bot runtime built around Codex app-server`

### Rust Workspace Naming

- Core crate: `codex-bridge-core`
- CLI crate: `codex-bridge-cli`
- Binary: `codex-bridge`

### Runtime Naming

- Runtime directory: `.run/default/`
- Environment variable prefix: `CODEX_BRIDGE_`

### Dependency Directory Naming

Keep dependency directories aligned with upstream names:

- `deps/NapCatQQ`
- `deps/codex`

This avoids unnecessary translation layers in scripts, docs, and upstream diff
tracking.

## Code Ownership Boundaries

### Code That Must Live in `acking-you/NapCatQQ`

Any change that modifies NapCat behavior itself belongs in the NapCat fork,
including:

- OneBot WebSocket server lifecycle fixes,
- NapCat configuration schema and default transport changes,
- NapCat logging changes,
- NapCat WebUI and frontend changes,
- NapCat test additions,
- NapCat-specific Linux launcher behavior under the current top-level
  `scripts/run_napcat_linux.py` flow.

In the current working tree, this means the `packages/napcat-*`, `scripts/`,
and `tests/scripts/` NapCat-oriented changes are dependency changes, not
`codex-bridge` product changes.

### Code That Must Live in `codex-bridge`

The following are first-party bridge responsibilities:

- NapCat child-process orchestration,
- formal OneBot WebSocket client transport,
- message routing,
- task scheduling and queueing,
- SQLite state persistence,
- Codex app-server stdio runtime integration,
- approval/safety policy,
- local API and CLI,
- user-facing bridge docs and operational workflow.

Today, these responsibilities originate from the code under `my_qq_bot/`.

### Code That Must Live in `acking-you/codex`

Any future change that modifies Codex app-server behavior itself belongs in the
Codex fork, including:

- JSON-RPC protocol behavior fixes,
- turn lifecycle behavior changes,
- approval protocol changes,
- app-server runtime behavior changes.

`codex-bridge` may consume those capabilities, but it must not inline-source a
private patched copy of Codex behavior.

## Git Strategy

### NapCat First

Before restructuring the new repository, the current NapCat modifications must
be isolated and pushed to:

- remote: `git@github.com:acking-you/NapCatQQ.git`
- branch: `acking/qqbot-bootstrap`

This branch becomes the dependency anchor for the future
`deps/NapCatQQ` submodule.

### Separate Product Repository

`codex-bridge` should be created as a new standalone repository rather than
continuing to evolve inside the top-level NapCat repository identity.

### Preserve Bridge History

The Rust bridge history should be migrated into the new repository, not copied
as a flat code snapshot. The new repository should reflect the real evolution
of the bridge/orchestrator rather than hiding it behind a single import commit.

## Migration Sequence

The migration should happen in this order.

### 1. Push NapCat Dependency Changes

- Add the `acking-you/NapCatQQ` remote.
- Separate NapCat-specific modifications from bridge-specific ones.
- Commit the dependency changes to the NapCat fork branch
  `acking/qqbot-bootstrap`.

### 2. Create the `codex-bridge` Repository

- Create a new empty GitHub repository named `codex-bridge`.
- Make that repository the new first-party home for the bridge runtime.

### 3. Move the Bridge Code With History

Reshape the existing Rust code into the new layout:

- `my_qq_bot/qqbot-core` -> `crates/codex-bridge-core`
- `my_qq_bot/qqbot-cli` -> `crates/codex-bridge-cli`
- `my_qq_bot/README.md` -> top-level `README.md`

At the same time:

- rename packages and binaries from `qqbot-*` to `codex-bridge-*`,
- keep existing bridge behavior unless a change is required by the new layout,
- retain the current API surface unless a concrete break is necessary.

### 4. Add Dependency Submodules

Add:

- `deps/NapCatQQ -> git@github.com:acking-you/NapCatQQ.git`
- `deps/codex -> git@github.com:acking-you/codex.git`

Pin:

- NapCat to the pushed `acking/qqbot-bootstrap` commit,
- Codex to a stable commit from the fork.

### 5. Rewire Paths and Launch Logic

Only after submodules exist should path assumptions be rewritten:

- NapCat source paths resolve through `deps/NapCatQQ`,
- Codex source paths resolve through `deps/codex`,
- build and launch scripts default to these dependency locations,
- `.run/default/` remains first-party runtime state owned by `codex-bridge`.

### 6. Remove Legacy Layout

After the new layout works:

- remove the `my_qq_bot/` directory shell,
- remove inline NapCat source from the main product repository,
- update docs, commands, and onboarding instructions to the new paths.

## Behavior Compatibility Rules

The restructure is about ownership and layout. It should not silently redesign
the product surface.

### Preserve in Phase 1

- Existing local API routes such as `/api/session`, `/api/status`,
  `/api/queue`, `/api/messages/private`, `/api/messages/group`,
  `/api/tasks/cancel`.
- Existing single-task queue semantics.
- Existing Codex app-server integration model.
- Existing OneBot WebSocket transport direction.

### Allowed Changes

- Crate/package names,
- internal file layout,
- dependency paths,
- README and operational instructions,
- launcher path configuration needed to consume submodules.

## Risks

### Mixed-Ownership Commits

If NapCat changes are not pushed first, the new repository will inherit
dependency code that does not belong to it.

### History Loss

If the Rust bridge is copied as a plain directory snapshot, useful design and
debugging history will be lost.

### Path Drift

If scripts are updated before submodule paths are real, the migration will
likely create duplicate path rewrites and brittle launcher logic.

### Naming Drift

If `qqbot` names remain in crates, binaries, or docs after the repository
rename, the new product identity will remain muddled.

## Success Criteria

The restructure is complete when all of the following are true:

- a standalone repository named `codex-bridge` exists,
- bridge/orchestrator code lives under `crates/`,
- NapCat and Codex are consumed through `deps/` Git submodules,
- NapCat modifications are anchored in `acking-you/NapCatQQ` on
  `acking/qqbot-bootstrap`,
- the bridge still starts NapCat and Codex from the new layout,
- the first-party repository no longer directly carries dependency source trees,
- the naming visible to users and developers consistently says `codex-bridge`.
