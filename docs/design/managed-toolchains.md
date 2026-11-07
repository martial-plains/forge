# Managed toolchains

Status: Phase 8 design note. Nothing here is implemented. The current
package-manager spec explicitly leaves managed toolchains out of scope
until later (`docs/package-manager-spec.md:122`); this note describes what
"later" could mean.

## Problem and who has it

A source package is built with whatever toolchain is on the host. Forge
records the *text* of `rust-toolchain.toml` or `rust-toolchain` in
`BuildRecipe::toolchain` (`crates/core/src/package.rs:871`, `:889`), but it
does not resolve that file to an installed compiler version. Two machines
with the same checkout can therefore build different bytes, and a machine
without the requested channel fails at build time with a toolchain error
from the build command rather than a clear package-manager error.

The users are plugin authors who publish source packages, users who build
from Git on a fresh machine, and CI maintainers who need reproducibility.
A plugin author can pin a toolchain in the repository, but Forge does not
manage it; the user has to install rustup/the channel first.

This is not the same as pinning a plugin version. The component artifact
depends on the compiler, target, features and build command. The build
audit already notes that recipe consent is incomplete and builds still run
in the checkout with host access (`docs/package-manager-audit.md:88`).

## Proposed Rhai surface

Add a static `toolchain` key for source declarations. Start with Rust and
rustup, because the default build target is
`wasm32-wasip2` (`crates/core/src/package.rs:885`) and Cargo is the common
source build. Do not pretend to manage every language in the first version.

```rhai
// Use the repository's rust-toolchain.toml. If it is missing, fail clearly.
use_package("lsp", #{
  git: "https://github.com/acme/forge-lsp",
  rev: "v1.2.0",
  toolchain: #{
    source: "file",          // "file" | "channel"
    install: "ask",          // "ask" | "never" | "always"
    components: ["rustfmt"],
  },
});

// Pin a channel without a repository toolchain file.
use_package("helper", #{
  git: "https://github.com/acme/forge-helper",
  toolchain: #{ kind: "rust", channel: "1.85.0", install: "ask" },
});
```

Suggested rules:

- `kind` defaults to `"rust"`. Unsupported kinds fail with a clear message;
  do not silently use the host toolchain.
- `source: "file"` reads `rust-toolchain.toml`/`rust-toolchain` from the
  checkout, as `BuildRecipe::for_checkout` does today. If the file requests
  a channel that is not installed, use `install` policy.
- `source: "channel"` takes an explicit `channel`: a release (`"1.85.0"`),
  `"stable"`, `"beta"` or a dated `"nightly-YYYY-MM-DD"`. Avoid floating
  `"nightly"` in a reproducible recipe; allow it only with a warning.
- `install` defaults to `"ask"`. `"ask"` prompts with the exact channel,
  target and components before downloading; `"never"` fails if the
  toolchain is missing; `"always"` may install without a prompt only in an
  explicitly non-interactive/trusted context. Headless sync should default
  to `"never"` unless the user passes an explicit `--allow-toolchain-install`.
- `components` and `targets` are validated allowlists, not arbitrary rustup
  commands. The target remains `wasm32-wasip2` for the plugin build.
- `toolchain` is only meaningful for source builds (`git`/`vc`). It is an
  error on an `index` or `url` declaration that only installs a prebuilt
  component, because nothing is built.
- A repository toolchain file is untrusted input. It may request a channel,
  but it cannot request a custom distribution URL, a shell command or an
  environment variable. Those are user/index policy, not project content.
- Do not add a general "run this command before building" escape hatch.
  That is what `build` already is, and it is separate from toolchain
  management.

The host-side command surface can be small: `M-x package-toolchain-install`
for the selected declaration, and `forge package toolchain list/install` in
the headless CLI. The Rhai declaration remains the policy source.

## Static/closure split

- `toolchain` is static data. It is parsed before any build and has no
  closure form.
- Resolution happens after checkout and before the build command runs. It
  must not run Rhai `config`/`init` closures.
- The declaration is hashed as data; the resolved compiler identity is
  recorded separately in the build recipe and lock.
- A `toolchain` value that depends on the current environment or host is not
  allowed; that would make the static hash and CI behavior non-deterministic.
  Use an explicit channel or the repository file.

## Hashing

Current state: `BuildRecipe::toolchain` stores the raw file text and
`BuildRecipe::hash` includes command, raw toolchain text, features and target
(`crates/core/src/package.rs:904`). That is better than nothing but does not
prove which compiler ran.

Proposed:

- Add `toolchain` declaration data to `PluginUse` and `static_spec_hash`
  (`crates/editor/src/declare.rs:239`).
- Extend `BuildRecipe` additively with resolved fields, for example
  `toolchain_kind`, `toolchain_channel`, `toolchain_version`,
  `toolchain_target`, `toolchain_components` and a `toolchain_source`.
  Preserve `extra` so old/new lockfiles round-trip.
- Include the resolved identity in a *versioned* recipe hash. A raw file
  text hash is not enough; `stable` can move and a dated nightly should be
  recorded as the resolved version as well as the requested channel.
- Do not silently change existing `recipe_hash` values without a migration
  story. Old locks must still load; a changed hash should appear as a
  recipe diff and make `package doctor`/update review explain that the
  recipe identity is now more precise.
- The build cache key already needs `(commit, recipe_hash, toolchain,
  target)` per the spec (`docs/package-manager-spec.md:120`). The resolved
  toolchain string is that key's toolchain component.
- Do not include installer URLs, mirror credentials or a full environment
  dump in the hash or lock.

## Lockfile

- No new top-level lock format is required. `LockEntry` already carries
  `recipe` and `recipe_hash` (`crates/core/src/lockfile.rs:31`), and
  `BuildRecipe.extra` preserves additive fields.
- Record the resolved toolchain in the recipe, not as a separate mutable
  machine setting. Two machines that resolve the same channel to different
  compiler versions should produce a visible recipe diff, not silently
  reuse the wrong build cache entry.
- A lock that requested `stable` but recorded a resolved version should
  still be usable offline. `sync` must not need to resolve `stable` again
  if the locked artifact and recipe are present; an explicit upgrade may
  move the resolution.
- Snapshots inherit the recipe. Restoring a snapshot does not install a
  toolchain; it restores components. A later rebuild uses the recorded
  recipe and either finds the exact toolchain or asks/fails per policy.
- Machine-local toolchain installation paths are not part of the shared
  lock. Store the install location in the host's cache/state, not in
  `forge-lock.toml`.

## WIT changes

None. This is a build/package-manager feature. It changes the build
environment and recipe metadata, not the plugin/editor contract in
`wit/forge-editor.wit`.

## Security implications

- Installing a toolchain downloads and executes a compiler. It is a
  supply-chain operation, not a convenience. Use the existing rustup
  distribution and signature/checksum mechanisms or an explicitly allowed
  mirror; do not implement a "curl | sh" path.
- Restrict channels and components to a known allowlist. Reject a
  repository toolchain file that tries to set a custom distribution URL,
  proxy credential or arbitrary install command.
- Require consent for a new channel, a floating channel that resolves to a
  new version, a new component/target and a toolchain change in an upgrade.
  Headless/CI needs an explicit opt-in flag; the safe default is to fail
  with an actionable message.
- A project's `.forge/init.rhai` must not be able to install toolchains or
  add a source declaration automatically. Keep project code and package
  installation separate, as with the existing project grant.
- Build environment scrubbing still applies. Do not pass tokens,
  `SSH_AUTH_SOCK`, credentials or the full process environment to rustup or
  the build. Use the `BuildPolicy` allowlist and add only required
  toolchain variables.
- Managed toolchains consume disk and can be large. Cap installations,
  expose a cleanup path, and do not leave untrusted project-requested
  versions behind.
- Verify the compiler that actually runs. A `rust-toolchain.toml` can be
  overridden by environment or a shim; set the toolchain explicitly for the
  build and record the resolved version.
- Cross-platform support matters (Linux, macOS, Windows). Do not assume
  rustup paths or a POSIX shell for installation. Unsupported platforms
  should fail with a clear message rather than silently using the host
  toolchain when the user asked for a managed one.

## Effort

**L.** The initial "resolve a rustup channel and run the existing build with
a fixed toolchain" path is M. Making it trustworthy (consent, provenance,
versioned recipe hashes, cache keying, headless policy, cleanup,
cross-platform install) is L and depends on Phase 5 build hardening and the
Phase 6 supply-chain work. It should not be attempted before staged/offline
builds and environment scrubbing are in place.

## What would make this not worth building

- rustup already respects `rust-toolchain.toml` on every supported machine,
  and the real problem is just a better error message when the toolchain is
  missing. That is a small doctor/build improvement, not a manager.
- Forge only builds Rust plugins and the team is comfortable requiring
  rustup as a prerequisite.
- There is no trustworthy way to verify/download toolchains on all
  supported platforms, or the feature would have to execute arbitrary
  installer code.
- Phase 5 is still incomplete; managing a toolchain without staged builds,
  an offline build step and a reproducible-build story adds risk without
  reproducibility.
- "Managed toolchains" expands to Python, Node, Go and system package
  managers. That is a general environment manager and not a plugin package
  manager feature.
- A floating channel would silently change artifacts. If only exact,
  immutable channels are acceptable, the feature is much smaller and should
  be scoped to those.

## Sources

- `crates/core/src/package.rs:871` — `BuildRecipe`; `:875` — `toolchain`
  field; `:885` — `BUILD_TARGET`; `:889` — `for_checkout`; `:904` —
  `hash`; `:917` — `diff`.
- `crates/editor/src/declare.rs:149` — lock entry; `:239` —
  `static_spec_hash`; `:270` — `recipe_for`.
- `crates/core/src/lockfile.rs:31` — `LockEntry`; `:168` — `LockFile`.
- `crates/core/src/package.rs` — `BuildPolicy` and default build environment.
- `docs/package-manager-spec.md:120`, `:122`, `:144` — build cache key,
  toolchain file and the "managed toolchains are out of scope for now"
  note.
- `docs/package-manager-audit.md:75`, `:88` — recipe consent and build
  hardening status.
- `crates/editor/src/project.rs:22` — project-code grant boundary.
