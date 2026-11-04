#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build a Forge plugin written in Lua:
#
#   ./build.sh plugin.lua [more.lua ...] [-o plugin.wasm] [--manifest forge.toml]
#
# The result is an ordinary plugin component: Lua 5.4 (lua/, unmodified
# except that io, os, package and debug are left out) plus host.c, which
# gives the scripts the `forge` API, plus your scripts, which run in
# order at init. It's sandboxed like any other plugin.
#
# Needs wit-bindgen, wasm-tools and zig or wasi-sdk (see ../c/build.sh).
# Lua's errors use setjmp/longjmp, which on WebAssembly need the
# exception-handling proposal: the compiler flags below turn it on, and
# Forge's Wasmtime runs it.
#
# The compiled Lua library is cached in .cache/ next to this script.
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
[ -n "$SOURCES" ] || { echo "usage: build.sh FILE.lua... -o OUT.wasm" >&2; exit 2; }
[ -n "$OUT" ] || OUT=plugin.wasm
if [ -z "$MANIFEST" ]; then
    set -- $SOURCES
    MANIFEST="$(dirname "$1")/forge.toml"
fi

if [ -n "${WASI_SDK_PATH:-}" ]; then
    CC="$WASI_SDK_PATH/bin/clang --target=wasm32-wasip1"
    AR="$WASI_SDK_PATH/bin/llvm-ar"
elif command -v zig >/dev/null 2>&1; then
    CC="zig cc -target wasm32-wasi"
    AR="zig ar"
elif python3 -c "import ziglang" 2>/dev/null; then
    CC="python3 -m ziglang cc -target wasm32-wasi"
    AR="python3 -m ziglang ar"
else
    echo "build.sh: no C compiler for WebAssembly: set WASI_SDK_PATH or install zig" >&2
    exit 1
fi
# setjmp/longjmp as Wasm exceptions (the standard encoding, not the
# legacy one); signal.h is emulated (Lua only includes it).
EH="-mexception-handling -mllvm -wasm-enable-sjlj -mllvm -wasm-use-legacy-eh=false"
CFLAGS="-O2 -g0 $EH -D_WASI_EMULATED_SIGNAL"

LIB="$SDK/.cache/liblua.a"
if [ ! -f "$LIB" ] || [ -n "$(find "$SDK/lua" "$SDK/setjmp.c" -newer "$LIB" 2>/dev/null)" ]; then
    echo "compiling Lua (once)..." >&2
    rm -rf "$SDK/.cache"
    mkdir -p "$SDK/.cache/obj"
    for c in "$SDK"/lua/*.c; do
        # shellcheck disable=SC2086
        $CC $CFLAGS -c "$c" -o "$SDK/.cache/obj/$(basename "$c" .c).o"
    done
    # The runtime that the sjlj lowering calls (as in wasi-libc, which
    # zig's bundled copy doesn't build with exceptions).
    $CC -O2 -g0 -mexception-handling -c "$SDK/setjmp.c" -o "$SDK/.cache/obj/setjmp.o"
    $AR rcs "$LIB" "$SDK"/.cache/obj/*.o
    rm -rf "$SDK/.cache/obj"
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
wit-bindgen c "$SDK/wit" --world plugin-full --rename-world plugin --out-dir "$TMP" >"$TMP/log" 2>&1 || { cat "$TMP/log" >&2; exit 1; }

# The scripts, as C byte arrays (NUL-terminated for convenience).
{
    echo '#include <stddef.h>'
    i=0
    for s in $SOURCES; do
        [ -f "$s" ] || { echo "build.sh: no such file: $s" >&2; exit 1; }
        echo "static const char s$i[] = {"
        od -An -v -tx1 "$s" | sed 's/ *\([0-9a-f][0-9a-f]\)/0x\1,/g'
        echo "0};"
        i=$((i + 1))
    done
    echo 'struct forge_lua_script { const char *name; const char *source; size_t len; };'
    echo 'const struct forge_lua_script forge_lua_scripts[] = {'
    i=0
    for s in $SOURCES; do
        echo "  {\"$(basename "$s")\", s$i, sizeof s$i - 1},"
        i=$((i + 1))
    done
    echo '  {0, 0, 0}};'
} > "$TMP/scripts.c"

for c in "$SDK/host.c" "$TMP/scripts.c" "$TMP/plugin.c"; do
    # shellcheck disable=SC2086
    $CC $CFLAGS -Wall -I"$TMP" -I"$SDK/lua" -c "$c" -o "$TMP/$(basename "$c" .c).o"
done
# Linking without the exception flags: with them, zig would rebuild its
# libc's setjmp, which fails (we bring our own).
$CC -mexec-model=reactor -Wl,--strip-debug "$TMP/host.o" "$TMP/scripts.o" "$TMP/plugin.o" \
    "$TMP/plugin_component_type.o" "$LIB" -lwasi-emulated-signal -o "$TMP/core.wasm"
wasm-tools component new "$TMP/core.wasm" --adapt "$SDK/wasi_snapshot_preview1.reactor.wasm" -o "$OUT"

# The manifest as the forge-manifest custom section (as in ../c/build.sh).
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
