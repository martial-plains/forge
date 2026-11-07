# Forge package-manager ground rules

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
