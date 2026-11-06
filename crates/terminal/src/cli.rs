// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge's command line, after Forge's: the options that make sense here,
//! in the order they are given (so `-l a.rhai -f foo --eval 'x'` runs them
//! in that order). options that would do nothing here
//! (fonts, colours, the X display, the splash screen) are not accepted.

use forge_editor::App;

pub const USAGE: &str = "\
usage: forge [OPTIONS] [FILE]...

Action options (run in the order given, after the files are visited):
  FILE                visit FILE            +LINE[:COL] FILE   visit FILE at LINE
  --file FILE, --find-file FILE, --visit FILE
                      visit FILE
  -l, --load FILE     run FILE as a Rhai script
  --eval EXPR, --execute EXPR
                      evaluate the Rhai expression EXPR
  -f, --funcall CMD   run the command CMD
  --insert FILE       insert the contents of FILE at point
  -L, --directory DIR load the plugins (.wasm files) in DIR
  --plugin FILE       load a plugin (a .wasm component); may be repeated
  --kill              exit after the actions

Initialization options:
  -q, --no-init-file, --no-config
                      don't run init.rhai or read the saved session
  -Q, --quick         same as -q
  --init-directory DIR
                      use DIR as the config directory (init.rhai, themes,
                      plugins, grants)
  -u, --user USER     use the config directory of USER's home
  --chdir DIR         change to DIR before opening files

Modes:
  --batch, -batch     run the actions without a screen and exit; messages go
                      to standard error, the exit status is 1 on an error
  --script FILE       --batch -l FILE
  --daemon[=NAME]     start in the background as an edit server
  --bg-daemon[=NAME]  same as --daemon
  --fg-daemon[=NAME]  start as an edit server in this process

Forge's own:
  --check-plugin FILE [--run COMMAND]... [--answer TEXT]... [--text TEXT]
                 [--open FILE] [--point LINE:COL] [--wait SECONDS]
                    load a plugin without the editor, show what it set up,
                    run its commands (answering questions with --answer),
                    open files, wait for events and its processes' output
  --headless [--init FILE] [--plugin FILE] [--open FILE] [--keys KEYS] [--type TEXT]
             [--wait SECONDS] [--test CMD] [--screen] [--size WxH]
                    run the editor without a terminal, in that order, and
                    print the screen (for testing plugins end to end).
                    --test runs plugin-bisect with CMD as the test (a
                    nonzero exit status means the problem happens)
  --package-entry FILE.wasm [--url URL]
                    print a package index entry for a plugin (see
                    docs/plugin-api.md, \"Sharing plugins\")
  --package-snapshot-diff BEFORE AFTER [--json]
                    compare two saved package snapshots
  --package-lockfile-diff BEFORE AFTER [--json]
                    compare two lockfiles
  --package-doctor [--json]
                    check declared and installed plugins without starting
                    the editor; exit status is 1 on an error finding
  -h, --help        show this help
  -V, --version     show the version

Use forgeclient to open files and run code in a running Forge (M-x
server-start, or start it with --daemon).";

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Load(String),
    Eval(String),
    Funcall(String),
    Insert(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Daemon {
    /// `--daemon`: start a detached process and return.
    Background(String),
    /// `--fg-daemon`: be the server.
    Foreground(String),
}

#[derive(Debug, Default, PartialEq)]
pub struct CliOptions {
    pub no_config: bool,
    pub plugin_files: Vec<String>,
    pub plugin_dirs: Vec<String>,
    pub files: Vec<String>,
    pub chdir: Option<std::path::PathBuf>,
    pub position: Option<(u32, u32)>,
    pub actions: Vec<Action>,
    pub batch: bool,
    pub kill: bool,
    pub daemon: Option<Daemon>,
    pub init_directory: Option<std::path::PathBuf>,
    pub user: Option<String>,
    pub package_snapshot_diff: Option<(String, String)>,
    pub package_lockfile_diff: Option<(std::path::PathBuf, std::path::PathBuf)>,
    pub package_doctor: bool,
    pub json: bool,
}

/// The part before `=` of `--opt=value`, so both spellings work.
fn split_eq(arg: &str) -> (&str, Option<&str>) {
    if arg.starts_with("--")
        && let Some((a, b)) = arg.split_once('=')
    {
        return (a, Some(b));
    }
    (arg, None)
}

pub fn parse(args: &[String]) -> anyhow::Result<CliOptions> {
    let mut opts = CliOptions::default();
    let mut next_position = None;
    let mut it = args.iter();
    // Forge also accepts a single dash on long options (-batch, -eval).
    while let Some(raw) = it.next() {
        let (name, inline) = split_eq(raw);
        // -batch is --batch.
        let name: String = match name {
            "-batch" | "-script" | "-eval" | "-execute" | "-load" | "-funcall" | "-insert"
            | "-kill" | "-daemon" | "-bg-daemon" | "-fg-daemon" | "-quick" | "-no-init-file"
            | "-find-file" | "-visit" | "-file" | "-directory" | "-user" | "-chdir"
            | "-init-directory" | "-plugin" => format!("-{name}"),
            _ => name.to_string(),
        };
        let name = name.as_str();
        let mut value = |what: &str| -> anyhow::Result<String> {
            match inline {
                Some(v) => Ok(v.to_string()),
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{name} needs {what}\n{USAGE}")),
            }
        };
        match name {
            "-q" | "-Q" | "--quick" | "--no-init-file" | "--no-config" => opts.no_config = true,
            "--chdir" => opts.chdir = Some(value("a directory")?.into()),
            "--init-directory" => opts.init_directory = Some(value("a directory")?.into()),
            "-u" | "--user" => opts.user = Some(value("a user name")?),
            "--file" | "--find-file" | "--visit" => {
                opts.files.push(value("a file")?);
                opts.position = next_position.take();
            }
            "--plugin" => opts.plugin_files.push(value("a file")?),
            "-L" | "--directory" => opts.plugin_dirs.push(value("a directory")?),
            "-l" | "--load" => opts.actions.push(Action::Load(value("a file")?)),
            "--eval" | "--execute" => opts.actions.push(Action::Eval(value("an expression")?)),
            "-f" | "--funcall" => opts.actions.push(Action::Funcall(value("a command")?)),
            "--insert" => opts.actions.push(Action::Insert(value("a file")?)),
            "--package-snapshot-diff" => {
                let before = value("the first snapshot name")?;
                let after = value("the second snapshot name")?;
                opts.package_snapshot_diff = Some((before, after));
            }
            "--package-lockfile-diff" => {
                let before = value("the first lockfile")?;
                let after = value("the second lockfile")?;
                opts.package_lockfile_diff = Some((before.into(), after.into()));
            }
            "--package-doctor" => opts.package_doctor = true,
            "--json" => opts.json = true,
            "--kill" => opts.kill = true,
            "--batch" => opts.batch = true,
            "--script" => {
                opts.batch = true;
                opts.actions.push(Action::Load(value("a file")?));
            }
            "--daemon" | "--bg-daemon" => {
                opts.daemon = Some(Daemon::Background(inline.unwrap_or("server").to_string()));
            }
            "--fg-daemon" => {
                opts.daemon = Some(Daemon::Foreground(inline.unwrap_or("server").to_string()));
            }
            s if s.starts_with('+') && inline.is_none() => {
                let spec = &s[1..];
                let (line, column) = spec.split_once(':').unwrap_or((spec, "0"));
                let line = line
                    .parse::<u32>()
                    .ok()
                    .filter(|line| *line > 0)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "invalid line location {s}; expected +LINE[:COLUMN]\n{USAGE}"
                        )
                    })?;
                let column = column.parse::<u32>().map_err(|_| {
                    anyhow::anyhow!("invalid column in {s}; expected +LINE[:COLUMN]\n{USAGE}")
                })?;
                next_position = Some((line, column));
            }
            s if s.starts_with('-') && s.len() > 1 => {
                anyhow::bail!("unknown option {raw}\n{USAGE}")
            }
            _ => {
                opts.files.push(raw.clone());
                opts.position = next_position.take();
            }
        }
    }
    if next_position.is_some() {
        anyhow::bail!("a +LINE[:COLUMN] location must be followed by a file\n{USAGE}");
    }
    if opts.package_snapshot_diff.is_some() && opts.package_lockfile_diff.is_some() {
        anyhow::bail!(
            "--package-snapshot-diff and --package-lockfile-diff cannot be used together\n{USAGE}"
        );
    }
    if opts.json
        && opts.package_snapshot_diff.is_none()
        && opts.package_lockfile_diff.is_none()
        && !opts.package_doctor
    {
        anyhow::bail!(
            "--json is only used with --package-snapshot-diff, --package-lockfile-diff, or --package-doctor\n{USAGE}"
        );
    }
    Ok(opts)
}

/// Run the actions in order. In batch mode the messages they produce go to
/// standard error. False if any failed.
pub fn run_actions(app: &mut App, actions: &[Action], batch: bool) -> bool {
    let mut ok = true;
    for action in actions {
        app.message = None;
        match action {
            Action::Load(file) => match std::fs::read_to_string(file) {
                Ok(src) => ok &= eval(app, &src),
                Err(e) => {
                    eprintln!("Cannot open load file: {file}: {e}");
                    ok = false;
                }
            },
            Action::Eval(src) => ok &= eval(app, src),
            Action::Funcall(cmd) => {
                app.run_command(cmd);
                if app
                    .message
                    .as_deref()
                    .is_some_and(|m| m.starts_with("Unknown command"))
                {
                    ok = false;
                }
            }
            Action::Insert(file) => match std::fs::read_to_string(file) {
                Ok(text) => {
                    app.buf().insert_str(&text);
                    app.buf().break_undo_group();
                }
                Err(e) => {
                    eprintln!("{file}: {e}");
                    ok = false;
                }
            },
        }
        if batch && let Some(m) = app.message.take() {
            eprintln!("{m}");
        }
    }
    ok
}

fn eval(app: &mut App, src: &str) -> bool {
    match forge_editor::plugins::eval_rhai_string(app, src) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("{e}");
            false
        }
    }
}

/// The plugins (`*.wasm`) in `dir`, for -L.
pub fn plugins_in(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wasm"))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> anyhow::Result<CliOptions> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn actions_keep_their_order_and_files_their_positions() {
        let o = p(&["-l", "a.rhai", "+3:2", "x.txt", "-f", "foo", "--eval=1+1"]).unwrap();
        assert_eq!(
            o.actions,
            vec![
                Action::Load("a.rhai".into()),
                Action::Funcall("foo".into()),
                Action::Eval("1+1".into())
            ]
        );
        assert_eq!(o.files, vec!["x.txt"]);
        assert_eq!(o.position, Some((3, 2)));
    }

    #[test]
    fn batch_script_daemon_and_single_dash_spellings() {
        let o = p(&["-batch", "-eval", "1", "-kill"]).unwrap();
        assert!(o.batch && o.kill);
        assert_eq!(o.actions, vec![Action::Eval("1".into())]);
        let o = p(&["--script", "s.rhai"]).unwrap();
        assert!(o.batch);
        assert_eq!(o.actions, vec![Action::Load("s.rhai".into())]);
        assert_eq!(
            p(&["--daemon=work"]).unwrap().daemon,
            Some(Daemon::Background("work".into()))
        );
        assert_eq!(
            p(&["--fg-daemon"]).unwrap().daemon,
            Some(Daemon::Foreground("server".into()))
        );
    }

    #[test]
    fn package_snapshot_diff_cli_accepts_json_only_with_snapshots() {
        let options = p(&["--package-snapshot-diff", "before", "after", "--json"]).unwrap();
        assert_eq!(
            options.package_snapshot_diff,
            Some(("before".into(), "after".into()))
        );
        assert!(options.json);
        assert!(p(&["--json"]).is_err());
    }

    #[test]
    fn package_doctor_cli_accepts_json() {
        let options = p(&["--package-doctor", "--json"]).unwrap();
        assert!(options.package_doctor);
        assert!(options.json);
        assert!(p(&["--package-doctor"]).is_ok());
    }

    #[test]
    fn package_lockfile_diff_cli_accepts_json_and_rejects_conflicts() {
        let options = p(&[
            "--package-lockfile-diff",
            "before.toml",
            "after.toml",
            "--json",
        ])
        .unwrap();
        assert_eq!(
            options.package_lockfile_diff,
            Some(("before.toml".into(), "after.toml".into()))
        );
        assert!(options.json);
        assert!(
            p(&[
                "--package-snapshot-diff",
                "before",
                "after",
                "--package-lockfile-diff",
                "before.toml",
                "after.toml"
            ])
            .is_err()
        );
    }

    #[test]
    fn unsupported_options_that_do_nothing_here_are_rejected() {
        let o = p(&["-Q", "f"]).unwrap();
        assert!(o.no_config);
        assert_eq!(o.files, vec!["f"]);
        for bad in ["-nw", "--no-splash", "--geometry=80x24", "-fn", "--display"] {
            assert!(p(&[bad]).is_err(), "{bad}");
        }
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["-l"]).is_err());
        assert!(p(&["+3"]).is_err());
    }
}
