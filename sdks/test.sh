#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Build every SDK's example plugin and check it in Forge (headless):
# load it, run its commands, compare with what it should do.
#
#   sdks/test.sh                 # all languages whose toolchain is installed
#   sdks/test.sh js python       # some
#
# Uses target/fast/forge if it exists (cargo build --profile fast: big
# JavaScript and Python components compile in seconds instead of
# minutes), else target/debug/forge. Missing toolchains are skipped.
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
FORGE=$ROOT/target/fast/forge
[ -x "$FORGE" ] || FORGE=$ROOT/target/debug/forge
[ -x "$FORGE" ] || { echo "build Forge first: cargo build --profile fast" >&2; exit 1; }
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
FAILED=0

# expect NAME OUTPUT-FILE LINE...: every LINE must appear in the output.
expect() {
    name=$1 file=$2
    shift 2
    for line in "$@"; do
        if ! grep -qxF -- "$line" "$file"; then
            echo "FAIL $name: missing \"$line\"" >&2
            sed 's/^/    /' "$file" >&2
            FAILED=1
            return
        fi
    done
    echo "ok   $name"
}

skip() { echo "skip $1: $2"; }

check() { "$FORGE" --check-plugin "$@" > "$OUT/check.txt" 2>&1 || echo "(exit status $?)" >> "$OUT/check.txt"; }

run_rust() {
    rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2 || { skip rust "rustup target add wasm32-wasip2"; return; }
    for p in hello; do
        (cd "$ROOT/plugins/$p" && cargo build -q --release --target wasm32-wasip2) || { echo "FAIL rust: $p doesn't build"; FAILED=1; return; }
    done
    check "$ROOT"/plugins/hello/target/wasm32-wasip2/release/forge_hello.wasm --run hello-greet --answer Ada
    expect rust/hello "$OUT/check.txt" "plugin: hello" "question: Greet whom? Ada" "Hello, Ada!"
}

run_meow() {
    [ -d "$ROOT/plugins/meow" ] || { skip meow "the plugin lives in its own repository"; return; }
    # Meow is built separately from the examples: it is big, and it is the
    # one plugin that rebinds the whole editor.
    rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2 || { skip meow "rustup target add wasm32-wasip2"; return; }
    (cd "$ROOT/plugins/meow" && cargo build -q --release --target wasm32-wasip2) || { echo "FAIL meow: doesn't build"; FAILED=1; return; }
    wasm="$ROOT/plugins/meow/target/wasm32-wasip2/release/forge_meow.wasm"
    printf 'alpha (beta gamma)\n' > "$OUT/meow.txt"
    check "$wasm" --open "$OUT/meow.txt" --point 1:0 --run meow-mode --run meow-mark-word
    expect meow/mode "$OUT/check.txt" "plugin: meow" "permissions: none" \
        "mode: meow-normal-mode (Minor)" \
        "mode: meow-leader-mode (Minor)" "mode: meow-motion-mode (Minor)" \
        "binding: w -> meow-mark-word (meow-normal-mode-map)" \
        "binding: SPC -> meow-keypad (meow-normal-mode-map)" \
        "binding: ESC -> meow-insert-exit (meow-insert-mode-map)"
    # The commands Meow's QWERTY layer runs all have to exist, or a key
    # silently does nothing.
    for c in meow-left meow-next meow-prev meow-right meow-expand-0 meow-expand-9 \
             meow-inner-of-thing meow-bounds-of-thing meow-beginning-of-thing meow-end-of-thing \
             meow-append meow-open-below meow-open-above meow-change meow-delete meow-backward-delete \
             meow-find meow-till meow-reverse meow-cancel-selection meow-grab meow-swap-grab \
             meow-sync-grab meow-kill meow-save meow-yank meow-undo meow-undo-in-selection \
             meow-visit meow-search meow-replace meow-join meow-block meow-to-block meow-quit \
             meow-goto-line meow-pop-selection meow-keypad meow-cheatsheet; do
        grep -q "^  $c: " "$OUT/check.txt" || { echo "FAIL meow: $c is bound in the layer but not registered" >&2; FAILED=1; }
    done
}

run_meow

run_js() {
    command -v node >/dev/null || { skip js "no node"; return; }
    [ -d "$ROOT/sdks/js/node_modules" ] || (cd "$ROOT/sdks/js" && npm install --no-audit --no-fund >/dev/null) || { skip js "npm install failed"; return; }
    node "$ROOT/sdks/js/bin/forge-plugin.js" build "$ROOT/sdks/js/examples/hello/plugin.js" -o "$OUT/hello-js.wasm" >/dev/null || { echo "FAIL js: build"; FAILED=1; return; }
    check "$OUT/hello-js.wasm" --run hello-js-greet --answer Ada
    expect js/hello "$OUT/check.txt" "plugin: hello-js" "version: 0.1.0" "binding: C-c C-j -> hello-js-greet (global-map)" \
        "events: buffer-saved" "question: Greet whom? Ada" "Hello, Ada!"
    node "$ROOT/sdks/js/bin/forge-plugin.js" build "$ROOT/sdks/js/examples/word-count/plugin.ts" -o "$OUT/wc.wasm" >/dev/null || { echo "FAIL ts: build"; FAILED=1; return; }
    check "$OUT/wc.wasm" --text "one two three" --run word-count
    expect ts/word-count "$OUT/check.txt" "plugin: word-count" "message: 3 words, 1 lines, 13 characters"
    node "$ROOT/sdks/js/bin/forge-plugin.js" build "$ROOT/sdks/js/examples/run-wc/plugin.js" -o "$OUT/run-wc.wasm" >/dev/null || { echo "FAIL js: build run-wc"; FAILED=1; return; }
    check "$OUT/run-wc.wasm" --text "one two three" --run run-wc --wait 10
    expect js/processes "$OUT/check.txt" "message: wc: 3 words (exit 0)"
}

run_python() {
    command -v componentize-py >/dev/null || { skip python "pip install componentize-py"; return; }
    (cd "$ROOT/sdks/python/examples/hello" && PYTHONPATH="$ROOT/sdks/python" python3 -m forge_plugin_build build app.py -o "$OUT/hello-py.wasm" >/dev/null) || { echo "FAIL python: build"; FAILED=1; return; }
    check "$OUT/hello-py.wasm" --text "one two three" --run hello-py-count-words --run hello-py-greet --answer Ada
    expect python/hello "$OUT/check.txt" "plugin: hello-py" "version: 0.1.0" "message: 3 words" "question: Greet whom? Ada" "Hello, Ada!"
}

run_c() {
    command -v wit-bindgen >/dev/null || { skip c "cargo install wit-bindgen-cli"; return; }
    command -v wasm-tools >/dev/null || { skip c "cargo install wasm-tools"; return; }
    (cd "$ROOT/sdks/c/examples/hello" && ../../build.sh hello.c -o "$OUT/hello-c.wasm" >/dev/null) || { echo "FAIL c: build"; FAILED=1; return; }
    check "$OUT/hello-c.wasm" --text "one two three" --run hello-c-count-words --run hello-c-greet --answer Ada
    expect c/hello "$OUT/check.txt" "plugin: hello-c" "version: 0.1.0" "message: 3 words" "question: Greet whom? Ada" "Hello, Ada!"
}

run_lua() {
    command -v wit-bindgen >/dev/null || { skip lua "cargo install wit-bindgen-cli"; return; }
    command -v wasm-tools >/dev/null || { skip lua "cargo install wasm-tools"; return; }
    (cd "$ROOT/sdks/lua/examples/hello" && ../../build.sh plugin.lua -o "$OUT/hello-lua.wasm" >/dev/null) || { echo "FAIL lua: build"; FAILED=1; return; }
    check "$OUT/hello-lua.wasm" --text "one two three" --run hello-lua-count-words --run hello-lua-greet --answer Ada
    expect lua/hello "$OUT/check.txt" "plugin: hello-lua" "version: 0.1.0" "message: 3 words" "question: Greet whom? Ada" "Hello, Ada!"
    check "$OUT/hello-lua.wasm" --text "one two three" --run hello-lua-wc --wait 10
    expect lua/processes "$OUT/check.txt" "message: wc: 3 words (exit 0)"
    check "$OUT/hello-lua.wasm" --text "one two three" --run hello-lua-slow-upcase --wait 20
    expect lua/background "$OUT/check.txt" "ONE TWO THREE"
    check "$OUT/hello-lua.wasm" --run hello-lua-fail
    expect lua/errors "$OUT/check.txt" "error: hello-lua: plugin.lua:26: this command always fails"
}

# contains NAME FILE TEXT...: every TEXT must appear somewhere in the output.
contains() {
    name=$1 file=$2
    shift 2
    for text in "$@"; do
        if ! grep -qF -- "$text" "$file"; then
            echo "FAIL $name: missing \"$text\"" >&2
            sed 's/^/    /' "$file" >&2
            FAILED=1
            return
        fi
    done
    echo "ok   $name"
}

# The LSP client against pyright: check_lsp TAG LSP.wasm
check_lsp() {
    tag=$1 wasm=$2
    mkdir -p "$OUT/py"
    touch "$OUT/py/pyproject.toml"
    printf 'def greet(name: str) -> str:\n    return name\n\n\nprint(greet(42))\n' > "$OUT/py/app.py"
    check "$wasm" --open "$OUT/py/app.py" --wait 20 --point 5:7 --run lsp-hover --wait 5 --run lsp-find-definition --wait 3
    expect "$tag" "$OUT/check.txt" "message: (function) def greet(name: str) -> str" "current buffer: app.py at 1:4"
    grep -q 'decoration: 5:12-5:14 error "Pyright: Argument of type' "$OUT/check.txt" || { echo "FAIL $tag: no diagnostic"; FAILED=1; }
    printf 'import os\n\nos.pat\n' > "$OUT/py/c.py"
    "$FORGE" --headless --size 70x12 --plugin "$wasm" --open "$OUT/py/c.py" --wait 8 --keys "M-> C-p C-e M-TAB" --wait 3 --screen \
        > "$OUT/lspc.txt" 2>&1
    contains "$tag-completion" "$OUT/lspc.txt" "pathsep"

    # References and rename, against pyright, on a two-file project.
    printf 'def shared(v: int) -> int:\n    return v\n' > "$OUT/py/lib.py"
    printf 'from lib import shared\n\nprint(shared(1))\nprint(shared(2))\n' > "$OUT/py/use.py"
    check "$wasm" --open "$OUT/py/use.py" --wait 20 --point 1:7 --run lsp-references --wait 6
    expect "$tag-refs" "$OUT/check.txt" "question: Go to reference: "
    grep -qE 'message: [0-9]+ references? to shared:' "$OUT/check.txt" || { echo "FAIL $tag: no reference count"; FAILED=1; }

    check "$wasm" --open "$OUT/py/use.py" --wait 20 --point 1:7 --run lsp-rename --answer renamed --wait 6
    expect "$tag-rename" "$OUT/check.txt" "question: Rename to: "
    grep -qE 'message: Renamed [0-9]+ places?' "$OUT/check.txt" || { echo "FAIL $tag: rename said nothing"; FAILED=1; }
    # The edits go into buffers, never straight to disk: the files on disk
    # still hold the old name and you review the buffers before saving.
    grep -q 'def shared' "$OUT/py/lib.py" || { echo "FAIL $tag: rename wrote to disk"; FAILED=1; }
    grep -q 'shared(1)' "$OUT/py/use.py" || { echo "FAIL $tag: rename wrote to disk"; FAILED=1; }

    # Signature help on a known call.
    printf 'def two(a: int, b: str) -> str:\n    return b * a\n\n\ntwo(1, "x")\n' > "$OUT/py/sig.py"
    check "$wasm" --open "$OUT/py/sig.py" --wait 20 --point 3:4 --run lsp-signature-help --wait 5
    grep -q 'message: .*def two' "$OUT/check.txt" || { echo "FAIL $tag: no signature"; FAILED=1; }
}

# plugins/lsp is Cairn: the same checks under Cairn's names and prompts,
# plus xref's list, Flymake and the mode line. check_cairn TAG LSP.wasm
check_cairn() {
    tag=$1 wasm=$2
    mkdir -p "$OUT/eg"
    touch "$OUT/eg/pyproject.toml"
    printf 'def greet(name: str) -> str:\n    return name\n\n\nprint(greet(42))\n' > "$OUT/eg/app.py"
    check "$wasm" --open "$OUT/eg/app.py" --wait 20 --point 5:7 --run eldoc --wait 5 --run xref-find-definitions --wait 3
    expect "$tag" "$OUT/check.txt" "message: (function) def greet(name: str) -> str" "current buffer: app.py at 1:4"
    grep -q 'decoration: 5:12-5:14 error "Pyright: Argument of type' "$OUT/check.txt" || { echo "FAIL $tag: no diagnostic"; FAILED=1; }
    grep -q "message: \[cairn\] Connected! Server .pyright" "$OUT/check.txt" || { echo "FAIL $tag: no connection message"; FAILED=1; }

    # The mode line with Flymake's counts; M-. and M-, back.
    "$FORGE" --headless --size 100x10 --plugin "$wasm" --open "$OUT/eg/app.py" --wait 15 \
        --keys "M-> C-p C-a C-f C-f C-f C-f C-f C-f C-f M-." --wait 3 --keys "M-," --wait 1 --screen \
        > "$OUT/eg1.txt" 2>&1
    contains "$tag-xref" "$OUT/eg1.txt" "[cairn:eg] 1E 0W" "buffer: app.py at 5:7"
    # Eldoc on an argument: the call's signature, its parameter marked.
    printf 'def two(a: int, b: str) -> str:\n    return b * a\n\n\ntwo(1, "x")\n' > "$OUT/eg/sig.py"
    "$FORGE" --headless --size 100x10 --plugin "$wasm" --open "$OUT/eg/sig.py" --wait 15 \
        --keys "M-> C-p C-e C-b C-b C-b C-b" --wait 2 --screen > "$OUT/eg3.txt" 2>&1
    contains "$tag-eldoc" "$OUT/eg3.txt" "|two: (a: int, ‹b: str›) -> str"

    printf 'import os\n\nos.pat\n' > "$OUT/eg/c.py"
    "$FORGE" --headless --size 70x12 --plugin "$wasm" --open "$OUT/eg/c.py" --wait 8 --keys "M-> C-p C-e M-TAB" --wait 3 --screen \
        > "$OUT/lspc.txt" 2>&1
    contains "$tag-completion" "$OUT/lspc.txt" "pathsep"

    # References go in *xref*, grouped by file; rename asks Cairn's way.
    printf 'def shared(v: int) -> int:\n    return v\n' > "$OUT/eg/lib.py"
    printf 'from lib import shared\n\nprint(shared(1))\nprint(shared(2))\n' > "$OUT/eg/use.py"
    "$FORGE" --headless --size 80x12 --plugin "$wasm" --open "$OUT/eg/use.py" --wait 15 --keys "C-e C-b M-?" --wait 4 --screen \
        > "$OUT/eg2.txt" 2>&1
    contains "$tag-refs" "$OUT/eg2.txt" "|4 references" "|lib.py" "|     1: def shared(v: int) -> int:" "|use.py" "|     4: print(shared(2))" "buffer: *xref*"

    check "$wasm" --open "$OUT/eg/use.py" --wait 20 --point 1:18 --run cairn-rename --answer renamed --wait 6
    expect "$tag-rename" "$OUT/check.txt" "question: Rename \`shared' to: renamed"
    grep -qE 'message: Renamed [0-9]+ places?' "$OUT/check.txt" || { echo "FAIL $tag: rename said nothing"; FAILED=1; }
    grep -q 'def shared' "$OUT/eg/lib.py" || { echo "FAIL $tag: rename wrote to disk"; FAILED=1; }
    grep -q 'shared(1)' "$OUT/eg/use.py" || { echo "FAIL $tag: rename wrote to disk"; FAILED=1; }

    # The old names still work.
    check "$wasm" --open "$OUT/eg/sig.py" --wait 20 --point 5:7 --run lsp-signature-help --wait 5
    grep -q 'message: two: (a: int, .*b: str' "$OUT/check.txt" || { echo "FAIL $tag: no signature"; FAILED=1; }
}

# The Phase 10 plugins, driven through the whole editor (--headless):
# check_favourites TAG VERTICO.wasm CORFU.wasm MEOW.wasm
check_favourites() {
    tag=$1
    "$FORGE" --headless --size 80x16 --plugin "$2" --keys "M-x" --type "buf kill" --screen > "$OUT/v.txt" 2>&1
    contains "$tag/vertico" "$OUT/v.txt" "|kill-buffer" "(C-x k)  Ask for a buffer name and kill that buffer" "|1/3 M-x buf kill"

    printf 'fn main() {\n    let letter = 1;\n    \n}\n' > "$OUT/t.rs"
    "$FORGE" --headless --size 60x10 --plugin "$3" --open "$OUT/t.rs" --keys "C-n C-n C-e" \
        --type "mat" --wait 0.6 --screen --keys "TAB" --type " let" --wait 0.6 --keys "C-n RET" > "$OUT/c.txt" 2>&1
    contains "$tag/corfu" "$OUT/c.txt" "match keyword" "buffer: t.rs at 3:16"

    printf 'hello world foo\nsecond line\n' > "$OUT/m.txt"
    "$FORGE" --headless --size 60x8 --plugin "$4" --open "$OUT/m.txt" --keys "w d i" --type "HI" \
        --keys "ESC j x d SPC x s" --screen > "$OUT/m.out" 2>&1
    # mktemp gives /var/... on macOS but the editor echoes the canonical
    # /private/var/... path, so only check the mode there.
    if [ "$(uname)" = Darwin ]; then
        contains "$tag/meow" "$OUT/m.out" "(Text [N])"
    else
        contains "$tag/meow" "$OUT/m.out" "(Text [N])" "echo: Wrote $OUT/m.txt"
    fi
    if [ "$(cat "$OUT/m.txt")" != "HI world foo" ]; then
        echo "FAIL $tag/meow: file is \"$(cat "$OUT/m.txt")\"" >&2
        FAILED=1
    fi
}

# vertico, corfu, meow and lsp: the plugins (Rust, plugins/) if the
# wasm32-wasip2 target is installed, and the same plugins written in Lua
# (sdks/lua/examples) if its toolchain is.
run_favourites() {
    if [ ! -d "$ROOT/plugins/vertico" ]; then
        skip "rust plugins" "they live in their own repositories"
    else
    if rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2; then
        ok=1
        for p in vertico corfu meow lsp; do
            (cd "$ROOT/plugins/$p" && cargo build -q --release --target wasm32-wasip2) || { echo "FAIL rust: $p doesn't build"; FAILED=1; ok=0; }
        done
        if [ $ok = 1 ]; then
            rel() { echo "$ROOT/plugins/$1/target/wasm32-wasip2/release/forge_$1.wasm"; }
            check_favourites rust "$(rel vertico)" "$(rel corfu)" "$(rel meow)"
            if command -v pyright-langserver >/dev/null; then check_cairn rust/lsp "$(rel lsp)"; else skip rust/lsp "pip install pyright"; fi
        fi
    else
        skip "rust plugins" "rustup target add wasm32-wasip2"
    fi
    fi
    if command -v wit-bindgen >/dev/null; then
        for p in vertico corfu meow lsp; do
            "$ROOT/sdks/lua/examples/$p/build.sh" -o "$OUT/$p-lua.wasm" >/dev/null || { echo "FAIL lua: $p doesn't build"; FAILED=1; return; }
        done
        check_favourites lua "$OUT/vertico-lua.wasm" "$OUT/corfu-lua.wasm" "$OUT/meow-lua.wasm"
        if command -v pyright-langserver >/dev/null; then check_lsp lua/lsp "$OUT/lsp-lua.wasm"; else skip lua/lsp "pip install pyright"; fi
    else
        skip "lua plugins" "cargo install wit-bindgen-cli"
    fi
}

LANGS=${*:-rust js python c lua favourites}
for l in $LANGS; do
    case $l in
        rust) run_rust ;;
        js | ts) run_js ;;
        python) run_python ;;
        c) run_c ;;
        lua) run_lua ;;

        favourites | vertico | corfu | meow | lsp) run_favourites ;;
        # Just Cairn, with a forge_lsp.wasm already built.
        cairn) check_cairn rust/lsp "$ROOT/plugins/lsp/target/wasm32-wasip2/release/forge_lsp.wasm" ;;
        *) echo "unknown language $l" >&2; FAILED=1 ;;
    esac
done
exit $FAILED
