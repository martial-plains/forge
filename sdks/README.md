# Forge plugin SDKs

A Forge plugin is a WebAssembly component for the `forge:editor/plugin`
world in [`wit/forge-editor.wit`](../wit/forge-editor.wit). Any language
whose toolchain makes components can write one; these SDKs add the same
small layer in each language: register a command with a function, ask a
question with a callback, subscribe to events with a function.

| Language | Where | Toolchain | Plugin size | First load* |
|----------|-------|-----------|-------------|-------------|
| Rust (reference) | [`crates/plugin-sdk`](../crates/plugin-sdk), example in [`plugins/hello`](../plugins/hello) | `cargo build --target wasm32-wasip2` | small | fast |
| JavaScript / TypeScript | [`sdks/js`](js) | ComponentizeJS (npm) | ~12 MB | ~8 s |
| Python | [`sdks/python`](python) | componentize-py (pip) | ~18 MB | ~7 s |
| C / C++ | [`sdks/c`](c) | wit-bindgen c + zig or wasi-sdk | ~30 KB | fast |
| Lua | [`sdks/lua`](lua) | wit-bindgen c + zig or wasi-sdk (Lua 5.4 is bundled) | ~350 KB | ~4 s debug, <1 s release |
| Go | not yet | TinyGo or Go 1.25 + `wit-bindgen go` | | |

\* Compiling to machine code, once: Forge caches the result, so later
starts take ~0.1 s, and an unchanged plugin isn't even started until it's
used (see "Loading" in [docs/plugin-api.md](../docs/plugin-api.md)).
JavaScript and Python plugins carry their language's engine, hence the
size.

All of them build WASI 0.2 components: the world has no `async`
functions, and 0.2 is what every toolchain supports today. Forge loads
WASI 0.2 and 0.3 plugins alike.

## Versions

Each SDK has its own version and says which API it targets (0.1.5 here:
`forgeWit` in package.json, the WIT it ships). Running programs
(processes) is in the Rust, JavaScript and Lua SDKs; background commands
are in the Rust, JavaScript, Lua and C SDKs. Python has neither yet.
Completion sources and annotations are in the Rust and Lua SDKs. The
vertico, corfu, meow and lsp plugins are written in Rust (separate `forge-plugin-NAME` repositories) and
again in Lua (`sdks/lua/examples/`); `sdks/test.sh favourites` checks
both, through the whole editor (`forge --headless`). A plugin built against any
0.1.x keeps loading in later Forge 0.1.x releases.

## Trying a plugin without the editor

    forge --check-plugin plugin.wasm --run COMMAND --answer "for questions" --text "buffer text"

loads it, prints what it registered (commands, keys, modes, events,
permissions), runs commands and prints the buffer afterwards. The exit
status says whether everything worked.

## Tests

    cargo build --profile fast     # optimized Forge that builds quickly
    sdks/test.sh                   # every language with a toolchain installed
    sdks/test.sh js python         # some

builds each SDK's example and checks it in Forge.
