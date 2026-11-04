// SPDX-License-Identifier: MIT OR Apache-2.0
//! Load a plugin component into a scratch editor and run its init.
//!
//!   cargo run -p forge-core --example load_plugin -- path/to/plugin.wasm
//!
//! (To use a plugin for real: forge --plugin path/to/plugin.wasm)

use forge_core::{Editor, PluginHost};

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: load_plugin <plugin.wasm>"))?;

    let mut editor = Editor::new();
    editor.create_buffer("*scratch*");
    let mut host = PluginHost::new()?;
    let (result, out) = host.load_file(
        &mut editor,
        std::path::Path::new(&path),
        &forge_core::Grants::default(),
    );
    for (plugin, line) in &out.logs {
        println!("  [{plugin}] {:?}: {}", line.level, line.message);
    }
    let name = result?;
    let p = host.get(&name).expect("just loaded");
    println!("Loaded {name} (WASI: {:?})", p.wasi);
    println!("It imports:");
    for i in &p.imports {
        println!("  {i}");
    }
    println!("Commands:");
    for c in editor
        .commands
        .names()
        .filter(|c| editor.commands.plugin_for(c) == Some(&name))
    {
        println!("  {c}: {}", editor.commands.get(c).unwrap().doc);
    }
    Ok(())
}
