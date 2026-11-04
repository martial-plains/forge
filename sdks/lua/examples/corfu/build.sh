#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build the plugin: ./build.sh [-o OUT.wasm]   (default: corfu.wasm here)
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
[ $# -gt 0 ] || set -- -o "$HERE/corfu.wasm"
exec "$HERE/../../build.sh" "$HERE/corfu.lua" --manifest "$HERE/forge.toml" "$@"
