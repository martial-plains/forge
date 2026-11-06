// SPDX-License-Identifier: MIT OR Apache-2.0
//! `forge --check-plugin FILE [--run COMMAND] [--answer TEXT]...`: load a
//! plugin without a terminal, show what it set up, and run commands.
//!
//! For plugin authors and for testing the SDKs of every language in CI:
//! the output is plain text, and the exit status says whether everything
//! worked. The plugin gets what its manifest asks for (it's your own
//! plugin; nothing is saved). Questions are answered from the `--answer`
//! values in order (C-g once they run out); buffer edits happen in a
//! *scratch* buffer printed at the end.
//!
//! Steps run in the order given: `--run COMMAND`, `--open FILE` (visit
//! a file, as if with C-x C-f), `--point LINE:COL` (move point in the
//! current buffer; line from 1, column from 0), `--wait SECONDS` (let
//! events and the output of the plugin's processes arrive, until they
//! stop or the time is up: for plugins like language servers).

use forge_core::plugin::runtime::Level;
use forge_core::{Editor, Effect, Outcome, PluginHost, Question};
use std::collections::VecDeque;
use std::path::Path;
use std::time::Instant;

const EVENTS: [&str; 5] = [
    "buffer-opened",
    "buffer-killed",
    "buffer-changed",
    "buffer-saved",
    "mode-changed",
];

pub enum Step {
    Run(String),
    Open(String),
    Point(usize, usize),
    Wait(f64),
}

pub struct Options {
    pub file: String,
    pub steps: Vec<Step>,
    pub answers: Vec<String>,
    /// Starting text of *scratch*.
    pub text: String,
    /// Compile cache (see PluginHost::with_cache_dir).
    pub cache: Option<std::path::PathBuf>,
}

/// Returns whether the plugin loaded and every command ran without errors.
pub fn run(opts: Options) -> bool {
    let mut ed = Editor::new();
    let scratch = ed.create_buffer("*scratch*");
    ed.state
        .get_buffer_mut(scratch)
        .expect("just made")
        .set_contents(&opts.text);
    // The compile cache makes checking the same plugin again quick.
    let mut host = match PluginHost::with_cache_dir(opts.cache.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            println!("error: can't start the plugin runtime: {e:#}");
            return false;
        }
    };
    let started = Instant::now();
    let req = match host.inspect_file(Path::new(&opts.file)) {
        Ok(r) => r,
        Err(e) => {
            println!("error: {e:#}");
            return false;
        }
    };
    let compiled = started.elapsed();
    let name = req.name().to_string();
    println!("plugin: {name}");
    if let Some(m) = &req.manifest {
        if let Some(v) = m.version() {
            println!("version: {v}");
        }
        if let Some(d) = m.description() {
            println!("description: {d}");
        }
    }
    let requested = req.requested();
    let perms = requested.describe();
    println!(
        "permissions: {}",
        if perms.is_empty() {
            "none".to_string()
        } else {
            perms.join("; ")
        }
    );
    println!("imports:");
    for i in &req.inspection.imports {
        println!("  {i}");
    }
    let (r, out) = host.load_request(&mut ed, req, &requested);
    let mut ok = report(&out);
    if let Err(e) = r {
        println!("error: {e:#}");
        return false;
    }
    println!(
        "loaded in {:.0?} (compiling {:.0?})",
        started.elapsed(),
        compiled
    );

    let p = host.get(&name).expect("just loaded");
    let commands: Vec<String> = ed
        .commands
        .names()
        .filter(|c| ed.commands.plugin_for(c) == Some(&name))
        .map(String::from)
        .collect();
    println!("commands:");
    for c in &commands {
        println!(
            "  {c}: {}",
            ed.commands.get(c).map(|i| i.doc.as_str()).unwrap_or("")
        );
    }
    for (scope, keys, command) in p.bindings() {
        println!("binding: {keys} -> {command} ({})", scope.label());
    }
    for (scope, from, to) in p.remaps() {
        println!("remap: {from} -> {to} ({})", scope.label());
    }
    for m in ed.modes.owned_by(&name) {
        println!("mode: {} ({:?})", m.name, m.kind);
    }
    let events: Vec<&str> = (0..EVENTS.len())
        .filter(|&k| p.subscribed(k))
        .map(|k| EVENTS[k])
        .collect();
    if !events.is_empty() {
        println!("events: {}", events.join(", "));
    }

    let mut answers: VecDeque<String> = opts.answers.into();
    let mut ran = false;
    for step in &opts.steps {
        let cmd = match step {
            Step::Run(c) => c,
            Step::Open(f) => {
                println!("> open {f}");
                match ed.open_file(f) {
                    Ok((id, _)) => ed.show_buffer(id),
                    Err(e) => {
                        println!("error: {f}: {e}");
                        ok = false;
                    }
                }
                ok &= report(&host.notice_changes(&mut ed, false));
                continue;
            }
            Step::Point(l, c) => {
                if let Some(b) = ed.state.active_buffer_mut() {
                    b.set_point(forge_core::Pos::new(l.saturating_sub(1), *c));
                }
                continue;
            }
            Step::Wait(secs) => {
                println!("> wait {secs}s");
                let deadline = Instant::now() + std::time::Duration::from_secs_f64(*secs);
                loop {
                    ok &= report(&host.notice_changes(&mut ed, true));
                    ok &= report(&host.poll_background(&mut ed));
                    ok &= report(&host.poll_processes(&mut ed));
                    if !(host.processes_running() || host.background_running())
                        || Instant::now() >= deadline
                    {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                continue;
            }
        };
        ran = true;
        println!("> {cmd}");
        if ed.commands.plugin_for(cmd) != Some(name.as_str()) {
            println!("error: {name} has no command {cmd}");
            ok = false;
            continue;
        }
        let mut out = host.run_command(&mut ed, cmd, None);
        // Questions and their answers, until none are left.
        loop {
            ok &= report(&out);
            let asked = out.effects.iter().find_map(|e| match e {
                Effect::Ask {
                    plugin,
                    request,
                    question,
                } => Some((plugin.clone(), *request, question.clone())),
                _ => None,
            });
            let Some((plugin, request, question)) = asked else {
                break;
            };
            let answer = answers.pop_front();
            let prompt = match &question {
                Question::Text { prompt, .. }
                | Question::Completing { prompt, .. }
                | Question::YOrN { prompt }
                | Question::Key { prompt } => prompt,
            };
            if let Question::Completing { candidates, .. } = &question {
                println!("candidates: {}", candidates.join(", "));
            }
            println!("question: {prompt}{}", answer.as_deref().unwrap_or("(C-g)"));
            out = host.answer(&mut ed, &plugin, request, answer.as_deref());
        }
    }
    if let Some(b) = ed
        .state
        .active_buffer()
        .filter(|_| ed.state.active_buffer != Some(scratch))
    {
        let p = b.point();
        println!("current buffer: {} at {}:{}", b.name(), p.line + 1, p.col);
    }
    let b = ed.state.get_buffer(scratch).expect("scratch");
    if ran {
        println!("--- *scratch* ---");
        println!("{}", b.get_contents());
    }
    for id in ed.buffer_ids() {
        let b = ed.state.get_buffer(id).expect("listed");
        if b.decorations().is_empty() {
            continue;
        }
        if id != scratch {
            println!("decorations in {}:", b.name());
        }
        for d in b.decorations() {
            let (a, z) = (b.offset_to_pos(d.start), b.offset_to_pos(d.end));
            println!(
                "decoration: {}:{}-{}:{} {}{}",
                a.line + 1,
                a.col,
                z.line + 1,
                z.col,
                d.face.name(),
                d.message
                    .as_deref()
                    .map(|m| format!(" \"{m}\""))
                    .unwrap_or_default()
            );
        }
    }
    ok
}

/// Print what a call left behind. False if there were errors.
fn report(out: &Outcome) -> bool {
    for (plugin, line) in &out.logs {
        let level = match line.level {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warning",
            Level::Error => "error",
        };
        println!("log [{plugin}] {level}: {}", line.message);
    }
    for e in &out.effects {
        match e {
            Effect::Message(m) => println!("message: {m}"),
            Effect::BrowseUrl(u) => println!("browses: {u}"),
            Effect::Popup(i) => println!("pops up {} items", i.len()),
            Effect::OpenDir(d) => println!("opens folder: {d}"),
            Effect::Kill(t) => println!("kills: {t}"),
            Effect::LoadRhai(p) => println!("loads: {p}"),
            Effect::RunCommand(c) => println!("runs command: {c}"),
            Effect::RunCommandWithArg(c, n) => println!("runs command: {c} (prefix {n})"),
            Effect::RunCommandLater(c, _) => println!("runs command once defined: {c}"),
            Effect::ExecuteKeys(k) => println!("types keys: {k}"),
            Effect::Completions { candidates, .. } => {
                let list: Vec<String> = candidates
                    .iter()
                    .map(|c| {
                        c.annotation
                            .as_ref()
                            .map_or(c.text.clone(), |a| format!("{} ({a})", c.text))
                    })
                    .collect();
                println!("completions: {}", list.join(", "));
            }
            Effect::Process { program, .. } => println!("runs process: {program:?}"),
            Effect::Fs { op, .. } => println!("file operation: {op:?}"),
            Effect::Http { method, url, .. } => println!("web request: {method} {url}"),
            Effect::Ask { .. } => {}
            Effect::Locate => {}
            Effect::SetPromptInput(t) => println!("sets prompt input: {t}"),
            Effect::SuspendPrompt => println!("suspends the prompt"),
            Effect::ResumeSession(i) => println!("resumes prompt session: {i}"),
            Effect::JumpToRow { row, accept } => {
                println!("jumps to row: {row} (accept: {accept})")
            }
        }
    }
    for e in &out.errors {
        println!("error: {e}");
    }
    out.errors.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_a_plugin_and_runs_its_commands() {
        let dir = std::env::temp_dir().join(format!("forge-check-{}", std::process::id()));
        let path = crate::tests::demo_plugin_file(&dir);
        let opts = |run: &[&str]| Options {
            file: path.display().to_string(),
            steps: run.iter().map(|s| Step::Run(s.to_string())).collect(),
            answers: vec!["Ada".into()],
            text: String::new(),
            cache: None,
        };
        assert!(run(opts(&["demo-greet"])));
        assert!(
            !run(opts(&["no-such-command"])),
            "an unknown command fails the check"
        );
        assert!(!run(Options {
            file: dir.join("missing.wasm").display().to_string(),
            ..opts(&[])
        }));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
