# Plugin state across hot reload

Status: Phase 8 design note. Nothing here is implemented.

## Problem and who has it

"Reload plugin" currently means "unload it and load a fresh instance."
`PluginHost::load_request` calls `self.unload(editor, &name)` and then
instantiates a new component store (`crates/core/src/plugin/host.rs:589`,
`:733`). The old `LoadedPlugin` and its `PluginState` are dropped
(`crates/core/src/plugin/runtime.rs:206`). `reload` and `reload_changed` in
`crates/editor/src/plugins.rs:223` and `:251` both take this path.

For a stateless plugin that is fine. For a plugin that maintains in-memory
state it is expensive or lossy:

- a language server keeps a parse cache, index, open-document set and
  per-server process state;
- an org/notes plugin keeps a search index or a current agenda view;
- a long-running plugin loses in-flight asynchronous work and has to rebuild
  everything after a one-line code change;
- users learn to avoid reload, so broken-state bugs stay hidden.

Forge already has two forms of persistence, but neither is a reload
mechanism:

- the lazy-start `Summary` stores registrations, not plugin memory
  (`crates/core/src/plugin/summary.rs:26`);
- a plugin can write its own files under
  `~/.cache/forge/plugin-data/NAME` (`wit/forge-editor.wit:817`), which is
  appropriate for durable rebuildable data and is explicitly allowed to be
  deleted.

The users are plugin authors (especially language/index plugins), users who
rebuild plugins, and the auto-reload path that watches files.

## Proposed Rhai surface

Keep reload policy in `init.rhai`, because it is user-visible behavior, and
make the state contract opt-in per declaration. The actual serialization is
implemented by the plugin through an optional WIT export; Rhai never sees
the bytes.

```rhai
// Default is fresh: today's behavior.
forge.opt["plugin_state"] = "save-restore";

use_package("lsp", #{
  index: "lsp",
  state: #{
    mode: "save-restore",   // "fresh" is the default
    max_bytes: 1048576,
    ttl_secs: 86400,
    on: ["reload"],         // never on a different binary or a new grant set
  },
});

// Explicitly not stateful, even if the global option changes later.
use_package("spell", #{ index: "spell", state: false });
```

Rules:

- `state: false` or omitted means today's behavior (fresh instance).
- `state: true` is shorthand for `#{ mode: "save-restore" }` with the host
  defaults.
- `mode: "save-restore"` asks the host to call the plugin's optional save
  export before `unload` and its restore export after the new instance's
  `init` has run.
- `on` defaults to `["reload"]`. It must not include `"upgrade"` or
  `"config"` in the first version: a new binary, new grants, a changed
  static declaration or a changed Forge version starts fresh.
- `max_bytes` and `ttl_secs` are host-enforced limits. The host may lower
  them; the plugin must cope with absent or rejected state.
- If the plugin does not export the state functions, `state: true` is a
  load-time error naming the plugin and the missing export, not a silent
  no-op.
- `forge.opt["plugin_state"]` is only a default policy. A per-declaration
  value wins, so a user can exempt a plugin that misbehaves.

Do not expose the bytes to Rhai. A Rhai-level state table would invite
configuration secrets, complicate hashing and make it unclear who owns
serialization.

## Static/closure split

- The `state` policy is static declaration data. It is parsed at
  declaration time and hashed as part of `PluginUse`; the bytes are runtime
  data and are never part of `static_spec_hash`.
- `state` must not accept a closure. A closure that produces state is
  arbitrary code and cannot be reasoned about during collection.
- Reload itself is a host lifecycle event. Do not call Rhai `init` again;
  it already runs as part of the new plugin load. The restore call is a
  plugin export, not a Rhai hook.
- A plugin should restore after its normal `init` has registered commands,
  modes and bindings. If it restores before `init`, it may depend on host
  APIs that are not ready. Save must run after all queued foreground work
  is drained, or the host should refuse to save while the plugin is busy.
- Background work needs special handling. `PluginHost` already tracks
  `away` and queued calls (`crates/core/src/plugin/host.rs:268`, `:780`).
  Reload while a background call is in flight must either wait, cancel and
  document the state loss, or reject the reload; it must not save a
  half-finished snapshot.

## Hashing

- Add `state` policy fields to `PluginUse` and `static_spec_hash`
  (`crates/editor/src/declare.rs:239`). Changing from fresh to save-restore,
  or changing the policy, changes the static hash.
- The state cache key should reuse the lazy-summary identity: the plugin
  component digest, the grants and the Forge version
  (`summary::key` at `crates/core/src/plugin/summary.rs:102`). Add a state
  format/version string so a host change can invalidate old blobs.
- Never hash the state bytes into a lock or snapshot. They are machine-local
  and may contain secrets.
- On restore, verify the cache key and a host-side integrity check; a
  mismatch or a corrupt file starts fresh and logs at info/debug level.

## Lockfile

- Nothing belongs in `forge-lock.toml`. The lock describes reproducible
  package resolution; a plugin's volatile in-memory state is not a package
  and must never make a shared lock machine-specific.
- `package-snapshot-save` and snapshot restore are about plugin artifacts
  and lock entries, not runtime state. They must not copy state blobs.
- The state directory is the right home:
  `~/.local/state/forge/plugin-state/NAME-<key>.bin` or a subdirectory of
  it. Machine-local, ignored by sharing, deletable. Existing grants live
  under the state directory (`crates/core/src/config.rs:98`), so this
  follows the same policy.
- A snapshot taken before a reload still restores the old binary and the
  old lock; the state blob keyed by the old digest simply does not apply to
  the old/new combination unless it matches. Do not try to make snapshots
  carry state.

## WIT changes

This is the one Phase 8 item that normally requires an additive WIT change.
Host-side registration summaries are not enough to recover plugin memory,
and snapshotting the Wasmtime linear memory is not a general solution: it
does not capture host resources, processes, async requests or WASI handles,
and it is not portable across compiler/runtime changes.

Preferred shape:

- Add a new optional world, for example
  `world plugin-with-state { include plugin; export state; }`, with an
  interface containing `save-state: func() -> result<list<u8>, string>` and
  `restore-state: func(bytes: list<u8>) -> result<_, string>`.
- Do not add required exports to `world plugin` at
  `wit/forge-editor.wit:1753`; that would break every existing plugin.
- Follow the existing compatibility rule: additive interfaces/worlds,
  package version bump, history paragraph update. The host detects the
  export by inspecting the component; a plugin without it can still load.
- If the team is unwilling to change WIT, do not build this feature. A
  host-only imitation (restoring the summary, then pretending state was
  preserved) is misleading.

## Security implications

- State can contain secrets: access tokens, document text, search indexes,
  credentials captured from a language server. Store it with owner-only
  permissions (`0600`, directory `0700`), never in the shared config or a
  reproducible bundle, and never log it.
- Invalidate on a new component digest, new grants, a changed static
  declaration, a new Forge version or a changed state schema. Do not let a
  plugin upgrade inherit the previous binary's secrets or permissions.
- Do not restore state saved while the plugin was in the middle of an
  asynchronous or background call. Drain or refuse first.
- Cap size and time. A plugin can produce a huge blob; reject with a clear
  error and start fresh rather than consuming unbounded memory or disk.
- Treat the blob as untrusted input. The plugin validates it; the host
  checks ownership, size, key and file type, and uses atomic writes.
- A restore failure must never leave the plugin half-initialized. Load the
  new instance with a fresh store, then attempt restore; if restore fails,
  unload and retry fresh (or keep the fresh instance) and report it.
- State is not a substitute for explicit user data. Do not use it for
  durable notes, undo history, grants or package configuration.
- Cross-user or cross-machine restore is out of scope. Never sync state
  through dotfiles.

## Effort

**L.** The host lifecycle, WIT world, SDK support, keying, limits, atomic
storage, background-call interaction and failure tests are all nontrivial.
The Rhai surface is small; the state contract is the work. It should follow
the WIT compatibility/versioning work and the background-call model.

## What would make this not worth building

- Most plugins can persist the state they actually need in their granted
  cache directory and rebuild the rest cheaply. The existing
  `plugin-data/NAME` directory already does this.
- Lazy summaries and the Wasmtime AOT cache already make reload cheap enough
  that preserving memory is not worth a WIT contract.
- The team cannot make state invalidation safe for secrets, or cannot handle
  reload during background/async work.
- A WIT version bump for optional state would break the current
  compatibility promise or the supported SDKs.
- Plugin authors do not implement the optional exports, so the feature is a
  policy surface with no users.
- The design drifts into serializing Wasmtime memory or host resources,
  which is unsafe and not portable.

## Sources

- `crates/core/src/plugin/host.rs:238` — `cleanup_plugin`; `:268` —
  `PluginHost`; `:560` — `activate`; `:589` — `load_request` with
  `unload`; `:733` — `unload`; `:780` — call/background handling.
- `crates/core/src/plugin/runtime.rs:206` — `PluginState`; `:170` —
  `LoadedPlugin` store ownership.
- `crates/core/src/plugin/summary.rs:26` — `Summary`; `:96` — `digest`;
  `:102` — cache key including version/grants/digest.
- `crates/editor/src/plugins.rs:223` — `reload`; `:251` — `reload_changed`.
- `crates/core/src/config.rs:98` — state directory.
- `wit/forge-editor.wit:817` — plugin cache directory; `:1735` —
  `handler`; `:1753` — `plugin` world; `:1806` — `plugin-full` world.
- `docs/package-manager-spec.md:144` — Phase 8 scope.
