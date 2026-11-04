// swift-tools-version: 5.9
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Tests for the macOS app's Swift, run without Xcode or the Rust core:
//
//   cd apple && swift test
//
// `ForgeCore` compiles the app's pure Swift (KeyTranslator, Theme) together
// with the UniFFI bindings, so the tests exercise the real types. The FFI
// symbols aren't linked (the Rust library isn't built here); the tests never
// call into it, and `-undefined dynamic_lookup` lets the test bundle load
// with those C symbols unresolved. Xcode's own build of the app doesn't use
// this package: it links the static library and the generated modulemap.
import PackageDescription

let package = Package(
    name: "ForgeSwiftTests",
    platforms: [.macOS(.v13)],
    targets: [
        // The low-level UniFFI C module (the generated header + modulemap),
        // so `forge_ffi.swift` can `import forge_ffiFFI`. `Generated/shim.c`
        // is an empty translation unit: a C target needs a source file.
        .target(
            name: "forge_ffiFFI",
            path: "Generated",
            publicHeadersPath: "include"
        ),
        // The app's Swift that doesn't need AppKit app state: the generated
        // bindings (for `Key`, `KeyInput`) and the pure logic on top of them.
        .target(
            name: "ForgeCore",
            dependencies: ["forge_ffiFFI"],
            path: "Forge",
            // The app's other sources need AppKit app state (and are
            // compiled by Xcode); exclude them so SwiftPM doesn't warn.
            exclude: [
                "EditorModel.swift",
                "EditorView.swift",
                "ForgeApp.swift",
                "ForgeMenuBar.swift",
                "FrameToolbar.swift",
                "MetalGrid.swift",
                "SceneRenderer.swift",
                "AppIntents.swift",
                "Desktop.swift",
                "MacOS.swift",
                "Assets.xcassets",
            ],
            sources: [
                "KeyTranslator.swift",
                "Theme.swift",
                "Generated/forge_ffi.swift",
            ],
            linkerSettings: [
                .unsafeFlags(["-Xlinker", "-undefined", "-Xlinker", "dynamic_lookup"])
            ]
        ),
        .testTarget(
            name: "ForgeCoreTests",
            dependencies: ["ForgeCore"],
            path: "Tests"
        ),
    ]
)
