# Forge plugins in C

```c
#include "forge.h"

static void got_name(const char *name, void *data) {
    if (name) forge_insert_formatted("Hello, %s!", name);
}

static bool greet(const int64_t *prefix) {
    forge_read_string("Name: ", "", got_name, NULL);
    return true;
}

bool forge_plugin_init(void) {
    return forge_command("greet", "Ask for a name and insert a greeting.", greet);
}
```

    ./build.sh greet.c -o greet.wasm
    forge --check-plugin greet.wasm --run greet --answer Ada

`build.sh` needs `wit-bindgen` and `wasm-tools` (`cargo install
wit-bindgen-cli wasm-tools`) and a WebAssembly C compiler: wasi-sdk
(`WASI_SDK_PATH`) or zig (`brew install zig`, `pip install ziglang`).
It generates the raw bindings (`plugin.h`) from the WIT, compiles your
files with [forge.c](forge.c), makes a component, and embeds
`forge.toml` from next to your sources as the manifest:

```toml
[package]
name = "greet"
version = "0.1.0"
description = "Greets you"
```

[forge.h](forge.h) has commands, questions with callbacks, events,
printf-style messages and logging, and the current buffer's text; the
raw API is all of `plugin.h` (`forge_editor_buffers_...`). Functions that
can fail return false and `forge_error()` says why; a command returns
false after `forge_fail("...")` to report an error.

The core module is wasm32-wasip1, adapted to WASI 0.2 with the bundled
Wasmtime 48 adapter (`wasi_snapshot_preview1.reactor.wasm`, Apache-2.0
WITH LLVM-exception). A plugin that uses no stdio imports no WASI at
all. C++ works the same way (`wit-bindgen cpp` for a C++ API).
