// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::BTreeSet;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use forge_docs::{Block, Doc, DocRegistry, Kind, Manual, Source, Span};

mod man;
mod news;
mod wit;

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Docs {
        #[arg(long)]
        out: Option<String>,
        /// Run the registry lints and fail if any doc is not up to scratch.
        #[arg(long)]
        check: bool,
    },
    Coverage,
    /// Generate a reference for the plugin API from the WIT file.
    Wit {
        #[arg(long)]
        out: Option<String>,
        /// Fail if any WIT item has no `///` comment.
        #[arg(long)]
        check: bool,
    },
    /// Write the forge(1) man page (docs/man/forge.1 by default).
    Man {
        #[arg(long)]
        out: Option<String>,
        /// Fail if the checked-in page is out of date.
        #[arg(long)]
        check: bool,
    },
    /// Refresh the generated block in NEWS.org from the registry.
    News {
        /// The release to generate for (defaults to the crate version).
        #[arg(long)]
        version: Option<String>,
        /// Fail if the checked-in NEWS.org is out of date.
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Docs { out, check } => {
            let (registry, commands, manual) = load();
            if check {
                check_docs(&registry)?;
                check_commands(&registry, &commands)?;
                check_manual(&manual)?;
                return Ok(());
            }
            let out = out.unwrap_or_else(|| "target/docs-site".to_string());
            let dir = std::path::Path::new(&out);
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join("index.html"), index_html(&registry))?;
            std::fs::write(dir.join("index.md"), index_text(&registry))?;
            std::fs::write(
                dir.join("manual.html"),
                forge_docs::render_manual_html(&manual),
            )?;
            std::fs::write(dir.join("forge.texi"), forge_docs::render_texinfo(&manual))?;
            std::fs::write(dir.join("search-index.json"), search_index(&registry))?;
            std::fs::write(dir.join("forge.1"), man::to_roff())?;
            eprintln!("wrote docs site to {out}");
        }
        Cmd::Coverage => {
            let (registry, commands, _manual) = load();
            let total = registry.iter().count();
            let cmds = commands.len();
            let docd: std::collections::BTreeSet<&str> = registry
                .iter()
                .filter(|d| d.kind == Kind::Command)
                .map(|d| d.name.as_str())
                .collect();
            eprintln!(
                "docs coverage: {}/{} commands documented, {} entries total",
                docd.len(),
                cmds,
                total
            );
            println!("docs_coverage_total={}", total);
            println!("docs_coverage_commands={}/{}", docd.len(), cmds);
            return Ok(());
        }
        Cmd::Wit { out, check } => {
            let wit = wit::Wit::parse(wit::WIT_SRC);
            let undocumented = wit.undocumented();
            if check {
                if !undocumented.is_empty() {
                    for item in &undocumented {
                        eprintln!("wit: no doc comment on {item}");
                    }
                    bail!("{} undocumented WIT items", undocumented.len());
                }
                eprintln!("wit: {} interfaces, all documented", wit.interfaces.len());
                return Ok(());
            }
            let out = out.unwrap_or_else(|| "target/docs-site/plugin-api.md".to_string());
            if let Some(parent) = std::path::Path::new(&out).parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, wit.to_markdown())?;
            eprintln!("wrote plugin API reference to {out}");
        }
        Cmd::Man { out, check } => {
            let page = man::to_roff();
            let out = out.unwrap_or_else(|| "docs/man/forge.1".to_string());
            if check {
                match std::fs::read_to_string(&out) {
                    Ok(on_disk) if on_disk == page => {
                        eprintln!("man: {out} is up to date");
                    }
                    Ok(_) => bail!("man: {out} is out of date; run `cargo xtask man`"),
                    Err(e) => bail!("man: cannot read {out}: {e}"),
                }
                return Ok(());
            }
            if let Some(parent) = std::path::Path::new(&out).parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, page)?;
            eprintln!("wrote man page to {out}");
        }
        Cmd::News { version, check } => {
            let version = version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            let (registry, _commands, _manual) = load();
            let docs: Vec<&Doc> = registry.iter().collect();
            let block = news::generated(&version, &docs);
            let path = "NEWS.org";
            let source = std::fs::read_to_string(path)?;
            let updated = match news::replace_block(&source, &version, &block) {
                Some(u) => u,
                None => bail!(
                    "news: {path} has no release section for {version} with a #+begin_generated fence"
                ),
            };
            if check {
                if updated == source {
                    eprintln!("news: {path} is up to date for {version}");
                } else {
                    bail!("news: {path} is out of date for {version}; run `cargo xtask news`");
                }
                return Ok(());
            }
            if updated != source {
                std::fs::write(path, &updated)?;
                eprintln!("wrote {path} for {version}");
            } else {
                eprintln!("{path} is already up to date for {version}");
            }
        }
    }
    Ok(())
}

/// Run [`mod@forge_docs::lint`] over the registry and report every problem
/// before failing, so a run fixes all of them at once.
fn check_docs(registry: &DocRegistry) -> Result<()> {
    let problems = forge_docs::lint(registry);
    if problems.is_empty() {
        eprintln!("docs: {} entries, no problems", registry.iter().count());
        return Ok(());
    }
    for problem in &problems {
        eprintln!("docs: {problem}");
    }
    anyhow::bail!("docs: {} problem(s) found", problems.len());
}

/// Cross-check the doc'd commands against the commands the editor actually
/// registers: every command doc must name a real command, and every command
/// must have a doc. Both directions, because a stale name on either side is
/// a hole in C-h f / M-x discoverability.
fn check_commands(registry: &DocRegistry, commands: &BTreeSet<String>) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    for doc in registry.iter() {
        if doc.kind == Kind::Command && !commands.contains(doc.name.as_str()) {
            problems.push(format!("docs: {} is doc'd but no such command", doc.name));
        }
    }
    let docd: BTreeSet<&str> = registry
        .iter()
        .filter(|d| d.kind == Kind::Command)
        .map(|d| d.name.as_str())
        .collect();
    for command in commands {
        if !docd.contains(command.as_str()) {
            problems.push(format!("docs: {command} has no doc"));
        }
    }
    if problems.is_empty() {
        eprintln!(
            "docs: {} commands, {} doc'd both ways",
            commands.len(),
            docd.len()
        );
        return Ok(());
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    anyhow::bail!("docs: {} cross-check problem(s) found", problems.len());
}

/// The manual's own lints: unique titles free of `: . ,`, and every
/// `[[*Node]]` reference landing on a node that exists.
fn check_manual(manual: &forge_docs::Manual) -> Result<()> {
    let problems = manual.lint();
    if problems.is_empty() {
        eprintln!("docs: manual, {} nodes, no problems", manual.nodes.len());
        return Ok(());
    }
    for problem in &problems {
        eprintln!("docs: {problem}");
    }
    anyhow::bail!("docs: {} manual problem(s) found", problems.len());
}

/// Every doc the editor ships, the parsed manual, and the set of command
/// names the editor registers, so every subcommand sees one consistent view.
fn load() -> (DocRegistry, BTreeSet<String>, Manual) {
    let app = forge_editor::app::App::new(forge_core::Editor::default());
    let commands = app.editor.commands.names().map(str::to_string).collect();
    let manual = forge_docs::the_manual().clone();
    let mut registry = app.docs;
    for doc in manual.as_docs() {
        registry.add(doc);
    }
    registry.add(bootstrap_doc());
    (registry, commands, manual)
}

/// A small JSON array the site's search box can fetch: one object per doc with
/// its name, kind, summary and the URL of its anchor.
fn search_index(registry: &DocRegistry) -> String {
    let mut out = String::from("[\n");
    for (i, doc) in registry.iter().enumerate() {
        let url = if doc.kind == Kind::Node {
            format!("manual.html#node-{}", forge_docs::render::slug(&doc.name))
        } else {
            format!(
                "index.html#{}-{}",
                doc.kind,
                forge_docs::render::slug(&doc.name)
            )
        };
        if i > 0 {
            out.push_str(",\n");
        }
        out.push_str(&format!(
            "  {{\"name\":{},\"kind\":{},\"summary\":{},\"url\":{}}}",
            json_string(&doc.name),
            json_string(&doc.kind.to_string()),
            json_string(&doc.summary),
            json_string(&url),
        ));
    }
    out.push_str("\n]\n");
    out
}

/// A minimal JSON string literal: quote, escape `\` and `"`, and drop control
/// characters. Enough for names and one-line summaries.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The bootstrap page. Until the manual and the live registries are wired in
/// (Phases 3-5 of the documentation design), this holds the page that describes the
/// documentation system itself, so the render pipeline is exercised end to end.
fn bootstrap_doc() -> Doc {
    Doc {
        kind: Kind::Concept,
        name: "forge-docs".to_string(),
        summary: "How Forge documents itself, in the editor and on the site.".to_string(),
        body: vec![Block::Para(vec![Span::Text(
            "Documentation is data: a structured record rendered at the last moment, \
             checked in CI, and shared by every consumer (C-h commands, the manual, \
             and this site)."
                .to_string(),
        )])],
        args: Vec::new(),
        see_also: Vec::new(),
        examples: Vec::new(),
        since: None,
        deprecated: None,
        source: Source::Builtin,
        manual: None,
        category: None,
    }
}

fn index_html(registry: &DocRegistry) -> String {
    let mut out = String::from(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
         <title>Forge Documentation</title></head><body>\n",
    );
    for doc in registry.iter() {
        out.push_str(&forge_docs::render_html(doc));
    }
    out.push_str("\n</body></html>\n");
    out
}

fn index_text(registry: &DocRegistry) -> String {
    let mut out = String::from("# Forge Documentation\n\n");
    for doc in registry.iter() {
        out.push_str(&forge_docs::render_text(doc));
        out.push('\n');
    }
    out
}
