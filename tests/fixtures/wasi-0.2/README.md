WASI 0.2.12 interface definitions (io, clocks, filesystem, sockets), copied
unchanged from the `wasmtime-wasi` 48.0.3 crate (`src/p2/wit/deps/`).
License: Apache-2.0 WITH LLVM-exception (the WASI project / Bytecode Alliance).

The plugin runtime tests use them to build components that call real WASI
functions (listing preopened directories, looking up host names), to check
that plugins only get the directories and network they were granted.
