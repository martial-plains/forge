# Forge package manager: extension spec

> For GitHub Copilot (agent mode or chat). Place at `docs/package-manager-spec.md` and reference it with `#file:docs/package-manager-spec.md`. Copy the "Ground rules" section into `.github/copilot-instructions.md` so they apply to every request.

Forge already has a working package manager built around `use_package`. This spec does **not** ask for a new one. It lists what exists, then what to add so Forge's package management is safer to update, easier to debug, and more trustworthy than lazy.nvim, use-package, straight.el or elpaca.

Everything in section 1 comes from `README.md`, `docs/plugin-api.md` and `docs/init.example.rhai` as of `forge:editor@0.1.29`. The author of this spec has **not read the source**, so Phase 0 is an audit. If this document and the code disagree, the code wins; record the difference.

## 1. What already exists (do not rebuild)

**Declaring plugins (Rhai, in `init.rhai`)**
- `use_package(name, #{ ... })` and `forge.plugin.declare(...)`. Sources: `git`/`vc` (URL or map with `url`, `rev`, `make`, `lisp_dir`, `features`), `index` (a name in a package index), `url` + `sha256` (prebuilt component), or a plugin already in the plugins directory.
- Keys include `pin`, `config`, `grant`, `hook`, `bind`, `bind_mode`, `bind_prompt`, `commands`/`modes`/`extensions` (lazy triggers), `after`, `requires`, `init`, `preface`, `custom`, `when`/`if`/`unless`, `disabled`, `demand`, `ensure`, `ensure_system_package`, `load_path`. `use_package_report()` says what each declaration did.
- Anything not installed is fetched at startup. `forge-lock.toml` (next to `init.rhai`) records the commit and sha256 each declaration resolved to.

**Packages and indexes**
- A package index is a TOML file (path, `file://` or `https://`) with `[[package]]` entries: `name`, `version`, `description`, `url` + `sha256`, or `git` + `rev`.
- Commands: `list-packages` (RET install/upgrade, `d` delete, `g` refresh, `U` upgrade all), `package-install`, `package-delete`, `package-upgrade-all`, `package-refresh-contents`, `package-vc-install` (`URL@REV`), `package-vc-upgrade`, `package-vc-upgrade-all`.
- Installed state is in `packages.toml` in the plugins directory (repository, rev, commit, build command). VC clones live in `~/.local/share/forge/vc/NAME`. A sha256 mismatch is refused. Downloads happen in the background.
- Build detection order: `forge-package.toml` (`build`, `wasm`, `name`), executable `build.sh`, `Cargo.toml` (`cargo build --release --target wasm32-wasip2`), `package.json` with a `"forge"` field, or exactly one `.wasm`. Forge shows the build command and asks first, and asks again only if the command changes. Output goes to `*Package Build*`.

**Plugins, permissions and runtime**
- Plugins are WASM components for the `forge:editor/plugin` world in `wit/forge-editor.wit`, run in Wasmtime.
- The manifest is TOML, embedded as a `forge-manifest` custom section (the Rust SDK builds it from `[package.metadata.forge]`) or a sidecar `NAME.toml` that overrides it. It declares `[package]`, `[[commands]]`, `[[options]]`, `[[settings]]`, `requires`, `modes`, `extensions`, `startup`, and `[permissions]` (`network`, `http`, `environment`, `processes`, `directories`, `memory-mb`).
- Forge shows requested permissions **and every interface the component imports**, then asks. Answers live in `~/.local/state/forge/plugin-grants.toml` (migrated from the config directory); `forge.plugin.grant` pre-grants. It asks again when a plugin wants something new or the same name resolves to different bytes. `list-plugins` shows grants and their source; `revoke-plugin-permissions` forgets them.
- Lazy start: after `init`, Forge saves a summary (commands, keys, modes, subscribed events). If the file and grants are unchanged, the next start loads from the summary without compiling or running the plugin. Compiled code is cached in `~/.cache/forge/wasmtime`. Rebuilt plugins reload automatically (`plugin_auto_reload`).
- Limits: 5 s per call (60 s background), C-g stops a busy plugin and unloads it, 128 MB memory unless `memory-mb`. A trap unloads the plugin and logs to `*Messages*`.
- Versioning: within `0.1.x` interfaces and functions may only be **added**. A plugin loads if every function it uses exists in the running Forge, otherwise loading fails with an error naming the function.
- Testing aids: `forge --headless --plugin P.wasm --keys ... --screen`, `forge --check-plugin FILE --run CMD`, WAT fixture plugins in `tests/fixtures/`.

## 2. Ground rules for Copilot

- Work **one phase task at a time**. Before coding a task, post a short plan (files, public items, tests) and wait for approval. Prefer small PRs.
- **Phase 0 comes first.** Do not start Phase 1 until the audit is reviewed.
- Configuration stays **Rhai**. Do not introduce a TOML user spec. New options are `forge.opt[...]` keys or new `use_package` keys, documented in `docs/init.example.rhai`. New file formats are limited to the existing ones (`forge-lock.toml`, `packages.toml`, manifests, package index), extended **additively**: old files must still load, and unknown fields must be preserved on rewrite.
- `use_package` accepts closures (`init`, `config`, `when`), so a declaration is not pure data. Treat the data keys (source, pin, triggers, grants, `requires`, `after`) as the **static part** used for hashing, planning and caching. Closures run only when needed.
- WIT changes follow "Versions and compatibility" in `docs/plugin-api.md`: additions only, no new variant cases, bump the package version and extend the history paragraph. Prefer solving things host-side without WIT changes.
- Rust 2024, MSRV 1.85. `cargo fmt`, `cargo clippy` (with the repo's `clippy.toml`), `cargo deny` (`deny.toml`), `cargo test`, and `cargo build --workspace` must pass. No `unwrap()`/`expect()` outside tests.
- Writes are **staged then atomically renamed**; every operation is idempotent and resumable (a killed update never leaves a half-installed plugin). Never hand the full process environment, tokens or `SSH_AUTH_SOCK` to a build.
- Logic must be testable natively; keep network, git and clock access behind traits so tests use local fixtures only. No real network in tests.
- Every new `M-x` command gets a doc string and follows the existing `package-*` / `plugin-*` naming. Document user-visible changes in `docs/plugin-api.md`, `docs/init.example.rhai`, `docs/manual/forge.org` and `NEWS.org`.
- Keep code platform-neutral (Linux, macOS, Windows). The WinUI app is not verified to compile, so don't depend on it.
- Errors name the plugin, the declaring `init.rhai` line when known, and the next step to fix it.

## 3. Phases

### Phase 0: audit (no behavior changes)

Write `docs/package-manager-audit.md` covering, with file paths:
- where `use_package` / `forge.plugin.declare`, the lockfile reader/writer, `packages.toml`, the build runner, the grant store, the lazy-start summary and the wasmtime cache live (likely under `crates/editor` and `crates/core`, but confirm);
- the actual field names and an example of `forge-lock.toml` and `packages.toml`;
- for each item in Phases 1 to 7: **done / partial / missing**, with evidence;
- how a build is run today: environment passed, network access, timeouts, whether `npm install` runs lifecycle scripts, whether install is staged and atomically swapped.

Acceptance: reviewed by the maintainer; the phase list below is adjusted to match.

### Phase 1: lockfile and snapshots

- Extend `forge-lock.toml` (additively, with a `lock_version`) per plugin: `spec_hash` (hash of the static declaration), `artifact_sha256` of the installed `.wasm`, `tier` (`prebuilt` | `source`), `recipe_hash` for source builds (command, toolchain, features, target), and `capabilities` (manifest permissions plus imported interfaces).
- `package-snapshot-save NAME`, `-list`, `-diff A B`, `-restore NAME`. A snapshot is the lockfile plus the installed artifacts' hashes; restore re-installs or re-uses by hash.
- `package-restore`: make installed state match the lockfile exactly, including downgrades, and report any plugin whose resolved commit or sha256 differs.
- Per-machine overrides in an ignored `forge-lock.local.toml` so switching machines doesn't churn the shared lockfile.
- Lockfile diff output readable in a buffer and as `--json`.

Acceptance: round-trip tests (save, change, restore); old lockfiles load; unknown fields survive a rewrite; restore after a killed run converges.

### Phase 2: safe updates

- `package-upgrade-all-dry-run` (and `C-u` on upgrade commands): list exactly which plugins change, old and new commit or version, and which build commands would run, without touching anything.
- Update = snapshot, apply, **smoke test**, **auto-rollback on failure**. Smoke test: every updated plugin loads and its `init` runs; commands it declares are registered; `doctor` (Phase 3) reports no new errors. Failure restores the snapshot and says which plugin broke and why.
- Update a single plugin and test it without touching the rest (`package-upgrade NAME`, which should compose with the above).
- Show a changelog before applying: commit subjects between the locked and new commit for git sources, flagging `BREAKING` markers and tag moves; release notes if the index provides them.
- `pin` and a new `freeze: true` key hold a plugin; semver-range support for **index** packages (`version: "^1.2"`), exact for git.

Acceptance: a fixture plugin whose new version fails `init` leaves the editor and lockfile in the previous working state, automatically; dry-run makes zero filesystem changes (assert by hashing the plugin and data directories).

### Phase 3: diagnostics

- `package-doctor` (also usable headless, exit code non-zero on problems): broken or unknown `use_package` keys, declared but not installed, installed but not declared, lock/installed hash mismatch, plugin needs a function this Forge lacks, missing build tools, grants that no longer match the manifest, orphaned VC clones, dependency cycles in `requires`/`after`, cache problems, per-plugin size report.
- `package-doctor` warns when `plugin-grants.toml`, authinfo/netrc credential files, the plugins directory, or `session.toml` is inside a Git work tree and is not ignored; it prints the exact `.gitignore` lines to add.
- **Install-time compatibility preflight**: read the component's imports and compare them with what this Forge provides for `forge:editor`; show "needs a newer Forge (missing `X`)" in `list-packages`, before install, and refuse in `package-install` unless forced.
- `plugin-bisect`: disable halves of the loaded plugins until the problem disappears, then name the culprit. Respect `requires` (dependencies stay with their dependents) and lazy plugins. Interactive (`y/n` after each step) and scripted (`forge --headless ... --test CMD`) modes. Always restore the user's original state afterwards.
- `plugin-repro NAME`: write a directory containing a minimal `init.rhai` (only that plugin and its dependencies, pins included), the `.wasm` and manifest, the Forge version and system info, ready to attach to a bug report.
- `plugin-profile`: per plugin, time spent in cache lookup, compile (on cache miss), `init`, lazy start, and each call (count, total, max). Show in a buffer and via `forge --profile-startup`. Add `forge.opt["startup_budget_ms"]`: warn in `*Messages*` when exceeded and fail in headless/CI mode.
- Structured per-operation log for package operations (fetch, build, install, upgrade, rollback) with timings, viewable in a buffer and exportable as JSON lines.

Acceptance: bisect test over a fixture set with one failing plugin finds it in O(log n) steps and restores state; doctor has one test per check; profile output is deterministic for a fixture.

### Phase 4: permissions and capability diff

- Move interactive `plugin-grants.toml` to the state directory (`~/.local/state/forge`, following the platform's state-directory convention) so machine-local approvals are not stored beside synchronized configuration. Continue to load the existing config-directory file as a migration source, then write new approvals only to the state directory.
- Bind saved grants to the plugin name and its declared source or artifact hash. When the same name resolves to a different binary, invalidate the old binary-bound approval, re-prompt, and make the prompt explicitly say **“same name, different binary.”** Preserve `forge.plugin.grant` as an explicit config pre-grant.
- Record each plugin's `capabilities` in the lockfile (Phase 1). On upgrade, diff old and new capabilities; **additions require explicit confirmation** and block unattended upgrades; removals are informational.
- Verify at load and install that the component's imports are covered by the manifest and grants. If an import needs a permission the manifest doesn't declare, surface it (the existing prompt already lists imports; make the mismatch explicit and refuse silent escalation).
- Show a **build-recipe diff** when a source plugin's build command, toolchain or features change (the existing consent prompt already re-asks on command change; extend it to the other inputs).
- `list-plugins` / `describe-plugin` show the capability set and when each grant last changed.

Acceptance: upgrading a fixture plugin that adds `network` or a writable directory prompts and, when unattended, fails with a clear message and no changes.

### Phase 4b: dotfile and sharing safety

Document this share policy in a “Sharing your configuration” section and ship the corresponding `.gitignore` snippet:

| Share | Keep out of shared repositories |
|---|---|
| `init.rhai`, `forge-lock.toml` | `plugin-grants.toml`, authinfo/netrc credentials, the plugins directory, `session.toml` |

- Move per-machine interactive approvals to the state directory (`~/.local/state/forge` or the platform equivalent). Migrate grants from the old config-directory file without losing or broadening approvals; write subsequent changes only to the new location.
- Bind interactive grants to plugin name and artifact hash. When the same name presents a different binary, invalidate the old binary-bound grant and re-prompt with the exact explanation **“same name, different binary.”** Config `forge.plugin.grant` remains an explicit pre-grant.
- `package-doctor` warns if any sensitive file listed above is inside a Git work tree and not ignored, and prints copyable `.gitignore` rules for the specific paths.
- Add opt-in `forge.opt["init_review"]`. If `init.rhai` changes outside Forge and the new version adds grants, process permissions, network access, or writable directories, show the diff and ask before evaluating it. Declining must start Forge with the same configuration/code behavior as `-q`.

Acceptance: migration preserves existing grant decisions; changing a plugin binary under the same name re-prompts; doctor reports each unignored sensitive path with ignore guidance; init review only prompts for the specified external changes and decline starts without evaluating the init file.

### Phase 5: build hardening and verification

- Build in a staging directory and **atomically swap** on success; a failed build keeps the previous working plugin.
- Scrub the build environment to an allowlist (PATH, HOME-scoped build dirs, toolchain variables); time and memory limits.
- Two-step builds where the toolchain supports it: fetch dependencies first (`cargo fetch`/`--locked`, `npm ci --ignore-scripts`), then run the build **offline**. A build that needs the network in the offline step fails with a clear message. Sandbox backends behind a trait: container (podman/docker) first, native (Landlock, `sandbox-exec`) later. Unsupported platforms fall back to today's behavior plus the consent prompt, and say so.
- Build cache keyed by `(commit, recipe_hash, toolchain, target)`; reuse across upgrades and snapshots. Cache the AOT output separately (it is per machine, the `.wasm` is not).
- `package-verify NAME`: rebuild from source and compare the output sha256 with the installed or published artifact. Report reproducible, differs, or not rebuildable. Document how plugin authors make builds reproducible.
- Respect a plugin's `rust-toolchain.toml`; record the toolchain version in the recipe hash. Managed toolchains are out of scope for now.

Acceptance: a build that tries to read an environment secret or reach the network offline fails in a test; kill mid-build leaves the old plugin intact; two builds of the fixture plugin produce identical hashes.

### Phase 6: supply-chain trust

- Signed package indexes: optional detached signature verified against keys in `forge.opt["package_index_keys"]`; a strict mode refuses unsigned indexes.
- Git sources: warn when the new upstream commit is not a descendant of the locked commit (force-push or rewritten history), and when a pinned tag now points somewhere else.
- Optional checks where an API is available (repository archived, owner changed), off by default and never blocking.
- Allowlist mode: `forge.opt["package_allowed_sources"]` restricts hosts and indexes.

Acceptance: tests with local git repositories that rewrite history trigger the warning; tampered index fixtures are refused.

### Phase 7: headless CLI and CI

- Follow the existing CLI style (see `docs/command-line.md`; `--package-entry` is an example). Provide, without starting the UI: sync (install what `init.rhai` declares, honoring the lock), restore, update (with `--dry-run`), snapshot, doctor, verify, bisect. Evaluate `init.rhai` headlessly to collect declarations (the `use_package_report()` machinery is a starting point).
- `--json` output and meaningful exit codes. Document a GitHub Actions example that runs `doctor` and the startup-budget check.

Acceptance: a fresh machine with only `init.rhai` and `forge-lock.toml` reaches the locked state using one command; CI example runs in the repo's own workflow.

### Phase 8: later, only if wanted

Profiles (`forge.profile("work")` enabling subsets of declarations), conditional declarations by OS/host/project beyond `when`, state serialize/restore across plugin hot reload (today reload starts fresh), plugin-recommended default config bundles, Wizer-style pre-initialized snapshots, managed toolchains.

## 4. Definition of done (every task)

1. Builds, formatted, clippy- and deny-clean.
2. Tests added (native unit tests plus headless/fixture tests where behavior is user-visible).
3. Docs updated (see ground rules) and a `NEWS.org` entry.
4. Behavior matches this spec, or the PR description proposes a spec change.

## 5. Open questions for the maintainer

- Sandbox strategy for builds: container-first, or native per platform?
- Should signed indexes be required for the future public index, or stay optional?
- CLI shape: new subcommands, or more flags in the current style?
- Do you want `forge-lock.toml` to stay hand-diffable (stable ordering, no timestamps)? This spec assumes yes.
