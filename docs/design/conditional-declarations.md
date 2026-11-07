# Conditional declarations by host, OS or project

Status: Phase 8 design note. Nothing here is implemented.

## Problem and who has it

`use_package` already accepts `when`, `if` and `unless`
(`crates/core/src/rhai/setup.rs:1423`). A value can be a boolean or a
function. During collection a function is deliberately **not** called:
`collect_str` records `DeclarationState::Conditional` and
`DeferredCode::Condition`, so `forge package declarations` and headless sync
know the declaration exists but cannot decide whether it applies
(`crates/core/src/collect.rs:125`, `crates/core/src/declare.rs:115`).

That is the right safety rule, but it leaves common conditions in user code:

- "this plugin is macOS-only" or "this plugin is Linux-only";
- "this only applies on my corporate host";
- "this only applies inside a project";
- "this is a CI-machine package".

Today some of these can be written as static booleans using
`forge.system.type()`, `forge.system.name()` and similar facts
(`crates/core/src/rhai/setup.rs:1162`). But the result is an opaque boolean.
Collection cannot report *why* it was false, headless runs cannot explain a
skipped package, and hostname or project logic tends to be copied into
closures. Closures then become conditional declarations that no headless
sync can evaluate, which is exactly the failure mode Phase 7 is trying to
remove.

The users are people with cross-platform dotfiles, people with work and
personal machines, CI maintainers, and anyone using a project-local
`.forge/init.rhai` to add a package for one checkout.

## Proposed Rhai surface

Add a `forge.when` namespace that returns plain data, not closures, plus
support for those values in the existing `when`/`if`/`unless` keys. Keep
the existing boolean and closure forms for compatibility.

```rhai
// init.rhai
use_package("mac", #{ index: "mac", when: forge.when.os("macos") });
use_package("linux", #{ index: "linux", when: forge.when.os("linux") });

use_package("ci", #{ index: "ci", when: forge.when.all([
  forge.when.env_set("CI"),
  forge.when.host("*.build.example.com"),
]) });

// .forge/init.rhai: the project root is known here.
use_package("app-tools", #{ index: "app-tools", when: forge.when.project() });
```

Suggested constructors:

- `forge.when.os("macos" | "linux" | "windows" | "gnu")` — compares
  against the platform facts already exposed by `forge.system`. Map the
  friendly names to the values from `environment.system-type` in
  `wit/forge-editor.wit:926`; do not expose a second, inconsistent platform
  spelling.
- `forge.when.arch("aarch64" | "x86_64" | "...")` — optional, from the
  target architecture.
- `forge.when.host("pattern")` — exact hostname or a small glob (`*` and
  `?` only, case-insensitive). No regular expressions; this avoids a
  user-configurable ReDoS and keeps matching clear.
- `forge.when.env_set("NAME")` — true when the process environment has the
  variable. Do **not** make value comparison the default; values often hold
  tokens.
- `forge.when.env_is("NAME", "value")` — optional, for explicitly
  non-secret values such as `CI=true`. The expected value may be recorded in
  the declaration, but never in a report or log.
- `forge.when.project()` — true when the active project root is a Forge
  project. `forge.when.project("/path")` matches that canonical root;
  `forge.when.project("~/src/*")` matches a path glob. Project conditions
  are only legal in a project `.forge/init.rhai` or when a project is
  already active. In global `init.rhai` they are an error, not
  `Conditional`, because startup has no active project.
- `forge.when.all([...])`, `forge.when.any([...])`, `forge.when.not(...)` —
  combine conditions. These are ordinary nested maps/vectors and can be
  canonicalized.

The exact spelling of the map returned by `forge.when` is internal. It only
needs to be data and to serialize deterministically.

## Static/closure split

- `forge.when.*` returns a data condition. Collection evaluates it with
  host facts and records `Active` or `Skipped(...)`. It must not call a
  closure, read an arbitrary file, run a process or make a network request.
- A plain boolean `when` stays exactly as it is today.
- A function `when` stays deferred. It is still reported as
  `Conditional`, with a message saying that a closure prevented a headless
  decision. Do not pretend a closure is a data condition.
- Add structured reasons for the new false cases, for example
  `SkipReason::WhenFalse` plus a `UseReport` detail, or a reason carrying
  the failing condition. `SkipReason` is internal serialized data; an
  additive variant is acceptable, but keep the existing variants for old
  reports.
- Evaluate `profiles` (if that feature exists) before conditions. An
  inactive profile should not even inspect a project path or environment
  variable.
- A project condition in a global init is a validation error. Do not defer
  it and silently load the package later; that would make startup behavior
  depend on which buffer the user opens.

## Hashing

- Add an optional canonical `condition` value to `PluginUse` (or a
  declaration-level equivalent) and include it in `static_spec_hash`
  (`crates/editor/src/declare.rs:239`).
- Canonicalize before hashing: sort map keys, preserve array order, and
  normalize platform aliases. A change from `when: true` to a condition
  tree, or a change in a host glob, must change the static hash.
- Do **not** put the evaluated result or the current hostname/OS in the
  hash. The same declaration is the same declaration on another machine;
  only the decision differs.
- An exact hostname or an env value that is part of the declaration will be
  hashed as written. Warn authors not to put secrets in `env_is`; use
  `env_set` for tokens.
- Keep condition closures out of the static hash, as today, and make their
  presence visible in the report.

## Lockfile

- The lock remains a resolution cache, not a list of "packages for this OS".
  It may add an optional canonical `condition` field to `LockEntry` for
  diagnostics, but it must not record the hostname, the active OS or the
  evaluated boolean.
- `restore` should still restore all locked artifacts byte-for-byte. It must
  not become a host-condition evaluator.
- `sync` evaluates the current host's conditions to choose what to install.
  It records the selected entries and preserves entries whose conditions do
  not apply here, so switching from macOS to Linux does not delete the
  macOS artifacts from the shared lock.
- If the lock is pruned on restore, a macOS-only entry must not be deleted
  merely because the current machine is Linux. Add the optional metadata or
  change the prune policy before enabling condition-aware sync; do not rely
  on the current active subset as the definition of the lock.
- No `lock_version` bump is needed for additive optional metadata. Old
  locks without a condition still load; their condition is simply unknown.

## WIT changes

None. Host facts already exist in the `environment` interface at
`wit/forge-editor.wit:926`. Conditions are evaluated on the host side before
a plugin is loaded.

## Security implications

- Hostname matching must not leak the hostname into shared files or logs.
  The declaration contains the pattern; reports should say "host condition
  was false", not print the machine name unless the user asks for a debug
  dump.
- Environment conditions are a secret-leak risk. Prefer presence checks.
  Never echo a value in `use_package_report()`, `package declarations
  --json`, `package doctor`, build logs or error messages.
- Project conditions must not let an untrusted checkout install packages or
  widen grants. A project's `.forge/init.rhai` already needs an explicit
  project-code grant (`crates/editor/src/project.rs:22`), but a granted
  project still should not trigger automatic package installation or a new
  source URL. Restrict project conditions to loading already-installed,
  locked components, or require the same package-install approval as any
  other install.
- Project roots are canonicalized today (`crates/core/src/project.rs:28`).
  Match the canonical root, not a symlink path. Do not follow a project path
  outside the granted root when evaluating a glob.
- Globs are limited to `*` and `?` for hostnames and project paths. Do not
  accept user-supplied regular expressions, and do not let a condition
  execute arbitrary Rhai.
- A false condition must be visible. Hidden declarations are a review and
  debugging problem; `package doctor` should list them with the reason.
- Do not make condition evaluation depend on the current buffer when
  collecting global init. The result must be stable for a headless run.

## Effort

**M.** The predicate data type, canonicalization, collection integration,
report reasons and tests are moderate. The project condition is the hard
part: it needs a clear answer for startup, project grants, install approval
and lock preservation. An MVP of OS, arch, host and env-presence is much
smaller; project conditions should follow only after that boundary is
settled.

## What would make this not worth building

- A boolean using `forge.system.type()` plus `when` closures is already good
  enough for the actual users, and nobody needs structured reasons or
  headless decisions.
- Headless sync cannot be made to agree with interactive startup about
  project conditions, so the feature would create two different package
  sets.
- Project conditions cannot be separated from automatic installation, and
  the team is not willing to let project init install packages even with a
  grant.
- The predicate language grows into a general configuration-expression
  language. That is not worth the compatibility and review burden for a
  few platform checks.
- The conditions would become a place where users hide grants or sources
  from review.

## Sources

- `crates/core/src/rhai/setup.rs:1423` — `use_condition`; `:1645` —
  `use_package_spec` condition handling; `:1162` — `forge.system` facts.
- `crates/core/src/collect.rs:13` — collection; `:99` — `finish`; `:125`,
  `:157` — conditional/deferred handling.
- `crates/core/src/declare.rs:91` — `CollectedDeclarations`; `:115` —
  `DeclarationState`; `:135` — `DeferredCode`.
- `crates/editor/src/declare.rs:239` — `static_spec_hash`.
- `crates/core/src/lockfile.rs:168` — `LockFile`; `:406` — restore/prune.
- `crates/core/src/project.rs:28` — canonical project root;
  `crates/editor/src/project.rs:22` — project grant.
- `wit/forge-editor.wit:926` — environment/system facts; `:1753` — plugin
  world.
- `docs/package-manager-spec.md:144` — Phase 8 scope.
