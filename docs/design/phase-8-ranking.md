# Phase 8 ranking

Status: design recommendation. No Phase 8 item is implemented.

This ranks the six Phase 8 ideas by value over cost for the current Forge
codebase. "Cost" includes implementation, review, compatibility, security
and the amount of new policy the package manager has to own. "Value" means
solving a real problem that the existing `use_package` system cannot solve
cleanly, especially for headless/CI use.

| Rank | Item | Effort | WIT change | Main dependency / risk | Recommendation |
|---|---|---|---|---|---|
| 1 | [Conditional declarations](conditional-declarations.md) | M | None | Must settle project-condition trust; otherwise OS/host is small | Build the OS/arch/host/env subset first |
| 2 | [Profiles](profiles.md) | M | None | Lock union/preserve behavior; duplicate-name policy | Build after condition plumbing, or together with it |
| 3 | [Config bundles](config-bundles.md) | M | None | Manifest parsing, precedence, upgrade review | Best quick win if plugin authors will publish bundles |
| 4 | [Managed toolchains](managed-toolchains.md) | L | None | Phase 5 build hardening and Phase 6 provenance are incomplete | Defer until builds are staged/offline and verifiable |
| 5 | [Plugin state across reload](plugin-hot-reload-state.md) | L | Additive optional world | Secrets, lifecycle, background calls; plugins can already use their cache dir | Defer; needs a WIT contract and clear invalidation |
| 6 | [Wizer-style snapshots](preinitialized-snapshots.md) | L–XL | Optional pure-preinit world | Component/WASI support, supply-chain verification, secrets in images | Do not build now |

## 1. Conditional declarations (M)

**Why first.** This is the most direct fix for a correctness gap in the
existing static/closure split. `when`, `if` and `unless` already accept
closures, but collection deliberately leaves them unevaluated
(`crates/core/src/rhai/setup.rs:1423`, `crates/core/src/collect.rs:125`).
That means a headless `package sync` cannot decide a condition that the
interactive editor can. Adding host facts as *data* instead of arbitrary
closures is a natural extension of the current model, needs no WIT change,
and improves `package declarations`, `doctor`, sync and reporting at the
same time.

The OS/host/environment subset is moderate and testable. The project
condition is the risk: startup has no active project, project code is
untrusted until granted, and a project must not be able to install packages
or widen grants. Do not block the whole feature on that; ship the host
conditions first, then design the project boundary separately.

**Cost drivers.** Canonical condition serialization, collection/report
states, hash integration, hostname/env privacy and headless tests. No new
plugin API.

**Do not build it if** existing boolean `forge.system` checks and closures
are sufficient, or if project conditions cannot be separated from package
installation. See the detailed note for the full rejection list.

## 2. Profiles (M)

**Why second.** Profiles are a high-value user-facing grouping feature and
are almost entirely host-side: a static `profiles` key plus a selection
call. They reuse the same "inactive declaration" reporting that conditional
declarations need, so doing them after item 1 avoids two parallel
filtering mechanisms.

They are not first because the lock policy is real work. The lock is keyed
by package name and restore prunes entries absent from the shared lock
(`crates/core/src/lockfile.rs:168`, `:406`). A normal profile switch must
not delete another profile's packages. That requires either a union lock
with optional profile metadata or an explicit "lock is a cache, do not
prune on profile change" policy. Profile-specific duplicate package names
must also be rejected.

**Cost drivers.** Lock union/preserve behavior, duplicate-name validation,
project-boundary rejection, report reasons and headless tests. No WIT
change.

**Do not build it if** users already split config files and lock pruning
cannot be made non-destructive. See the detailed note.

## 3. Config bundles (M; S if lock diff is skipped)

**Why third.** This is the lowest-risk feature in the set: manifest parsing,
validation against existing `[[settings]]` and one static declaration key
(`crates/core/src/permissions.rs:606`, `:802`). It needs no WIT, no new
lifecycle and no host-fact evaluation. For plugin authors it turns "copy
these six settings from the README" into a named preset.

It ranks below conditions and profiles because the value is narrower:
existing `[[settings]]` defaults plus a `config` table already solve the
basic case. The feature earns its place only if plugin authors publish
bundles and users need upgrade review of changed recommendations.

**Cost drivers.** Precedence (`config` > bundle > default), unknown/wrong
settings errors, hash/lock metadata for effective config, and update
review. The first version can ship without a lock `config_hash`, but then
silent bundle changes on upgrade are harder to review.

**Do not build it if** very few plugins would publish bundles, or if the
precedence rules become ambiguous with config closures. See the detailed
note.

## 4. Managed toolchains (L)

**Why fourth.** The value is real: `BuildRecipe` stores only the raw
toolchain file text, not the resolved compiler (`crates/core/src/package.rs:871`,
`:889`, `:904`), so two machines can build different bytes from the same
checkout. But the cost and risk are much higher than the host-side
declaration features, and the prerequisites are not done: Phase 5 still
lacks staged/swap builds, an offline build step, a build cache keyed as
specified, and memory isolation (`docs/package-manager-audit.md:88`).
Phase 6 provenance is also partial. Installing and executing a compiler is
a supply-chain operation; doing it before the basic build hardening exists
adds risk without delivering reproducibility.

The package-manager spec already says managed toolchains are out of scope
for now (`docs/package-manager-spec.md:122`). Respect that until Phase 5 is
complete. A much smaller improvement — resolve the existing toolchain file,
record the version and fail with a clear doctor message when it is missing
— could be considered earlier if users are blocked.

**Cost drivers.** Toolchain resolution, installation consent, versioned
recipe hashes, build-cache keying, cross-platform rustup handling, cleanup
and provenance. No WIT change.

**Do not build it if** rustup already handles the repository file, or if
"managed toolchains" expands into a general language/environment manager.

## 5. Plugin state across reload (L)

**Why fifth.** The problem is real: reload unloads and discards the store
(`crates/core/src/plugin/host.rs:589`, `:733`), and the lazy summary only
holds registrations (`crates/core/src/plugin/summary.rs:26`). But this is
the first item that normally needs an additive WIT world and SDK support,
and plugin authors can already persist durable rebuildable data under
`~/.cache/forge/plugin-data/NAME` (`wit/forge-editor.wit:817`). The
remaining value is in-memory state for expensive plugins, which is a
narrower audience.

The security and lifecycle complexity is high: state can contain tokens or
document text, must be invalidated on a new binary/grants/Forge version,
and must not be saved during background/async work. Do not fake it with a
host-side summary. Either add an optional state world with clear
invalidation or defer it.

**Cost drivers.** WIT world, SDK/runtime support, state keying, atomic
owner-only storage, size/TTL limits, background-call handling and failure
tests.

**Do not build it if** plugins can use their cache directory, lazy
summaries are sufficient, or the team cannot safely invalidate state.

## 6. Wizer-style pre-initialized snapshots (L–XL)

**Why last.** This is the most speculative item. The existing lazy summary
and Wasmtime AOT cache already avoid a lot of startup cost; a snapshot only
helps a genuinely expensive, eager plugin. It needs a pure initialization
contract distinct from normal `init` (otherwise build-time initialization
is either impossible or bakes in host state), plus source/recipe
verification and a component/WASI compatibility story. Wizer's support for
the component model and WASI 0.3 may not be sufficient.

The security surface is also the worst: a preinitialized image is memory
that may contain secrets, absolute paths or machine state. It must never be
created from a secret-bearing build or shared from a host cache. The
benefit is a startup optimization; the cost is a new supply-chain artifact.
Do not build it before Phase 5 and Phase 6 are substantially complete, and
do not build it at all if startup profiles show `init` is not the cost.

**Cost drivers.** Pure preinit export/world, build/index support, image
provenance, lock metadata, cache invalidation, cross-platform/runtime tests
and a real plugin benchmark.

**Do not build it if** lazy/AOT caching already meets the startup budget or
Wizer cannot handle the supported components. See the detailed note.

## Dependency and delivery order

1. **Conditional declarations** — build the host-condition data type,
   canonicalization and collection/report integration. This creates the
   "inactive for a host reason" plumbing.
2. **Profiles** — reuse that plumbing for profile filtering; solve lock
   union/preserve and duplicate-name rejection before exposing the feature.
3. **Config bundles** — independent of 1 and 2; can be done in parallel if
   someone owns manifest parsing and upgrade review.
4. **Managed toolchains** — only after Phase 5 staged/offline builds and
   Phase 6 provenance; otherwise the managed compiler is a new trust hole.
5. **Plugin state** — after an additive WIT compatibility plan and a decision
   about background calls; keep it opt-in and never in the shared lock.
6. **Wizer snapshots** — after a pure preinit contract and verifiable
   artifact provenance; treat it as an optimization, not a package-manager
   requirement.

## If only one or two are built

- **One:** build conditional declarations. It fixes a headless correctness
  gap, uses existing host facts, and needs no WIT.
- **Two:** conditional declarations plus profiles. Profiles are the
  user-facing grouping feature most likely to replace manual `init.rhai`
  editing; the condition/report plumbing is shared.
- **Cheapest useful follow-up:** config bundles, if plugin authors will
  publish them. It is self-contained and has no WIT or lifecycle cost.
- **Do not start with:** managed toolchains, hot-reload state or Wizer
  snapshots. Each is L or larger, and each has a prerequisite (Phase 5,
  WIT compatibility, or a pure preinit contract) that is not met today.

## Sources

- `crates/core/src/rhai/setup.rs:1423` — `use_condition`; `:1351` —
  `use_package` keys; `:1162` — host/system facts.
- `crates/core/src/collect.rs:125`, `:157` — conditional/deferred collection.
- `crates/core/src/declare.rs:115`, `:135` — declaration states and deferred
  code.
- `crates/editor/src/declare.rs:239` — `static_spec_hash`.
- `crates/core/src/lockfile.rs:168`, `:406` — lock and restore/prune.
- `crates/core/src/permissions.rs:606`, `:802` — settings and validation.
- `crates/core/src/package.rs:871`, `:889`, `:904` — build recipe and hash.
- `crates/core/src/plugin/host.rs:589`, `:733` — load/reload unload path.
- `crates/core/src/plugin/summary.rs:26`, `:102` — lazy summary and key.
- `wit/forge-editor.wit:817` — plugin cache directory; `:1735`, `:1753` —
  plugin exports and world.
- `docs/package-manager-spec.md:120`, `:122`, `:144` — build cache,
  toolchain scope and Phase 8 list.
- `docs/package-manager-audit.md:75`, `:88` — build hardening and recipe
  status.
- `docs/design/profiles.md`, `conditional-declarations.md`,
  `config-bundles.md`, `managed-toolchains.md`,
  `plugin-hot-reload-state.md`, `preinitialized-snapshots.md` — detailed
  notes.
