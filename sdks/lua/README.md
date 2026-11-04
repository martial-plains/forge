# Forge plugins in Lua

Write a plugin as Lua scripts; `build.sh` packs them with Lua 5.4 into a
WebAssembly component that Forge loads like any other plugin, with the
same sandbox and permissions. The API has the same namespaces as the user configuration (`forge.command`,
`forge.keymap`, `forge.ui`, `forge.buffer`, `forge.mode`, `forge.on`,
`forge.notify`, `forge.system`, `forge.env`), minus what a plugin can't do
(keymap.set takes command names, not functions).

    ./build.sh examples/hello/plugin.lua -o hello-lua.wasm
    forge --check-plugin hello-lua.wasm --run hello-lua-greet --answer Ada

`examples/` also has vertico, corfu, meow and lsp: the same plugins as
the Rust ones in `plugins/`, written in Lua (each has a `build.sh`).

`forge.toml` next to the first script is the manifest (name, version,
description, permissions), as for the other SDKs.

## What's inside

- `lua/`: Lua 5.4.9, unmodified, without `io`, `os`, `package` and
  `debug` (MIT license, at the end of `lua/lua.h`). `print` goes to
  *Messages*.
- `host.c`: the plugin exports (init, commands, events, answers) and the
  `forge` table.
- `setjmp.c`: Lua raises errors with longjmp. On WebAssembly that needs
  the exception-handling proposal: `build.sh` compiles with
  `-mexception-handling -mllvm -wasm-enable-sjlj`, and this file is the
  small runtime the compiler's lowering calls. Forge's Wasmtime runs Wasm
  exceptions natively.

## Differences from the Rhai configuration

| Rhai config | Lua plugin |
|----------|------------|
| runs in the editor, trusted | runs in a component, sandboxed |
| `forge.keymap.set(keys, function)` | bind command names only |
| `forge.opt`, `forge.plugin` | not available (the user's settings) |
| `forge.env.get(name)` (nil, no prompt) | same, but needs the environment permission; there is no `forge.env.set`/`.substitute`/`.initial`/`.with` (a plugin can't change the environment) |
| `forge.system.*` | available: `type`, `configuration`, `name`, `pid`, `null_device`, `invocation_name` (no `invocation_directory`, `installation_directory`, `load_average`, `tty_erase_char`) |
| `set_decorations` | `buf:set_decorations({{l1, c1, l2, c2, face, message}, ...})` |
| — | `forge.ui.select(items, opts, fn)`, `forge.ui.confirm(prompt, fn)` |
| — | `forge.process.spawn(program, args, { dir, on_output, on_exit })`, `.write`, `.close_input`, `.kill`, `.project_root` (needs the processes permission) |
| — | `forge.buffer.visit(path)` |
| — | `forge.command.create(name, fn, { background = true })`, `forge.on(event, fn, { background = true })` |
| — | `forge.completion.set_minibuffer_ui{...}`, `.set_minibuffer_cycle{...}`, `.set_matching{...}`, `.annotate(category, fn)`, `.set_at_point_ui{...}`, `.source(name, fn(buf, line, col, reply), { modes })` |
| — | `forge.keymap.execute(keys)`, `.is_prefix(keys)`, `.where_is(cmd)`, `forge.ui.read_key(prompt, fn)`, `forge.command.doc(name)`, `forge.mode.set_lighter(mode, text)`, `buf:set_mark(l, c)`, `buf:mark()` |

Positions are the same: lines count from 1, columns from 0.
