// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every command the editor knows: built-in ones (registered by the
//! frontend, which implements them) and ones plugins provide.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    /// Provided by the named plugin.
    Plugin(String),
    /// Defined in init.rhai.
    Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: String,
    pub doc: String,
    pub source: CommandSource,
}

#[derive(Debug, Default, Clone)]
pub struct CommandRegistry {
    commands: BTreeMap<String, CommandInfo>,
}

fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || "-_/:.*+<>=!?".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid command name {name:?}"))
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_builtin(&mut self, name: &str, doc: &str) {
        let info = CommandInfo {
            name: name.into(),
            doc: doc.into(),
            source: CommandSource::Builtin,
        };
        self.commands.insert(name.into(), info);
    }

    /// A plugin offers a command. It may re-register its own commands
    /// (when reloaded) but not take anyone else's name.
    pub fn register_plugin(&mut self, name: &str, doc: &str, plugin: &str) -> Result<(), String> {
        check_name(name)?;
        match self.commands.get(name).map(|c| &c.source) {
            Some(CommandSource::Builtin) => return Err(format!("{name} is a built-in command")),
            Some(CommandSource::Plugin(p)) if p != plugin => {
                return Err(format!("{name} is already provided by plugin {p}"));
            }
            Some(CommandSource::Config) => return Err(format!("{name} is defined in init.rhai")),
            _ => {}
        }
        let info = CommandInfo {
            name: name.into(),
            doc: doc.into(),
            source: CommandSource::Plugin(plugin.into()),
        };
        self.commands.insert(name.into(), info);
        Ok(())
    }

    /// init.rhai defines a command. Like a plugin, it can't take a
    /// built-in's or a plugin's name.
    pub fn register_config(&mut self, name: &str, doc: &str) -> Result<(), String> {
        check_name(name)?;
        match self.commands.get(name).map(|c| &c.source) {
            Some(CommandSource::Builtin) => return Err(format!("{name} is a built-in command")),
            Some(CommandSource::Plugin(p)) => {
                return Err(format!("{name} is already provided by plugin {p}"));
            }
            _ => {}
        }
        let info = CommandInfo {
            name: name.into(),
            doc: doc.into(),
            source: CommandSource::Config,
        };
        self.commands.insert(name.into(), info);
        Ok(())
    }

    /// Forget the commands init.rhai defined (before running it again).
    pub fn remove_config(&mut self) {
        self.commands
            .retain(|_, c| c.source != CommandSource::Config);
    }

    pub fn is_config(&self, name: &str) -> bool {
        self.commands
            .get(name)
            .is_some_and(|c| c.source == CommandSource::Config)
    }

    pub fn get(&self, name: &str) -> Option<&CommandInfo> {
        self.commands.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    /// All command names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(String::as_str)
    }

    /// The plugin that provides a command, if a plugin does.
    pub fn plugin_for(&self, name: &str) -> Option<&str> {
        match &self.commands.get(name)?.source {
            CommandSource::Plugin(p) => Some(p),
            CommandSource::Builtin | CommandSource::Config => None,
        }
    }

    /// Forget every command a plugin provided.
    pub fn remove_plugin(&mut self, plugin: &str) {
        self.commands
            .retain(|_, c| c.source != CommandSource::Plugin(plugin.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_cannot_take_names() {
        let mut r = CommandRegistry::new();
        r.register_builtin("forward-char", "Move.");
        assert!(r.register_plugin("forward-char", "", "p").is_err());
        r.register_plugin("p-hello", "Hi.", "p").unwrap();
        r.register_plugin("p-hello", "Hi again.", "p").unwrap();
        assert!(
            r.register_plugin("p-hello", "", "q")
                .unwrap_err()
                .contains("plugin p")
        );
        assert!(r.register_plugin("has space", "", "p").is_err());
        assert_eq!(r.plugin_for("p-hello"), Some("p"));
        r.remove_plugin("p");
        assert!(!r.contains("p-hello") && r.contains("forward-char"));
    }
}
