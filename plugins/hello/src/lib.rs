// SPDX-License-Identifier: MIT OR Apache-2.0
//! An example Forge plugin: a command that asks for a name and inserts a
//! greeting, bound to C-c C-h, plus a log line whenever a file is saved.
//!
//!   rustup target add wasm32-wasip2
//!   cargo build --release --target wasm32-wasip2
//!   forge --plugin target/wasm32-wasip2/release/forge_hello.wasm
//!
//! (Plugins can't bind C-c followed by a plain letter: those keys belong
//! to the user. C-c C-letter is fine.)

use forge_plugin_sdk::{
    Buffer, Event, EventKind, Scope, command, forge_plugin, keymap, on, plugin_log, read_string,
};

fn init() -> Result<(), String> {
    command(
        "hello-greet",
        "Ask for a name and insert a greeting at point.",
        |prefix| {
            let times = prefix.unwrap_or(1).max(1) as usize;
            read_string("Greet whom? ", "world", move |name| {
                if let Some(name) = name {
                    Buffer::current().insert(&format!("Hello, {name}!\n").repeat(times));
                }
            });
            Ok(())
        },
    )?;
    keymap::bind(&Scope::Global, "C-c C-h", "hello-greet")?;

    on(EventKind::BufferSaved, |e| {
        if let Event::BufferSaved(id) = e
            && let Some(b) = Buffer::from_id(*id)
        {
            plugin_log!("saved {}", b.name());
        }
    });

    plugin_log!(
        "Hello from the {} plugin! Try C-c C-h.",
        env!("CARGO_PKG_NAME")
    );
    Ok(())
}

forge_plugin!(init);
