#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build a Forge plugin written in C:
#
#   ./build.sh plugin.c [more.c ...] [-o plugin.wasm] [--manifest forge.toml]
#
# The manifest (name, version, description, permissions: see
# docs/plugin-api.md) is embedded in the component as its forge-manifest
# section. By default it's forge.toml next to the first source file.
#
# Needs wit-bindgen and wasm-tools (cargo install wit-bindgen-cli
# wasm-tools) and a C compiler for WebAssembly, whichever is found first:
#   - wasi-sdk:  WASI_SDK_PATH=/opt/wasi-sdk
#   - zig:       brew install zig, or pip install ziglang
#
# The core module is wasm32-wasip1 (what C toolchains make today); the
# bundled adapter (wasi_snapshot_preview1.reactor.wasm, from Wasmtime 48)
# turns it into a WASI 0.2 component. A plugin that doesn't call stdio
# or other WASI functions ends up importing no WASI at all.
set -eu

SDK=$(cd "$(dirname "$0")" && pwd)
OUT=
SOURCES=
MANIFEST=
while [ $# -gt 0 ]; do
    case "$1" in
        -o) OUT=$2; shift 2 ;;
        --manifest) MANIFEST=$2; shift 2 ;;
        *) SOURCES="$SOURCES $1"; shift ;;
    esac
done
[ -n "$SOURCES" ] || { echo "usage: build.sh FILE.c... -o OUT.wasm" >&2; exit 2; }
[ -n "$OUT" ] || OUT=plugin.wasm
if [ -z "$MANIFEST" ]; then
    set -- $SOURCES
    MANIFEST="$(dirname "$1")/forge.toml"
fi

if [ -n "${WASI_SDK_PATH:-}" ]; then
    CC="$WASI_SDK_PATH/bin/clang --target=wasm32-wasip1"
elif command -v zig >/dev/null 2>&1; then
    CC="zig cc -target wasm32-wasi"
elif python3 -c "import ziglang" 2>/dev/null; then
    CC="python3 -m ziglang cc -target wasm32-wasi"
else
    echo "build.sh: no C compiler for WebAssembly: set WASI_SDK_PATH or install zig" >&2
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
wit-bindgen c "$SDK/wit" --world plugin --out-dir "$TMP" >"$TMP/log" 2>&1 || { cat "$TMP/log" >&2; exit 1; }
# shellcheck disable=SC2086
$CC -O2 -g0 -Wl,--strip-debug -mexec-model=reactor -Wall -I"$TMP" -I"$SDK" $SOURCES "$SDK/forge.c" "$TMP/plugin.c" \
    "$TMP/plugin_component_type.o" -o "$TMP/core.wasm"
wasm-tools component new "$TMP/core.wasm" --adapt "$SDK/wasi_snapshot_preview1.reactor.wasm" -o "$OUT"

# Append the manifest as a custom section (id 0: LEB128 size, then the
# LEB128-prefixed name, then the TOML).
leb() {
    n=$1
    while :; do
        b=$((n & 127)); n=$((n >> 7))
        [ "$n" -gt 0 ] && b=$((b | 128))
        printf "\\$(printf %03o "$b")"
        [ "$n" -gt 0 ] || break
    done
}
if [ -f "$MANIFEST" ]; then
    name=forge-manifest
    size=$(wc -c < "$MANIFEST")
    body=$((1 + ${#name} + size))
    { printf '\000'; leb "$body"; leb ${#name}; printf %s "$name"; cat "$MANIFEST"; } >> "$OUT"
else
    echo "build.sh: no manifest ($MANIFEST): the plugin is named after the file" >&2
fi
echo "$OUT: $(wc -c < "$OUT") bytes"
