# Forge for macOS

A SwiftUI app around the same editor as the terminal `forge`: the Rust
core (`crates/ffi`, on top of `crates/terminal`'s editor and
`crates/core`) describes each frame (windows of styled text, mode lines,
the echo area, the cursor) and the app draws it as a grid of cells.

## Build

1. Install Rust (https://rustup.rs) and Xcode.
2. Open `Forge.xcodeproj` and run. The "Build Rust core" phase runs
   `../../scripts/build-apple.sh`, which builds `forge-ffi` for the Mac's
   architecture (`cargo build --profile fast`; `release` for Release
   builds) and regenerates the Swift bindings in `Forge/Generated/` and
   `Generated/include/`. The first build takes several minutes (Wasmtime).

## How it works

- `ForgeApp.swift`: the window, the menus (Cmd-O open, Cmd-V paste),
  Settings (Option as Meta).
- `EditorModel.swift`: owns `Forge` (from the bindings), sends keys,
  text and mouse events, asks for a frame after each and four times a
  second (idle work like messages and auto-save).
- `EditorView.swift`: an AppKit view that draws the frame and takes keys.
- `KeyTranslator.swift`: Control is C-, Option is M-, Command stays with
  macOS.
- `Theme.swift`: face names to system colors (light and dark mode).

The editor runs on its own thread in Rust, so a slow plugin never
freezes the window: the app keeps drawing the last frame with a "busy"
banner, and C-g stops the plugin.

The app isn't sandboxed (it reads `~/.config/forge` and any file you
open) and has the JIT entitlements Wasmtime needs to compile plugins.

## Not yet

Input methods (Japanese, emoji picker), the system clipboard beyond
Cmd-V, multiple windows/tabs, font settings, drag and drop.
