# Package-manager audit (Phase 0)

This document began as the Phase 0 source audit. It is updated as approved phases are implemented; where the extension spec and the source differ, the current source behavior is recorded here.

## Source map

| Area | Source |
|---|---|
| Rhai `use_package` registration and report | `crates/core/src/rhai/setup.rs`; closure state and `plugin_uses()` / `use_package_report()` in `crates/core/src/rhai/call.rs` |
| Parsed declaration model | `crates/core/src/declare.rs` |
| Resolve declarations, install missing plugin artifacts, and update lock entries | `crates/editor/src/declare.rs` |
| `forge-lock.toml` format, load/save, hash check | `crates/core/src/lockfile.rs` |
| Durable package-update journal and startup recovery | `crates/core/src/package_journal.rs`; transaction wiring in `crates/editor/src/packages.rs` |
| Package index format, `packages.toml`, artifact install, build-plan detection | `crates/core/src/package.rs` |
| Index package commands/browser | `crates/editor/src/packages.rs`; command documentation/registration in `crates/editor/src/commands.rs` |
| Git clone/fetch/build/install state machine and `*Package Build*` output | `crates/editor/src/package_vc.rs` |
| Permission manifest, grants model and grant persistence | `crates/core/src/permissions.rs`; state-dir loading and config-dir migration in `crates/editor/src/lib.rs` and `crates/editor/src/package_doctor.rs` |
| Package doctor checks, findings, and headless/interactive rendering | `crates/core/src/package_doctor.rs`; `crates/editor/src/package_doctor.rs` |
| Plugin bisect search, journal, interactive and headless modes | `crates/core/src/plugin_bisect.rs`; `crates/editor/src/plugin_bisect.rs`; `crates/terminal/src/headless.rs` |
| Plugin component inspection, runtime, and lazy load | `crates/core/src/plugin/host.rs`, `crates/core/src/plugin/runtime.rs` |
| Lazy-start summary serialization/key | `crates/core/src/plugin/summary.rs` |
| Wasmtime compilation cache | `crates/core/src/plugin/host.rs` (cache path is `<cache_dir>/wasmtime`); cache directory selected in `crates/editor/src/lib.rs` |
| Child process environment, output and timeout support | `crates/core/src/process.rs` |
| Existing command-line entry points | `crates/editor/src/main.rs`, `crates/editor/src/commands.rs`, `docs/command-line.md` |

`use_package` records declarations and closures in the Rhai state; declaration resolution is performed by the editor layer. Lock entries are keyed by plugin name under `[packages.NAME]`. The package database is a separate local record for packages installed via package commands.

## On-disk fields and examples

The lock file is located alongside the active `init.rhai`. `LockData` contains `lock_version`, a `packages` map, and flattened unknown root fields. `LockEntry` uses kebab-case field names and supports source/revision, required `sha256`, `artifact_sha256`, `spec_hash`, `tier`, `recipe_hash`, version/build metadata, permissions, capabilities, commands, modes, extensions, settings, and flattened unknown entry fields. A current entry resembles:

```toml
[packages.org-modern]
git = "https://github.com/you/forge-org-modern"
rev = "v0.2.0"
commit = "abc123"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
spec_hash = "..."
tier = "source"
recipe_hash = "..."
commands = ["org-modern-mode"]
```

Unknown fields on the root, entries, and capabilities survive a read/write cycle. Legacy lockfiles without `lock_version` continue to load. A missing lock file loads as an empty lock; an unreadable or unparsable shared or local lock file is an error that names the file and parse problem, and Forge does not rewrite it. Shared lock saves and snapshot writes use `fsutil::write_atomic`; machine-local entries from the ignored `forge-lock.local.toml` overlay shared entries by name and are not written into the shared lock.

Named snapshots are TOML files under `package-snapshots/` beside the lockfile. Verified artifacts are content-addressed under `package-artifacts/<sha256>.wasm`; restoration refuses missing or hash-mismatched artifacts rather than resolving another version. `package-restore` and snapshot restore rewrite the locked component files atomically, update installed metadata when applicable, and remove package-managed components absent from the restored lock. Snapshot save reads custom `load_path` components through the declaration rather than assuming every component is in the plugins directory.

`packages.toml` is stored in the plugins directory. The root has a `packages` table keyed by package name; each `Installed` value has `version`, `sha256`, `url`, `git`, `rev`, `commit`, and `build`. Example:

```toml
[packages.org-modern]
version = "0.2.0"
sha256 = "deadbeef"
git = "https://github.com/you/forge-org-modern"
rev = "v0.2.0"
commit = "abc123"
build = "cargo build --release --target wasm32-wasip2"
```

The optional source-specific values are omitted when absent. The package database's parse/read failure also defaults to an empty database. It is serialized and atomically written as a single file. Installed component artifacts are separately written to the plugins directory; Git checkouts are under the configured Forge data directory's `vc/NAME` subdirectory (normally `~/.local/share/forge/vc/NAME`).

While an update is applying, the data directory holds `package-journal.toml` and prior artifacts under `package-journal-artifacts/<sha256>.wasm`. The journal is machine-local, versioned (`journal_version = 1`), sorted by package name, has an `applying`/`committed` state, preserves unknown fields, and contains no timestamps. It records the prior artifact, installed record, and lock entry for each changed package. Startup recovery rolls back an `applying` journal and only removes a `committed` one. A recovery failure keeps the journal and blocks later package updates. Journal metadata is never written into the shared lockfile or snapshots.

Package indexes use strict TOML deserialization (`deny_unknown_fields`) with repeated `[[package]]` entries. A package has `name`, `version`, optional `description`, and either `url` plus `sha256` or `git` plus optional `rev`. Package indexes and installed records do not currently preserve unknown fields.

## Current build and installation behavior

- Git operations and build commands are queued asynchronously through `Processes` in `crates/core/src/process.rs`; output is accumulated in `*Package Build*`. The Git build command is a shell command executed in the checkout, with `OUT` set to the planned output path.
- Package build processes use `Processes::spawn_env` (cleared environment) with `forge_core::package::BuildPolicy`: by default only `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_*`, `TMPDIR`/`TEMP`/`TMP`, `CARGO_HOME`, `RUSTUP_HOME`, the Windows system variables, and proxy/CA variables are copied from the parent. Tokens and `SSH_AUTH_SOCK` are dropped. A declaration can extend the allowlist with `build_env` and change `build_timeout_secs`. Builds have a default 30-minute timeout: the `Vc` poll loop kills a build past its deadline and reports it as timed out. There is still no memory limit; `run_sync` supports a caller-supplied timeout for Git inspection operations.
- The build command can access the network using the host's normal network access; no offline mode or sandbox is configured. Git fetch and submodule operations also access the network.
- The npm build plan uses `npm ci --ignore-scripts --no-audit --no-fund && npx --no-install forge-plugin build ...` when a `package.json` contains Forge build metadata and a `package-lock.json` or `npm-shrinkwrap.json` is committed. Without a lockfile the plan is refused, unless the declaration supplies a reviewed `build`.
- For a new Git install, the checkout is used as the build workspace. The built component is installed through `InstalledDb::install_built`, which atomically writes the component file and separately saves `packages.toml`. This is not a staged source/build directory plus atomic swap of artifact and metadata as one transaction. For upgrades, the journal restores the artifact, `packages.toml`, and lock entry on failure or on the next startup after an interruption; it does not roll back the Git checkout/build workspace.
- Prebuilt index artifacts are checked against the declared SHA-256 before install. Their component file is atomically written; the package metadata file is then saved separately. Upgrades journal both files before installation, so a failed batch restores every package it changed.
- Build approval is requested before running a newly detected build command and remembered in the installed record; the prompt is repeated when the stored command differs. This is not a complete recipe consent record: toolchain, target, or features are not compared as a unified hash.
- Index content is read from local paths, `file://`, or `https://` using the repository's Rust HTTP client. Git itself is invoked as an external program.

## Phases 1–7 status

Status means implementation present in current source, not merely described in existing docs. “Partial” identifies existing adjacent functionality that does not meet the phase acceptance criteria.

| Phase | Status | Evidence |
|---|---|---|
| 1. Lockfile and snapshots | **Complete** | `crates/core/src/lockfile.rs` adds `lock_version`, declaration/artifact/recipe hashes, tier, capabilities, unknown-field preservation, ignored local overrides, content-addressed artifacts, named snapshots, restore primitives, and errors for malformed locks that leave the file untouched. `crates/editor/src/packages.rs` exposes snapshot save/list/diff/restore and `package-restore`; `crates/terminal/src/main.rs` provides snapshot and arbitrary lockfile diff with `--json`. Legacy locks load and unknown fields survive rewrites. Restore removes package-managed components absent from the restored lock, snapshot save includes components loaded from a custom `load_path`, and interrupted snapshot and lock restores converge when rerun. Missing local artifacts fail rather than resolving different bytes. |
| 2. Safe updates | **Partial** | `crates/editor/src/packages.rs` implements `package-upgrade NAME`, `package-upgrade-all-dry-run`, `C-u` dry runs, index release-note review, semver requirements for index entries, frozen-package filtering, and isolated candidate `init`/command-registration smoke tests. `crates/core/src/package_journal.rs` persists prior artifacts and installed/lock records, rolls back interrupted updates at startup, and makes `package-upgrade-all` batches atomic across the index and Git phases; a failed batch also reloads restored plugins that were already running. `crates/editor/src/package_vc.rs` uses `git ls-remote` for a non-mutating preview, reviews commit subjects, `BREAKING` markers, moved tags and rewritten history, and inspects remote build metadata through a fixture-testable `GitRemote` trait. Ancestry checks fetch into a scratch repository, never the real checkout. The terminal packaging tests cover dry-run filesystem non-mutation, remote build-command detection, rewritten-history and moved-tag warnings, unattended rewrite blocking, batch rollback, and the doctor-check seam. Smoke tests still use the no-op doctor seam rather than Phase 3 `package-doctor`, and the lockfile's hash check remains a startup satisfaction check, not an exact update/restore workflow. |
| 3. Diagnostics | **Partial** | `crates/core/src/package_doctor.rs` implements the Phase 3 checks (unknown/broken declarations, declared/installed mismatches, lock hashes, component compatibility, build tools, grants, orphaned VC clones, `requires`/`after` cycles, cache problems, and per-plugin sizes) behind one registry. `crates/editor/src/package_doctor.rs` provides `M-x package-doctor` and `forge package doctor [--json]` with a non-zero exit only for error findings. `crates/core/src/plugin_bisect.rs` implements a graph-aware halving search behind a `ProblemOracle` trait, with a journal and startup recovery; `crates/editor/src/plugin_bisect.rs` provides `M-x plugin-bisect` (`y`/`n` per step, C-g restores) and `forge --headless ... --test CMD`. `crates/editor/src/plugin_repro.rs` provides `M-x plugin-repro`: it writes a deterministic bundle with a minimal `requires`-closed `init.rhai`, exact pins, components and manifests, Forge/system info, and a README, while omitting grants, credentials, sessions, and unrelated config and redacting secret-looking declaration-config keys and URL credentials. Sensitive-file Git warnings are implemented in `check_sharing` behind a fixture-testable `Git` trait; per-plugin profiling/startup budget and the structured package-operation log are implemented in `crates/core/src/plugin/profile.rs`, `crates/editor/src/startup_profile.rs`, and `crates/core/src/package_log.rs`. |
| 4. Permissions and capability diff | **Partial** | `crates/core/src/permissions.rs` stores grants and manifest permissions, now in the state-directory grants file with a one-time migration from the old config-directory file. Interactive grants are bound to the component's source and SHA-256; reopening the same name with different bytes invalidates the approval and re-prompts with “same name, different binary.” `forge.plugin.grant` remains an explicit, unbound pre-grant. Unknown root values and unknown binding fields survive rewrites. `CapabilityDiff` compares an entry's recorded capabilities with the new manifest: network, HTTP, environment, directory, process and memory additions require confirmation, removals are informational, and `--batch`/`--headless` upgrades stop before replacing the installed plugin or lock entry. Index upgrades gate after download and before smoke test/install; Git upgrades gate after build and before install. `crates/core/src/plugin/host.rs` inspects component imports and the load prompt can show imports and requested grants. The lock records manifest permissions and imported interfaces. The dotfile-sharing part of Phase 4b is implemented: `crates/core/src/init_review.rs` statically compares external `init.rhai` changes and prompts only for added grants, processes, network access or writable directories, while `crates/editor/src/init_review.rs` makes a decline start like `-q`; the manual documents the share policy. There is still no explicit missing-manifest-permission verification, and recipe hashes have no user-facing diff/consent comparison beyond re-asking when the command string changes. |
| 5. Build hardening and verification | **Partial** | `crates/core/src/package.rs` verifies index artifact SHA-256 and detects build recipes; `InstalledDb` writes artifacts atomically. `BuildPolicy` scrubs the build environment to an allowlist, declarations can extend it with `build_env`, and the `Vc` poll loop enforces `build_timeout_secs` (30 minutes by default). npm plans use `npm ci --ignore-scripts` with a required lockfile. Recipe hashes include the command, features, target, and available rust-toolchain file text. Builds still run in the checkout with host network access. No isolated staging/swap, memory bound, offline build, sandbox, keyed build cache, source verification command, or reproducible-build workflow is implemented. |
| 6. Supply-chain trust | **Partial** | Index prebuilt artifacts are protected by a SHA-256 declared in the index, and build commands require consent. Package indexes can have a detached Minisign signature (`index.toml.minisig`) verified against `forge.opt["package_index_keys"]` before TOML parsing; multiple keys support rotation, and `forge.opt["package_index_strict"]` refuses unsigned indexes. Package-index TOML is parsed strictly. Git upgrades warn when the new commit is not a descendant of the locked commit and when a pinned tag moved; dry runs use a scratch repository and unattended upgrades block on a rewrite unless `allow_history_rewrite: true` is declared. `forge.opt["package_allowed_sources"]` restricts index, download and Git sources before any read or network request, with exact-host, subdomain-suffix, exact-source and directory-prefix rules; HTTP redirect targets are checked too. `forge.opt["package_repository_checks"]` optionally asks GitHub, in the background, whether a repository is archived or changed owner/name; results are warnings and never block. |
| 7. Headless CLI and CI | **Partial** | CLI supports existing plugin-related options such as `--package-entry`, plugin checks, headless plugin execution, and `--headless --test CMD` for plugin bisect. `forge package declarations [--json]` collects `init.rhai`/theme `use_package` calls without running closures, installing anything, or starting the editor; `forge package doctor`, `package sync`, `package snapshot diff`, and `package lockfile diff` replace the old headless package flags. `package sync` evaluates declarations, fetches locked commits/URLs when the artifact cache is empty, verifies artifact hashes, installs without prompting, reports text or JSON, and uses exit codes 0/1/2/3 for success/failure/bad usage/blocked approval. Build, capability and grant consent are controlled by `--allow-build`, `--allow-capabilities`, `--allow-grants` or the matching `forge.opt["package_allow_*"]` settings; without them sync stops before changing state. Remaining work is headless restore/update/verify commands, richer JSON operation output, and a CI workflow running package declarations/doctor/sync/startup budget. |

## Source/spec differences to carry forward

- The repository manifest reports package version `0.1.0`, not the spec's reference `forge:editor@0.1.29`; checkouts should treat source as authoritative and update version references if needed.
- Malformed `forge-lock.toml` content is an error naming the file and parse problem; Forge leaves the file untouched. A missing file still loads as an empty lock. Phase 1 also adds `lock_version`, preserves unknown fields, and loads legacy entries without a version.
- The extension spec's build detection list broadly matches `crates/core/src/package.rs`. The npm command is now `npm ci --ignore-scripts` with a required lockfile and `npx --no-install`, matching Phase 5's fetch-then-build intent, but builds still run in the host network (no offline step), staging/swap, memory bound or sandbox.
- Package artifact and package-db writes are individually atomic, not an atomic two-file operation. Upgrades use a durable journal to recover after interruption, but Git source/build work is not staged as part of the transaction.
- The declared plugin is not necessarily installed “at startup” in the literal sense: `crates/editor/src/declare.rs` resolves declarations during editor setup, while install/build processes run asynchronously. The new interactive `M-x package-manager` dashboard is in `crates/editor/src/packages.rs`; existing docs primarily describe `M-x list-packages`.
- Index HTTP fetching uses the repository's Rust HTTP client. Git operations use external Git.
- The implementation of lazy summaries and Wasmtime compilation cache exists, but these caches are separate from lock/snapshot or package build caches. A plugin summary is keyed by Forge version, grants, and component digest; it stores registrations and lazy-start metadata. The Wasmtime cache is under `<cache_dir>/wasmtime`.

Phase 1 is complete for lockfile, snapshot, restore, and diff behavior. Phase 2 has durable update recovery, atomic batches, remote build-metadata dry runs, and a smoke-test doctor seam; wiring that seam to the now-implemented Phase 3 doctor and staged-build hardening remain open. Phase 3 now has the package doctor, plugin bisect, reproducible `plugin-repro` bundles, per-plugin profiling/startup budgets, structured package-operation logging, and sensitive-file Git warnings. The dotfile-sharing and init-review parts of Phase 4b are implemented. Phase 7 now has declaration collection, doctor, sync, snapshot diff and lockfile diff, with sync's non-interactive approval policy; restore/update/verify commands and CI remain open.
