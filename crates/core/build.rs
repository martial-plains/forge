// SPDX-License-Identifier: MIT OR Apache-2.0
// Forge's `system-configuration` (see `crate::env`): the target triple
// Cargo is building for. Cargo hands it to build scripts, not to the code,
// so pass it along as a compile-time environment variable.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=FORGE_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");

    // Rhai's tree-sitter grammar has no crate on crates.io, so its generated
    // parser is vendored (third_party/tree-sitter-rhai) and built here.
    let grammar = std::path::Path::new("third_party/tree-sitter-rhai");
    cc::Build::new()
        .include(grammar.join("src"))
        .file(grammar.join("src/parser.c"))
        .opt_level(2)
        .warnings(false)
        .compile("tree_sitter_rhai");
    println!(
        "cargo:rerun-if-changed={}",
        grammar.join("src/parser.c").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        grammar.join("src/tree_sitter/parser.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        grammar.join("queries/highlights.scm").display()
    );
}
