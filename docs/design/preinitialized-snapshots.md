# Wizer-style pre-initialized plugin snapshots

Status: Phase 8 design note. Nothing here is implemented. This is the most
speculative Phase 8 item.

## Problem and who has it

A plugin's first load pays for compiling the component, instantiating it and
running its `init`. Forge already caches compiled Wasmtime output and can
avoid running a quiet `init` via the lazy-start summary
(`crates/core/src/plugin/summary.rs:26`, `crates/core/src/plugin/host.rs`).
Neither removes the first-run cost after a fresh install or a changed
plugin.

A Wizer-style pre-initialized snapshot runs pure initialization at *build*
time and stores the resulting Wasm instance state as the artifact. The host
then starts from that state instead of replaying initialization. Potential
users are plugin authors with expensive setup (parser tables, bundled data,
precomputed indexes), users with many plugins, and CI jobs with a startup
budget.

The current cost model is visible in `PluginHost` (startup profiling,
cache lookup, compile and init timers) and in the Phase 3 startup-budget
work. That is also the evidence needed to decide whether this is worth
anything: if the profile says `init` is a small fraction, a snapshot will
not help.

Two existing features reduce the need:

- Wasmtime's compilation cache under `<cache>/wasmtime`;
- lazy-start summaries that avoid loading a quiet plugin at all
  (`crates/core/src/plugin/summary.rs:26`).

A snapshot is only justified when a plugin is genuinely expensive to
initialize and is loaded eagerly.

## Proposed Rhai surface

Keep it opt-in and static. The host should not silently replace a plugin
with an image it cannot verify.

```rhai
// Only accept author-signed snapshots from the locked source. Never create
// a snapshot from configuration or machine state.
forge.opt["preinit_policy"] = "verify";

use_package("tree-sitter", #{
  index: "tree-sitter",
  preinit: true,             // use the author's verified preinitialized image
});

// A plugin whose init needs user config, the editor, or host services is
// not eligible. Saying so explicitly is better than guessing.
use_package("lsp", #{
  index: "lsp",
  preinit: false,
});
```

Rules:

- `preinit: true` means "this declaration may use a preinitialized artifact
  published for the same locked source and recipe". It does not mean "make a
  snapshot on this machine."
- Default is `false`. A plugin without an eligible artifact loads normally.
- The index/lock must associate the preinitialized artifact with the same
  source commit and recipe. An artifact from another source, recipe,
  Forge/plugin ABI version or target must be refused.
- Host-generated snapshots are a separate, later policy and should not be
  enabled by this key. If added, they belong in a machine-local cache, never
  in the shared lock or a snapshot package.
- A plugin author opts in by publishing the preinitialized artifact through
  the same index mechanism as a prebuilt component, with the normal hash
  and signature checks.
- If `preinit_policy` is `"verify"` and no verifiable artifact exists, fall
  back to a normal build/load and say so at info level. `"require"` would
  fail; `"off"` ignores `preinit` entirely.

## Static/closure split

- `preinit` is static declaration data and is part of the static spec hash.
  It has no closure form.
- Preinitialization must not run user `init`, `preface` or `config`
  closures. It must not read editor state, environment variables, granted
  files, network, processes or the current project.
- To make that enforceable, the plugin needs a *pure* entrypoint distinct
  from normal `init`. A preinitialized image that runs the same `init` used
  by the host will either fail (host imports are not available at build
  time) or bake in build-machine state. The design therefore requires a
  separate `preinit` export (see WIT below). Without that separation, do
  not build this feature.
- A preinitialized plugin still runs its normal `init` on load. Wizer
  initializes data; the host lifecycle, registration and permission checks
  still happen at runtime.
- If a declaration has a config closure or any closure that can affect
  initialization, it is ineligible for a preinitialized artifact. Report
  that reason; do not silently ignore the closure.

## Hashing

- Add `preinit` to the declaration and `static_spec_hash`
  (`crates/editor/src/declare.rs:239`).
- The lock already records `artifact_sha256`, `sha256`, `tier`,
  `recipe_hash` and `recipe` (`crates/core/src/lockfile.rs:31`,
  `crates/editor/src/declare.rs:149`). Add an additive optional
  `preinit_sha256` (or a recipe field) only for an author-published
  artifact. The existing artifact hash must continue to identify the bytes
  that are actually installed.
- The build recipe must distinguish the source build inputs from the image
  generation inputs: preinit tool version, snapshot format, target and
  recipe. `BuildRecipe::hash` currently covers command, toolchain text,
  features and target (`crates/core/src/package.rs:904`); do not overload
  it silently. Extend it additively and version the recipe hash.
- Host-generated snapshots are keyed by artifact digest, grants, Forge
  version, recipe and target, but are never written to the lock. They are a
  cache, like the Wasmtime cache.

## Lockfile

- The lock must record enough to know which preinitialized artifact was
  used and from where. Prefer an optional `preinit_sha256` and the normal
  source/commit/recipe fields; do not put a host-specific snapshot in the
  shared lock.
- `package-snapshot-save` is unrelated to Wizer snapshots and must not be
  repurposed. The existing snapshot format stores lock entries plus
  content-addressed artifacts (`crates/core/src/lockfile.rs:322`, `:406`).
  A preinitialized artifact can be one of those artifacts because it is
  content-addressed by hash.
- If the author publishes only a preinitialized image and no source build,
  the lock records its hash and source provenance; `package verify` must say
  how to reproduce it or that it is not rebuildable.
- Restoring an old lock must not silently select a preinitialized image
  that was not recorded. If the field is absent, use the normal artifact.

## WIT changes

Prefer none in the general runtime, but this feature realistically needs one
additive hook for a *pure* preinitialization step.

Option A (preferred): add an optional world
`world plugin-with-preinit { include plugin; export preinit: func() -> result<_, string>; }`.
The export may not call host interfaces; Wizer runs it at build time. The
normal `world plugin` at `wit/forge-editor.wit:1753` is unchanged. The host
ignores `preinit` unless it is using an artifact built for that world.

Option B: no WIT change, plugin authors run Wizer externally and publish a
preinitialized component. This is only viable if their `init` is already
pure and the build tooling can separate it. In practice most plugins use
host APIs in `init`, so this is not a general solution. It may be a
useful author-only experiment.

Do not add required exports to `world plugin`; that breaks every existing
plugin. Follow the additive world/interface rule and bump the package
version if the contract is extended.

## Security implications

- A preinitialized image contains initialized memory, possibly including
  build-time secrets, absolute paths, host names, configuration or data
  baked in by mistake. Never create one from a build that had access to
  secrets, and never share a host-generated image.
- Only accept an image whose source commit, recipe, target and hash are
  pinned and, ideally, signed by the index. A snapshot must not bypass the
  normal capability diff, manifest validation or grant prompt. The host
  still inspects imports and asks for permissions at load.
- Wizer runs arbitrary plugin code at build time. That is already the case
  for a build, but it may run with different host imports or a different
  sandbox. Do not let an untrusted project request a preinit build or
  install an image automatically.
- Invalidate preinitialized images on a new compiler/runtime, target,
  declaration config, grants, Forge version or image format. A stale image
  must fail closed, not be "close enough".
- A host-generated snapshot could capture editor state or user data. Keep it
  out of the shared lock, use owner-only permissions in the machine-local
  cache, and make deletion safe.
- Wizer's component-model/WASI 0.3 support may lag. Do not ship a path that
  works only for a subset of plugins without saying so; never silently fall
  back to an unverified image.
- Verify the image's actual imports and exports. A preinitialized component
  must not gain capabilities that the normal component would not have.

## Effort

**L–XL.** It requires a new pure initialization contract, WIT/world
support, build/index changes, lock metadata, source/provenance verification,
cache invalidation and tests across at least one real plugin. It also
depends on the Phase 5 build hardening and provenance work. Without those,
the supply-chain risk outweighs a startup win.

## What would make this not worth building

- Startup profiles show that plugin `init` is not a meaningful part of the
  budget; lazy summaries and the AOT cache already solve the real cost.
- Wizer cannot reliably preinitialize the component model and the supported
  WASI versions, or it cannot separate pure preinit from host-dependent
  `init`.
- There is no trustworthy way to verify that a published image corresponds
  to the locked source and recipe.
- The team cannot guarantee that images contain no secrets or machine-local
  state.
- Phase 5's staged builds, offline builds, sandboxing and reproducible-build
  story are not done; a preinitialized image adds supply-chain risk before
  the basics exist.
- The work turns into "snapshot the entire Wasmtime store", including host
  resources and process handles. That is not safe or portable.

## Sources

- `crates/core/src/package.rs:871` — `BuildRecipe`; `:889` —
  `for_checkout`; `:904` — `hash`; `:917` — `diff`.
- `crates/core/src/plugin/summary.rs:26` — lazy summary; `:96` — digest;
  `:102` — key.
- `crates/core/src/plugin/host.rs` — Wasmtime cache and startup profiling;
  `:589` — `load_request`.
- `crates/core/src/lockfile.rs:31` — `LockEntry`; `:322` — snapshot save;
  `:406` — snapshot restore.
- `crates/editor/src/declare.rs:149` — lock entry creation; `:239` —
  `static_spec_hash`.
- `wit/forge-editor.wit:1735` — `handler`; `:1753` — `plugin` world;
  `:1806` — `plugin-full` world.
- `docs/package-manager-spec.md:120`, `:122`, `:144` — build cache,
  toolchain file and Phase 8 scope.
- `docs/package-manager-audit.md:88` — current build hardening status.
