#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build the plugin: ./build.sh [-o OUT.wasm]   (default: vertico.wasm here)
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
[ $# -gt 0 ] || set -- -o "$HERE/vertico.wasm"
exec "$HERE/../../build.sh" "$HERE/vertico.lua" --manifest "$HERE/forge.toml" "$@"
