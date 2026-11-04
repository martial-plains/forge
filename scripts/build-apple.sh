#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build Forge's core for the macOS app (frontends/apple/Forge.xcodeproj runs this
# before compiling, and you can run it by hand):
#
#   - forge-ffi as a static library for the app's architectures
#     (lipo'd into target/apple/$CONFIGURATION/libforge_ffi.a),
#   - the Swift bindings (UniFFI) into frontends/apple/Forge/Generated/ and
#     frontends/apple/Generated/include/.
#
# Debug builds use the "fast" cargo profile (optimized, no LTO): plugins
# compile to machine code quickly with it. Release uses "release".
set -eu

# Xcode doesn't read your shell profile.
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CONFIG=${CONFIGURATION:-Debug}
if [ "$CONFIG" = Release ]; then PROFILE=release; else PROFILE=fast; fi
ARCHS=${ARCHS:-$(uname -m)}
OUT=$ROOT/target/apple/$CONFIG
mkdir -p "$OUT"

command -v cargo >/dev/null || { echo "error: cargo not found (install Rust: https://rustup.rs)" >&2; exit 1; }

LIBS=
for arch in $ARCHS; do
    case $arch in
        arm64) target=aarch64-apple-darwin ;;
        x86_64) target=x86_64-apple-darwin ;;
        *) echo "error: unknown architecture $arch" >&2; exit 1 ;;
    esac
    if command -v rustup >/dev/null && ! rustup target list --installed | grep -q "^$target\$"; then
        rustup target add "$target"
    fi
    # Xcode's own SDKROOT/LIBRARY_PATH/CPATH (for Swift/ObjC) confuse the
    # C compiler that cargo runs (cc-rs): with SDKROOT unset `cc` can't
    # find system headers (`string.h not found` in zstd-sys), and with
    # Xcode's raw values tree-sitter/Lua miscompile. Point cargo at the
    # macOS SDK explicitly instead.
    if MACOSX_SDK=$(xcrun --sdk macosx --show-sdk-path 2>/dev/null); then
        export SDKROOT="$MACOSX_SDK"
    fi
    env -u LIBRARY_PATH -u CPATH \
        cargo build --manifest-path "$ROOT/Cargo.toml" -p forge-ffi --lib --profile "$PROFILE" --target "$target"
    LIBS="$LIBS $ROOT/target/$target/$PROFILE/libforge_ffi.a"
done
# shellcheck disable=SC2086
lipo -create $LIBS -output "$OUT/libforge_ffi.a.new"
mv "$OUT/libforge_ffi.a.new" "$OUT/libforge_ffi.a"

# The Swift bindings. Only replace files that changed, so Xcode doesn't
# rebuild everything each time.
GEN=$ROOT/target/apple/gen
rm -rf "$GEN"
set -- $LIBS
env -u LIBRARY_PATH -u CPATH cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -p forge-ffi --bin uniffi-bindgen -- \
    generate --library "$1" --language swift --out-dir "$GEN" --no-format
put() {
    mkdir -p "$(dirname "$2")"
    cmp -s "$1" "$2" || cp "$1" "$2"
}
put "$GEN/forge_ffi.swift" "$ROOT/frontends/apple/Forge/Generated/forge_ffi.swift"
put "$GEN/forge_ffiFFI.h" "$ROOT/frontends/apple/Generated/include/forge_ffiFFI.h"
put "$GEN/forge_ffiFFI.modulemap" "$ROOT/frontends/apple/Generated/include/module.modulemap"
echo "forge-ffi: $OUT/libforge_ffi.a ($ARCHS, $PROFILE)"
