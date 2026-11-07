# Plugin-recommended config bundles

Status: Phase 8 design note. Nothing here is implemented.

## Problem and who has it

A plugin manifest can declare settings with `[[settings]]` and defaults
(`crates/core/src/permissions.rs:606`), and the host validates the JSON
object set by `forge.plugin.config` before load (`Manifest::check_config`,
`crates/core/src/permissions.rs:802`). What is missing is a *named,
recommended combination* of settings.

Plugin authors today document "set these six keys for the best experience"
in a README. Users copy them by hand, sometimes with a typo, and there is no
way to tell whether a plugin upgrade changed its recommended values. A user
who installs several related plugins has to read several READMEs to get a
coherent setup.

The users are primarily plugin authors and new users. A plugin author can
say "start with the recommended bundle" and a user can adopt it without
knowing every setting. Existing users keep their explicit configuration.

The relevant existing pieces are:

- `Setting`, `SettingKind` and `Manifest::settings` in
  `crates/core/src/permissions.rs:563`, `:606` and `:732`;
- `check_config` at `crates/core/src/permissions.rs:802`;
- `config` in a declaration, converted to JSON and stored in
  `PluginUse::config` at `crates/core/src/rhai/mod.rs:1368`;
- `Manifest::for_component` and the embedded/sidecar manifest path in
  `crates/core/src/permissions.rs`.

## Proposed Rhai surface

Make the bundle a static declaration key. The bundle's values come from the
plugin's manifest, so the declaration only names the bundle. Explicit
`config` wins over the bundle; the manifest's setting default is the base.

```rhai
// Use the plugin's "recommended" bundle; config below overrides two keys.
use_package("org", #{
  index: "org",
  config_bundle: "recommended",
  config: #{ agenda_files: ["~/org"], startup: false },
});

use_package("lsp", #{
  index: "lsp",
  config_bundle: "minimal",
  config: #{ servers: #{ rust: true } },
});

// No bundle: keep existing behavior (manifest defaults plus config).
use_package("spell", #{ index: "spell" });
```

Proposed manifest shape, extending the existing sidecar/embedded TOML
manifest additively:

```toml
[[settings]]
name = "progress"
kind = "boolean"
default = false

[[settings]]
name = "style"
kind = "string"
default = "plain"

[bundles.recommended]
progress = true
style = "compact"

[bundles.minimal]
progress = false
```

Rules:

- A bundle may set only names declared in that plugin's `[[settings]]`, and
  every value must pass the declared `SettingKind`. Unknown names and wrong
  kinds are refused before the plugin starts, naming the plugin, bundle and
  key.
- Precedence is: explicit `config` map > selected bundle > manifest
  `default`. A `config` function is still deferred and runs at load; if it
  writes configuration, it has the final say, but the bundle remains the
  static base.
- `config_bundle` is a single string. Selecting two bundles is deliberately
  not supported until there is a clear merge order; users can copy values
  or choose one.
- A missing bundle is an error, not a silent fallback to defaults. List the
  available bundle names in the error.
- A bundle cannot grant permissions, add directories, change the build
  command, add sources, or install anything. It only sets declared settings.
- A plugin with no `[[settings]]` does not need bundles; `config_bundle` is
  still an error if the manifest declares none. This preserves the current
  "no settings accepts anything" behavior for plain `config`.
- Bundles are part of the plugin's manifest, not a new package format. The
  sidecar `NAME.toml` override still wins over the embedded manifest, so a
  user can inspect or override a bundle without rebuilding.

The Rhai surface is intentionally one static key. It composes with
[profiles](profiles.md) and [conditional declarations](conditional-declarations.md)
but does not depend on them.

## Static/closure split

- `config_bundle` is a static key. It belongs in the `use_package` key list
  in `crates/core/src/rhai/setup.rs:1351` and in the `use_spec` key list at
  `crates/core/src/rhai/mod.rs:1368`, because `forge.plugin.declare` uses
  the latter.
- The bundle is resolved from the manifest, not by running Rhai. It has no
  closures and must not become a script hook.
- A `config` map remains static and hashed today. A `config` closure remains
  deferred and is recorded as `DeferredCode::Config`. The bundle must not
  make collection execute a config function just to find the base values.
- If a bundle and a `config` closure set the same key, the closure can win
  at load. That is the existing semantics of `forge.plugin.config`; document
  it rather than trying to freeze closure output.

## Hashing

- Add `config_bundle` to `PluginUse` and to `static_spec_hash`
  (`crates/editor/src/declare.rs:239`). A change from no bundle to
  `"recommended"`, or between bundle names, must change the static hash.
- The bundle contents live in the plugin manifest, not in the declaration.
  For a source plugin the component/recipe hash already changes when the
  plugin changes. For a prebuilt plugin the artifact hash changes too. That
  catches a new bundle in a new binary, but not a sidecar manifest edit.
- Recommended additional lock metadata: an optional `config_bundle` name
  and a `config_hash` over the *effective* settings after bundle + explicit
  config. This lets update review and `package doctor` show "the bundle
  changed" without pretending the settings are part of the artifact.
  Additive optional fields need no `lock_version` bump.
- Do not put secret setting values in a bundle or in a lock hash that could
  be used to brute-force a low-entropy secret. Bundles should be defaults,
  not credentials. If an app needs a token, it belongs in the environment or
  a granted file, not in a manifest bundle.

## Lockfile

- No new package source or artifact field is required. `spec_hash` will
  include the bundle name. If the optional `config_hash` is added, it
  records the effective non-secret settings so a change is visible.
- Existing `LockEntry` has `settings` and an extensible `extra` map
  (`crates/core/src/lockfile.rs:31`). A bundle name can be stored there
  additively, but a first-class field is preferable for `diff` output.
- Snapshots inherit the lock entry and therefore the bundle metadata. The
  bundle values themselves are not copied into the snapshot; the plugin
  artifact and manifest remain the source of truth.
- Restore should not need the bundle at all if the artifact and manifest are
  unchanged. If a sidecar manifest is missing or edited, restore behavior is
  unchanged from today: the installed component and metadata are restored,
  and load-time validation reports the mismatch.

## WIT changes

None. This is a declaration key plus manifest parsing and validation. The
plugin runtime still receives its settings through the existing
`forge.plugin.config` interface in `wit/forge-editor.wit:902`.

## Security implications

- A bundle is untrusted plugin data. It must be validated against declared
  settings and must never be able to set `grant`, `network`, `processes`,
  `directories`, `memory-mb`, `build`, source URLs, hooks or commands.
- The user's explicit `config` must win. A bundle must not override a
  user's deliberate value, especially a value that limits behavior.
- Bundles can change on a plugin upgrade. Show the effective config diff in
  update review, not just the new bundle name, and require the normal
  update consent. A silent change to a security-relevant setting (for
  example an HTTP endpoint or a directory) is not acceptable.
- Sidecar manifests can be edited without changing the `.wasm` artifact.
  `package doctor` should report a manifest/config change that is not
  reflected in the lock, or at least show the effective config when it
  differs from the locked `config_hash`.
- Do not let a bundle set a path outside granted directories or a command
  that will be executed. Bundles are settings only.
- Keep bundle names and keys out of logs if they might contain sensitive
  values; log the name and the key list, not the values.

## Effort

**M.** Manifest parsing, validation, precedence, the new declaration key,
hash integration, lock diff and tests are all moderate. There is no WIT
change and no new lifecycle. If the team skips `config_hash` and lock diff,
the first version is closer to **S**; the estimated M includes making
upgrade review trustworthy.

## What would make this not worth building

- `[[settings]]` defaults plus a few lines of documented `config` already
  solve the real onboarding problem, and almost no plugins would publish
  bundles.
- The precedence rules produce surprising overrides, especially when a
  config closure is involved.
- Bundles become a way to ship hidden or security-relevant defaults, or a
  way to bypass the normal permission and update review.
- The manifest format cannot be extended without breaking unknown-field
  handling or the sidecar/embedded override rules.
- A bundle is expected to install other plugins or compose across plugins.
  That is a package-set feature, not a settings preset, and should not be
  smuggled into this key.

## Sources

- `crates/core/src/permissions.rs:563` — `SettingKind`; `:606` — `Setting`;
  `:732` — manifest settings; `:802` — `check_config`; `:869` —
  `Manifest::for_component`.
- `crates/core/src/rhai/setup.rs:1351` — `USE_PACKAGE_KEYS`.
- `crates/core/src/rhai/mod.rs:1368` — `use_spec` key list and `config`.
- `crates/core/src/declare.rs:20` — `PluginUse::config`.
- `crates/editor/src/declare.rs:149` — `lock_entry`; `:239` —
  `static_spec_hash`.
- `crates/core/src/lockfile.rs:31` — `LockEntry` and extensible fields.
- `wit/forge-editor.wit:902` — `config` interface.
- `docs/plugin-api.md` — existing settings/config documentation.
- `docs/package-manager-spec.md:144` — Phase 8 scope.
