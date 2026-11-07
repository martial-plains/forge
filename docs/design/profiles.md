# Profiles

Status: Phase 8 design note. Nothing here is implemented.

## Problem and who has it

Forge configuration is one `init.rhai` file. A person who uses the same
dotfiles on a work laptop, a personal machine and a CI runner has no
first-class way to say "these packages belong to work" or "this machine is
minimal". Today the workarounds are:

- comment blocks in and out by hand;
- split configuration into several files and load them conditionally;
- use `when` with a closure, which collection cannot evaluate;
- give each machine a different `init.rhai`.

Those workarounds make the common case invisible to `use_package_report()`,
`package declarations`, `package sync`, `package doctor` and the lockfile.
`disabled` and `when` are per-declaration; they are not a named, reusable
group, and a headless CI run cannot ask a user which declarations are meant
for this machine.

The main users are people with more than one machine, maintainers of shared
dotfiles, and CI jobs that want a small package set without maintaining a
second configuration file. Plugin authors also benefit indirectly because
their users can keep a consistent set of related plugins.

The current declaration model is in `crates/core/src/declare.rs:20`
(`PluginUse`), collection is in `crates/core/src/collect.rs`, and the
static/closure split is described by `DeclarationState` and
`DeferredCode` at `crates/core/src/declare.rs:115` and
`crates/core/src/declare.rs:135`.

## Proposed Rhai surface

Use a small, explicit selection call plus a static `profiles` key on a
declaration. Omitted `profiles` means "all profiles"; a list means "only
when one of these is active"; an empty list means "never" (useful while
keeping a declaration around).

```rhai
// A literal pins the profile. `forge.profile()` with no argument would let
// the host choose from --config-profile or FORGE_PROFILE, else "default".
forge.profile("personal");

use_package("lsp", #{ index: "lsp", profiles: ["personal", "work"] });
use_package("org", #{ index: "org", profiles: ["personal"] });
use_package("jira", #{ index: "jira", profiles: ["work"] });

// No profiles key: every profile, including the unnamed legacy case.
use_package("spell", #{ index: "spell" });

// Empty list: keep the declaration and its review visible, but never install or load it.
use_package("legacy", #{ index: "legacy", profiles: [] });
```

Rules:

- `forge.profile("name")` or `forge.profile("a", "b")` may be called at most
  once in a configuration run. Calling it twice is an error naming both
  locations, rather than silently merging profiles.
- `forge.profile()` (no arguments) asks the host for the active profile:
  `--config-profile NAME` first, then `FORGE_PROFILE`, then `"default"`.
  A literal call pins the profile and ignores those inputs.
- If `forge.profile` is never called, all declarations are active. This
  preserves every existing configuration.
- Names are restricted to `[a-z0-9_-]+`, are at most 64 bytes, and are not
  paths. Profile names may appear in reports and logs.
- `profiles` is a list of names, not a closure and not a function. It is a
  static key.
- **One package name, one declaration.** Two declarations with the same
  `name` and different profile sets or different static data are rejected.
  This avoids a lock entry whose source or config changes with the profile.
- Profiles select declarations only. They do not select arbitrary Rhai code
  blocks; users who need that already have `if` in Rhai.

## Static/closure split

`profiles` belongs on the static side of the split. It is parsed by the
`use_package` path before any condition closure is evaluated. A
declaration whose profile is inactive is skipped with a reason, not marked
`Conditional`: the host can decide it without running user code.

Collection must add `SkipReason::ProfileInactive(String)` (or an
equivalent structured reason). The JSON report from
`forge package declarations` should say which profile was active and why a
declaration was skipped. `package doctor` should list declarations that
are invisible under the selected profile so users are not surprised by a
plugin that "disappeared".

`forge.profile` and `profiles` must be rejected in a project's
`.forge/init.rhai`. A project is code from a directory the user may have
just cloned; it must not be able to switch the global profile, add
declarations that belong to another profile, or change what gets installed.
The current project trust boundary is documented in
`crates/editor/src/project.rs` and `crates/core/src/project.rs`.

## Hashing

- Add `profiles: Option<Vec<String>>` to `PluginUse`.
- Add the normalized profile list to `static_spec_hash` in
  `crates/editor/src/declare.rs:239`, with a stable representation (sorted
  names, missing key distinct from empty list).
- Do **not** hash the active profile. It is host input, not part of the
  package's identity, and putting it in the static hash would make the same
  declaration churn between machines.
- A profile change that changes a declaration's membership changes its
  `spec_hash`, so `package doctor`, `package lockfile diff` and update review
  can show it.

## Lockfile

The lockfile is keyed by package name (`crates/core/src/lockfile.rs:168`),
and restore replaces the shared lock and prunes package-managed components
that are absent from the restored lock. Profiles must not make a normal
profile switch destructive.

Recommended behavior:

- Keep one union lock for all profiles. Add an optional `profiles` field to
  `LockEntry` (kebab-case, skipped when it means "all"). It records the
  declaration's profile set, never the active profile.
- `sync --config-profile work` records or reuses entries for active
  declarations and **preserves entries whose profile sets do not intersect
  the active profile**. It must never replace the whole shared lock with the
  active subset.
- `restore` remains byte-exact for every entry in the lock. It does not
  evaluate profiles. A future `restore --config-profile NAME` can filter,
  but the default must not delete another profile's packages.
- No `lock_version` bump is needed for an additive optional field; unknown
  fields already survive rewrites. The field is declaration metadata, not
  machine state, so it belongs in the shared lock beside `spec_hash`.
- Snapshots inherit the same field. A snapshot taken from a union lock
  restores the union.

If union locks turn out to be too surprising, the fallback is to treat the
lock as a resolution cache and never prune on profile changes. That is a
policy decision; it should not be inferred from the current profile.

## WIT changes

None. Profiles are host-side declaration filtering and do not change the
plugin/editor contract in `wit/forge-editor.wit`.

## Security implications

- A profile must not hide a permission grant or a source URL from review.
  `package declarations --json`, `package doctor` and the lock diff should
  still show inactive declarations and say why they are inactive.
- `forge.profile` must not be available in untrusted project init. A project
  must not be able to enable a work-only package, install from a new URL, or
  change grants by selecting a profile.
- Profile names are identifiers, not paths. Do not use a profile name to
  build a filesystem path without validation.
- Do not read a profile from an environment variable inside a condition
  closure. Use the host-selected `forge.profile()` path so collection and
  headless runs can agree.
- A profile is not a sandbox. It only selects declarations; grants, build
  policy and permission prompts still apply to whatever is selected.
- Keep the current rule that `forge-lock.local.toml` is machine-local. Do
  not turn a profile into a way to write machine-specific sources into the
  shared lock.

## Effort

**M.** The filtering, report reason and Rhai surface are small; the lock
union/preserve behavior, duplicate-name rejection and headless tests are
the real work. It should be done after the condition-evaluation plumbing
in [conditional declarations](conditional-declarations.md) if both are
built, so there is one place that decides "this declaration is inactive
for a host reason".

## What would make this not worth building

- Users already split configurations into files and use `when`; profile
  switching is not actually common enough to justify lock-policy work.
- The union-lock rule cannot be made non-destructive without surprising
  restores, and the team is not willing to accept a "lock is a cache"
  policy.
- Profile selection has to be available in project init to be useful, which
  would break the project trust boundary.
- The feature grows into "run a different init.rhai per profile", which is
  a larger configuration-loading redesign rather than a declaration filter.
- The package manager needs duplicated names with different sources per
  profile. That is a different feature and should be designed separately.

## Sources

- `crates/core/src/declare.rs:20` — `PluginUse`; `:115` — `DeclarationState`;
  `:135` — `DeferredCode`.
- `crates/core/src/rhai/setup.rs:1351` — `use_package` key list; `:1578` —
  `use_package_spec`.
- `crates/core/src/rhai/mod.rs:1368` — `use_spec` key list.
- `crates/core/src/collect.rs:13` — headless collection; `:99` — `finish`;
  `:125`, `:157` — conditional and skipped states.
- `crates/editor/src/declare.rs:239` — `static_spec_hash`.
- `crates/core/src/lockfile.rs:168` — `LockFile`; `:406` — snapshot restore
  and prune behavior.
- `crates/core/src/project.rs:28`, `crates/editor/src/project.rs:22`,
  `crates/editor/src/project.rs:81` — project trust and execution.
- `docs/package-manager-spec.md:144` — Phase 8 scope.
