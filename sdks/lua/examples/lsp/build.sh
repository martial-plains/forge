#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build the LSP plugin: ./build.sh [-o OUT.wasm]   (default: lsp.wasm here)
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
[ $# -gt 0 ] || set -- -o "$HERE/lsp.wasm"
exec "$HERE/../../build.sh" "$HERE/json.lua" "$HERE/lsp.lua" --manifest "$HERE/forge.toml" "$@"
