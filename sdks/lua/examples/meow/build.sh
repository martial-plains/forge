#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build the plugin: ./build.sh [-o OUT.wasm]   (default: meow.wasm here)
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
[ $# -gt 0 ] || set -- -o "$HERE/meow.wasm"
exec "$HERE/../../build.sh" "$HERE/meow.lua" --manifest "$HERE/forge.toml" "$@"
