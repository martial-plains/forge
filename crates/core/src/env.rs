// SPDX-License-Identifier: MIT OR Apache-2.0
//! The editor's process environment and what the machine is: Forge's
//! `process-environment`, `initial-environment`, `system-type`,
//! `system-name` and the rest.
//!
//! The environment lives on the [`Editor`](crate::Editor), so what
//! `forge.env.set` changes is what `forge.process.run` starts programs
//! with: those inherit `process-environment`, not the environment Forge
//! was launched from. (Forge's own subprocesses -- compiles, version
//! control, the shell -- still inherit the launching environment.)

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static GRAPHIC: AtomicBool = AtomicBool::new(false);

/// A frontend that draws pictures in a window says so; the terminal leaves it off.
pub fn set_display_graphic(yes: bool) {
    GRAPHIC.store(yes, Ordering::Relaxed);
}

pub fn display_graphic() -> bool {
    GRAPHIC.load(Ordering::Relaxed)
}

/// The pictures Forge ships, written to the temporary directory on first
/// use so a plugin can hand the path to `images::set`.
pub fn builtin_image(name: &str) -> Option<std::path::PathBuf> {
    let bytes: &[u8] = match name {
        "logo" => include_bytes!("../assets/forge-logo.png"),
        _ => return None,
    };
    let path = std::env::temp_dir().join(format!("forge-builtin-{name}-{}.png", env!("CARGO_PKG_VERSION")));
    let current = std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
    if !current && std::fs::write(&path, bytes).is_err() {
        return None;
    }
    Some(path)
}

/// The variables Forge's subprocesses inherit, and what `forge.env` reads
/// and writes. The first entry for a name wins, as in Forge's
/// `process-environment`.
#[derive(Debug, Clone)]
pub struct Environment {
    vars: Vec<(String, String)>,
    initial: Vec<(String, String)>,
}

impl Default for Environment {
    /// Start from the environment Forge was launched with: Forge's
    /// `initial-environment`.
    fn default() -> Self {
        let vars: Vec<(String, String)> = std::env::vars().collect();
        Self {
            initial: vars.clone(),
            vars,
        }
    }
}

impl Environment {
    /// An environment built from a fixed list, for tests.
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        let vars: Vec<(String, String)> = vars.into_iter().collect();
        Self {
            initial: vars.clone(),
            vars,
        }
    }

    /// The value of `name`, or None. The first entry wins.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Set `name` to `value`, or remove it when `value` is None. Like
    /// `setenv`, only the first entry for `name` is touched.
    pub fn set(&mut self, name: &str, value: Option<String>) {
        match self.vars.iter().position(|(k, _)| k == name) {
            Some(i) => match value {
                Some(v) => self.vars[i].1 = v,
                None => {
                    self.vars.remove(i);
                }
            },
            None => {
                if let Some(v) = value {
                    self.vars.push((name.to_string(), v));
                }
            }
        }
    }

    /// Every variable, in order: `process-environment`.
    pub fn all(&self) -> &[(String, String)] {
        &self.vars
    }

    /// What Forge was launched with: `initial-environment`.
    pub fn initial(&self) -> &[(String, String)] {
        &self.initial
    }

    /// Replace the whole environment (used to restore it after
    /// `forge.env.with`).
    pub fn replace(&mut self, vars: Vec<(String, String)>) {
        self.vars = vars;
    }

    /// `KEY=VALUE` for each variable, the way `process-environment`
    /// prints.
    pub fn as_list(&self) -> Vec<String> {
        self.vars.iter().map(|(k, v)| format!("{k}={v}")).collect()
    }

    /// `substitute-env-vars`: expand `$NAME` and `${NAME}` from
    /// this environment. An unknown name expands to nothing.
    pub fn substitute(&self, text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' && i + 1 < bytes.len() {
                let (name, next) = if bytes[i + 1] == b'{' {
                    match text[i + 2..].find('}') {
                        Some(rel) => (&text[i + 2..i + 2 + rel], i + 2 + rel + 1),
                        None => {
                            out.push('$');
                            i += 1;
                            continue;
                        }
                    }
                } else {
                    let start = i + 1;
                    let mut j = start;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                    {
                        j += 1;
                    }
                    if j == start {
                        out.push('$');
                        i += 1;
                        continue;
                    }
                    (&text[start..j], j)
                };
                if let Some(v) = self.get(name) {
                    out.push_str(v);
                }
                i = next;
            } else {
                let ch = text[i..].chars().next().expect("a char boundary");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
        out
    }
}

/// The separator between directories in a path list: `:` everywhere but
/// Windows. `path-separator`.
pub fn path_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

/// `parse-colon-path`: split a path list on the separator. An
/// empty component means the current directory, which comes back as
/// `None` (Lua has no nil in an array, so it sees `false`).
pub fn parse_path(path: &str) -> Vec<Option<String>> {
    let sep = path_separator();
    path.split(sep)
        .map(|part| {
            if part.is_empty() {
                None
            } else {
                Some(part.to_string())
            }
        })
        .collect()
}

/// `system-type`: which flavor of system this is, as a symbol
/// Forge spells as a string.
pub const fn system_type() -> &'static str {
    if cfg!(target_os = "linux") {
        "gnu/linux"
    } else if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows-nt"
    } else if cfg!(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )) {
        "berkeley-unix"
    } else {
        std::env::consts::OS
    }
}

/// `system-configuration`: the canonical name of the target Forge
/// was built for, e.g. `x86_64-unknown-linux-gnu`. Cargo sets it for the
/// build script (see build.rs).
pub const fn system_configuration() -> &'static str {
    match option_env!("FORGE_TARGET") {
        Some(target) => target,
        None => std::env::consts::OS,
    }
}

/// `system-name`: this machine's hostname.
pub fn system_name() -> String {
    crate::filelock::local_hostname()
}

/// This process's id: `process-id`.
pub fn pid() -> u32 {
    std::process::id()
}

/// `null-device`: where output is discarded.
pub fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// The name Forge was invoked as: `invocation-name`.
pub fn invocation_name() -> String {
    std::env::args_os()
        .next()
        .map(|arg| {
            Path::new(&arg)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| arg.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "forge".into())
}

/// The directory of the running binary: `invocation-directory`.
pub fn invocation_directory() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// `installation-directory`: where an uninstalled Forge keeps its
/// files. Forge installs nothing beside its binary, so this is nil, like
/// an installed Forge.
pub fn installation_directory() -> Option<std::path::PathBuf> {
    None
}

/// `load-average`: the system load over the last 1, 5 and 15
/// minutes, or None where the system won't say (Windows).
#[cfg(unix)]
pub fn load_average() -> Option<(f64, f64, f64)> {
    let mut load = [0f64; 3];
    let n = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    if n == 3 {
        Some((load[0], load[1], load[2]))
    } else {
        None
    }
}

#[cfg(not(unix))]
pub fn load_average() -> Option<(f64, f64, f64)> {
    None
}

/// `tty-erase-char`: the key the terminal sends for erase, or
/// None when this isn't a terminal.
#[cfg(unix)]
pub fn tty_erase_char() -> Option<u32> {
    use std::mem::MaybeUninit;
    unsafe {
        let mut termios = MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) == 0 {
            let c = termios.assume_init().c_cc[libc::VERASE] as u32;
            // 0 is unset; _POSIX_VDISABLE (usually 255) means "disabled".
            if c == 0 || c == libc::_POSIX_VDISABLE as u32 {
                None
            } else {
                Some(c)
            }
        } else {
            None
        }
    }
}

#[cfg(not(unix))]
pub fn tty_erase_char() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Environment {
        Environment::from_vars(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn get_and_set_follow_the_first_match() {
        let mut e = env(&[("A", "1"), ("B", "2")]);
        assert_eq!(e.get("A"), Some("1"));
        assert_eq!(e.get("missing"), None);

        // The first entry wins; set replaces it, not the later one.
        e.set("A", Some("9".into()));
        assert_eq!(e.get("A"), Some("9"));
        assert_eq!(e.all().len(), 2);

        e.set("C", Some("3".into()));
        assert_eq!(e.get("C"), Some("3"));

        e.set("B", None);
        assert_eq!(e.get("B"), None);
    }

    #[test]
    fn initial_stays_put_while_vars_change() {
        let mut e = env(&[("A", "1")]);
        e.set("A", Some("2".into()));
        assert_eq!(e.get("A"), Some("2"));
        assert_eq!(e.initial(), &[("A".to_string(), "1".to_string())]);
    }

    #[test]
    fn as_list_prints_key_value() {
        let e = env(&[("A", "1"), ("B", "2")]);
        assert_eq!(e.as_list(), vec!["A=1".to_string(), "B=2".to_string()]);
    }

    #[test]
    fn substitute_expands_both_forms() {
        let e = env(&[("HOME", "/home/me"), ("X", "y")]);
        assert_eq!(e.substitute("$HOME/bin"), "/home/me/bin");
        assert_eq!(e.substitute("${HOME}/bin"), "/home/me/bin");
        assert_eq!(e.substitute("a${X}b"), "ayb");
        assert_eq!(e.substitute("a$X b"), "ay b");
        assert_eq!(e.substitute("$NOPE/x"), "/x");
        assert_eq!(e.substitute("cost: $"), "cost: $");
        assert_eq!(e.substitute("literal $$X"), "literal $y");
    }

    #[test]
    fn parse_path_marks_the_current_directory() {
        assert_eq!(
            parse_path("/a:/b"),
            vec![Some("/a".into()), Some("/b".into())]
        );
        assert_eq!(parse_path(":/a"), vec![None, Some("/a".into())]);
        assert_eq!(parse_path(""), vec![None]);
    }

    #[test]
    fn system_facts_are_filled_in() {
        assert!(!system_type().is_empty());
        assert!(!system_configuration().is_empty());
        assert!(!system_name().is_empty());
        assert!(pid() > 0);
        assert!(null_device() == "/dev/null" || null_device() == "NUL");
    }
}

#[cfg(test)]
mod builtin_tests {
    #[test]
    fn the_logo_is_written_once_and_unknown_names_are_none() {
        let p = super::builtin_image("logo").expect("the logo");
        assert!(p.exists());
        assert_eq!(&std::fs::read(&p).unwrap()[1..4], b"PNG");
        assert_eq!(super::builtin_image("logo"), Some(p));
        assert!(super::builtin_image("nope").is_none());
    }
}
