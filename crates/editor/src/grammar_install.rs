// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::app::App;
use crate::minibuffer::{Choice, Purpose};
use forge_core::BufferId;
use forge_core::grammar_install::{self, InstalledGrammar, Recipe};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const BUFFER: &str = "*Grammar Build*";

enum Event {
    Output(String),
    Done(Box<Result<InstalledGrammar, String>>),
}

struct Job {
    recipe: Recipe,
    buffer: BufferId,
}

#[derive(Default)]
pub struct GrammarInstall {
    job: Option<Job>,
    rx: Option<Receiver<Event>>,
}

impl GrammarInstall {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    if cmd != "treesit-install-language-grammar" {
        return false;
    }
    let names: Vec<String> = grammar_install::RECIPES
        .iter()
        .filter(|r| forge_core::syntax::language(r.name).is_none())
        .map(|r| r.name.to_string())
        .collect();
    if names.is_empty() {
        app.error("All known grammars are already installed");
    } else {
        app.prompt(
            "Install grammar: ",
            Purpose::Choose {
                what: Choice::GrammarInstall,
                candidates: names,
            },
        );
    }
    true
}

pub fn install_named(app: &mut App, name: &str) {
    if app.packages.grammar.busy() {
        app.error("A grammar is already being installed");
        return;
    }
    let Some(recipe) = grammar_install::recipe(name).copied() else {
        app.error(format!("No grammar recipe for {name}"));
        return;
    };
    if forge_core::syntax::language(name).is_some() {
        app.error(format!("Grammar {name} is already available"));
        return;
    }

    let buffer = output_buffer(app);
    crate::compile::append(
        app,
        buffer,
        &format!(
            "Installing {} grammar from {}\n",
            recipe.name, recipe.repository
        ),
    );

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = grammar_install::install(&recipe, &mut |line| {
            let _ = tx.send(Event::Output(line.to_string()));
        });
        let _ = tx.send(Event::Done(Box::new(result)));
    });
    app.packages.grammar.job = Some(Job { recipe, buffer });
    app.packages.grammar.rx = Some(rx);
    app.msg(format!("Installing {} grammar...", recipe.name));
}

pub fn poll(app: &mut App) {
    if !app.packages.grammar.busy() {
        return;
    }
    let mut output = Vec::new();
    let mut done = None;
    if let Some(rx) = &app.packages.grammar.rx {
        loop {
            match rx.try_recv() {
                Ok(Event::Output(text)) => output.push(text),
                Ok(Event::Done(result)) => {
                    done = Some(*result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = Some(Err("installer stopped unexpectedly".to_string()));
                    break;
                }
            }
        }
    }
    let Some((buffer, name)) = app
        .packages
        .grammar
        .job
        .as_ref()
        .map(|j| (j.buffer, j.recipe.name))
    else {
        return;
    };
    for text in output {
        crate::compile::append(app, buffer, &format!("{text}\n"));
    }
    let Some(result) = done else {
        return;
    };
    app.packages.grammar.job = None;
    app.packages.grammar.rx = None;
    match result {
        Ok(grammar) => match grammar_install::register_into(&mut app.editor, &grammar) {
            Ok(()) => {
                apply_to_current_file(app, &grammar);
                crate::compile::append(
                    app,
                    buffer,
                    &format!("Installed {} grammar\n", grammar.name),
                );
                app.msg(format!("Installed {} grammar", grammar.name));
            }
            Err(e) => {
                crate::compile::append(app, buffer, &format!("Cannot load {}: {e}\n", name));
                app.error(format!("Grammar {name}: {e}"));
            }
        },
        Err(e) => {
            crate::compile::append(app, buffer, &format!("Install failed: {e}\n"));
            app.error(format!("Grammar {name}: {e}"));
        }
    }
}

fn output_buffer(app: &mut App) -> BufferId {
    let id = match app.editor.find_buffer(BUFFER) {
        Some(id) => id,
        None => app.editor.add_buffer(forge_core::Buffer::new(BUFFER)),
    };
    if let Some(buffer) = app.editor.state.get_buffer_mut(id) {
        buffer.set_contents("");
        buffer.set_major_mode("compilation-mode");
    }
    crate::compile::display_buffer(app, id);
    id
}

fn apply_to_current_file(app: &mut App, grammar: &InstalledGrammar) {
    let Some(file_name) = app
        .buf_ref()
        .path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
    else {
        return;
    };
    if app.editor.modes.major_for_file(&file_name) == Some(grammar.mode.as_str()) {
        app.buf().set_major_mode(&grammar.mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{Buffer, Editor};

    fn test_app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn the_command_offers_recipes() {
        let mut app = test_app();
        assert!(execute(&mut app, "treesit-install-language-grammar"));
        let mb = app.minibuffer.as_ref().unwrap();
        match &mb.purpose {
            Purpose::Choose {
                what: Choice::GrammarInstall,
                candidates,
            } => {
                assert!(candidates.contains(&"typescript".to_string()));
                assert!(candidates.contains(&"ruby".to_string()));
                assert!(!candidates.contains(&"rust".to_string()));
            }
            _ => panic!("wrong prompt"),
        }
    }

    #[test]
    fn unknown_grammar_reports_an_error() {
        let mut app = test_app();
        install_named(&mut app, "not-a-grammar");
        assert_eq!(
            app.message.as_deref(),
            Some("No grammar recipe for not-a-grammar")
        );
    }
}
