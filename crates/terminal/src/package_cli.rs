// SPDX-License-Identifier: MIT OR Apache-2.0
//! `forge package declarations`: show what init.rhai declares without
//! starting the editor or running any plugin code.

use forge_core::CollectedDeclarations;
use forge_core::declare::{CollectedDeclaration, DeclarationState, DeferredCode, SkipReason};

/// Print declarations and return true when collection found no errors.
pub fn run_declarations(json: bool) -> anyhow::Result<bool> {
    let collected = forge_core::collect::collect_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&collected)?);
    } else {
        print_text(&collected);
    }
    Ok(collected.errors.is_empty())
}

fn print_text(collected: &CollectedDeclarations) {
    match &collected.init_path {
        Some(path) => println!("{}", path.display()),
        None => println!("init.rhai"),
    }
    if collected.entries.is_empty() {
        println!("No package declarations.");
    }
    for entry in &collected.entries {
        let line = entry
            .line
            .map(|line| format!(" (line {line})"))
            .unwrap_or_default();
        println!("{}{line}: {}", entry.name, state(entry));
        if !entry.deferred.is_empty() {
            let code: Vec<&str> = entry.deferred.iter().map(deferred_name).collect();
            println!("  deferred: {}", code.join(", "));
        }
        for need in &entry.system_packages {
            match &need.command {
                Some(command) if need.present => {}
                Some(command) => println!(
                    "  system package: {} (missing; would run {command})",
                    need.executable
                ),
                None if need.present => {}
                None => println!(
                    "  system package: {} (missing; no installer found)",
                    need.executable
                ),
            }
        }
    }
    for error in &collected.errors {
        eprintln!("error: {error}");
    }
}

fn state(entry: &CollectedDeclaration) -> String {
    match &entry.state {
        DeclarationState::Active => match &entry.declaration {
            Some(use_) if use_.lazy() => "active, deferred until used".to_string(),
            _ => "active, loads at startup".to_string(),
        },
        DeclarationState::Conditional => "conditional, not evaluated".to_string(),
        DeclarationState::Skipped(reason) => format!("skipped: {}", skip_name(reason)),
        DeclarationState::Invalid => "invalid".to_string(),
    }
}

fn skip_name(reason: &SkipReason) -> String {
    match reason {
        SkipReason::Disabled => "disabled".to_string(),
        SkipReason::WhenFalse => "when was false".to_string(),
        SkipReason::IfFalse => "if was false".to_string(),
        SkipReason::UnlessTrue => "unless was true".to_string(),
        SkipReason::MissingRequires(name) => format!("requires {name}, which isn't declared"),
    }
}

fn deferred_name(code: &DeferredCode) -> &'static str {
    match code {
        DeferredCode::Preface => "preface",
        DeferredCode::Init => "init",
        DeferredCode::Config => "config",
        DeferredCode::Condition => "condition",
    }
}
