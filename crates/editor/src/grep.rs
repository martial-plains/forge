// SPDX-License-Identifier: MIT OR Apache-2.0
//! grep: M-x grep and friends, the *grep* buffer, grep-mode,
//! and Grep Edit mode.
//!
//! The matches grep reports are treated like compiler errors, so M-g M-n
//! and RET visit them (see compile.rs).  The *grep* buffer shows each
//! match with its file name, or under a heading per file when
//! `grep-use-headings` is on, and the mode line counts the matches as
//! they come in.
//!
//! The options are variables of the same names, set through
//! `forge.opt.grep_*`.

use crate::app::App;
use crate::minibuffer::{ConfirmAction, Purpose};
use forge_core::process::{self, ProcessId};
use forge_core::{BufferId, Pos};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The major mode of the *grep* buffer.
pub const MODE: &str = "grep-mode";

/// The major mode of the *grep* buffer while it is being edited.
pub const EDIT_MODE: &str = "grep-edit-mode";

/// The region of the *grep* buffer's command line that the ellipsis
/// stands for; a click on it shows the whole command.
pub const ABBREVIATION_REGION: &str = "grep-abbreviation";

/// The default regexp that finds the exact match in a line of grep's
/// output: the text between the ANSI escape sequences grep puts around
/// it.  The first group is the match.
pub const DEFAULT_MATCH_REGEXP: &str = r"\x1b\[(?:0?1;)?31m(.*?)\x1b\[[0-9]*m";

/// The default command `lgrep` runs.
/// `<R>` is the pattern, `<F>` the files, `<X>` the files to leave out
/// and `<C>` the options.
pub const DEFAULT_TEMPLATE: &str = "grep <X> <C> -nH -e <R> <F>";

/// The default command `rgrep` runs.
/// `<D>` is the directory to search.
pub const DEFAULT_FIND_TEMPLATE: &str =
    "find -H <D> <X> -type f <F> -exec grep <C> -nH -e <R> {} +";

/// The directories `rgrep` doesn't look in, unless the user says
/// otherwise: the data directories of the version control systems
///.
pub const DEFAULT_IGNORED_DIRECTORIES: &[&str] = &[
    "SCCS", "RCS", "CVS", "MCVS", ".src", ".svn", ".git", ".hg", ".bzr", "_MTN", "_darcs",
    "{arch}", ".repo", ".jj",
];

/// Short names for sets of files that `lgrep` and `rgrep` understand
/// (`forge.opt.grep_files_aliases`).
pub const DEFAULT_FILES_ALIASES: &[(&str, &str)] = &[
    ("all", "* .*"),
    ("el", "*.el"),
    ("rs", "*.rs"),
    ("py", "*.py"),
    ("js", "*.js *.mjs *.cjs *.jsx"),
    ("ts", "*.ts *.tsx"),
    ("go", "*.go"),
    ("md", "*.md *.markdown"),
    ("org", "*.org"),
    ("ch", "*.c *.h"),
    ("c", "*.c"),
    ("cc", "*.cc *.cpp *.cxx *.c++ *.C"),
    (
        "cchh",
        "*.cc *.cpp *.cxx *.c++ *.C *.c *.h *.hh *.hpp *.hxx *.h++ *.H",
    ),
    ("hh", "*.hh *.hpp *.hxx *.h++ *.H"),
    ("h", "*.h"),
    ("l", "ChangeLog* changelog* CHANGELOG*"),
    ("am", "Makefile.am GNUmakefile *.mk"),
    ("m", "Makefile* makefile*"),
    ("tex", "*.tex"),
    ("texi", "*.texi"),
    ("asm", "*.s *.S"),
];

/// What to do with the modified buffers when a grep starts
///.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SaveBuffers {
    /// Don't save them.
    Never,
    /// Ask about each one.
    #[default]
    Ask,
    /// Save them all.
    Always,
}

impl SaveBuffers {
    /// The name of the value, as the option spells it.
    pub fn name(self) -> &'static str {
        match self {
            SaveBuffers::Never => "nil",
            SaveBuffers::Ask => "ask",
            SaveBuffers::Always => "always",
        }
    }

    /// The value of `forge.opt.grep_save_buffers`, if it names one.
    pub fn from_name(s: &str) -> Option<SaveBuffers> {
        match s.trim() {
            "nil" | "never" | "false" => Some(SaveBuffers::Never),
            "ask" => Some(SaveBuffers::Ask),
            "always" | "true" => Some(SaveBuffers::Always),
            _ => None,
        }
    }
}

/// The kinds of search that ask for a pattern and then for the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepKind {
    /// `lgrep`: the files to search are the ones named.
    Local,
    /// `rgrep`: the files to search are those in a directory tree.
    Recursive,
    /// `zrgrep`: like `rgrep`, but the files are gzipped.
    Compressed,
    /// `grep-find`: like `rgrep`, but the whole command can be edited.
    Find,
}

/// The grep options, as variables of the same names.
#[derive(Debug, Clone)]
pub struct GrepOptions {
    /// Highlight the exact match in the source file, not the whole line.
    pub highlight_matches: bool,
    /// The regexp that picks the match out of a line of grep's output.
    pub match_regexp: String,
    /// What to do with the modified buffers when a grep starts.
    pub save_buffers: SaveBuffers,
    /// Show the file name in a heading above each file's matches.
    pub use_headings: bool,
    /// The directories `rgrep` doesn't look in.
    pub ignored_directories: Vec<String>,
    /// The command `lgrep` runs.
    pub template: String,
    /// The command `rgrep` runs.
    pub find_template: String,
    /// Other names for a set of files (`grep-files-aliases`).
    pub files_aliases: Vec<(String, String)>,
    /// Whether the command in the *grep* buffer is shown abbreviated.
    pub abbreviate: bool,
}

impl Default for GrepOptions {
    fn default() -> Self {
        GrepOptions {
            highlight_matches: false,
            match_regexp: DEFAULT_MATCH_REGEXP.to_string(),
            save_buffers: SaveBuffers::Ask,
            use_headings: false,
            ignored_directories: DEFAULT_IGNORED_DIRECTORIES
                .iter()
                .map(|d| d.to_string())
                .collect(),
            template: DEFAULT_TEMPLATE.to_string(),
            find_template: DEFAULT_FIND_TEMPLATE.to_string(),
            files_aliases: DEFAULT_FILES_ALIASES
                .iter()
                .map(|(a, f)| (a.to_string(), f.to_string()))
                .collect(),
            abbreviate: true,
        }
    }
}

impl GrepOptions {
    /// The regexp that finds the exact match, if it is a usable one.
    pub fn match_re(&self) -> Option<Regex> {
        Regex::new(&self.match_regexp).ok()
    }
}

/// A match grep reported.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// The file the match is in.
    pub file: String,
    /// The line in that file, counting from 1.
    pub line: usize,
    /// The column the match starts at, counting from 1.
    pub column: Option<usize>,
    /// The line of the *grep* buffer the match is on, counting from 1.
    pub out_line: usize,
    /// The text of the match itself, when it was found.
    pub text: Option<String>,
}

/// A match in a line of grep's output: the file and the line it names.
struct Split {
    /// The file the match is in.
    file: String,
    /// The line in that file.
    line: usize,
    /// The column, when grep gave one.
    column: Option<usize>,
    /// The text of the line in the file (the rest of the output line).
    body: String,
    /// How long the `file:line:` prefix is, in characters.
    prefix: usize,
}

/// `file:line:`, `file:line:column:` and (under a heading) `line:`.
static WITH_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^\s:][^:\n]*?):(\d+)(?::(\d+))?:(.*)$").unwrap());
/// Just `line:`, the lines under a heading.
static LINE_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+):(.*)$").unwrap());

/// What a line of grep's output says: the match it reports, and the
/// text to show in the *grep* buffer.
pub struct GrepOutput {
    /// The lines to add to the *grep* buffer.
    pub lines: Vec<String>,
    /// The match the output reports, and the line of `lines` it is on
    /// (counting from 0).
    pub found: Option<(usize, GrepMatch)>,
}

/// The text of a line without the ANSI escape sequences in it.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            // The sequence ends with a byte in the range @ to ~.
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The file, line and column a line of grep's output names.
fn split_line(line: &str, current_file: Option<&str>) -> Option<Split> {
    if let Some(c) = WITH_FILE.captures(line) {
        let file = c[1].trim();
        if file.is_empty() || file.contains(char::is_whitespace) {
            return None;
        }
        let line_no: usize = c[2].parse().ok()?;
        let column = c.get(3).and_then(|m| m.as_str().parse::<usize>().ok());
        let body = c[4].to_string();
        let prefix = line[..body_start(line, &body)].chars().count();
        return Some(Split {
            file: file.to_string(),
            line: line_no,
            column,
            body,
            prefix,
        });
    }
    // Under a heading the file name is on the line above.
    let file = current_file?;
    let c = LINE_ONLY.captures(line)?;
    let line_no: usize = c[1].parse().ok()?;
    let body = c[2].to_string();
    let prefix = line[..line.len() - body.len()].chars().count();
    Some(Split {
        file: file.to_string(),
        line: line_no,
        column: None,
        body,
        prefix,
    })
}

/// Where the body of a split line starts, in bytes.
fn body_start(line: &str, body: &str) -> usize {
    match line.find(body) {
        Some(i) if line[i + body.len()..].is_empty() => i,
        _ => line.len().saturating_sub(body.len()),
    }
}

/// The text of a line of grep's output, and the match it reports.
///
/// The ANSI escape sequences are taken out of the text that is shown;
/// what they marked is the match itself, and where it starts.
pub fn grep_output(line: &str, current_file: Option<&str>, options: &GrepOptions) -> GrepOutput {
    let split = split_line(line, current_file);
    let Some(split) = split else {
        // Not a match: the line goes in as it is.
        return GrepOutput {
            lines: vec![strip_ansi(line)],
            found: None,
        };
    };
    let exact = options.match_re().and_then(|re| exact_match(&re, line));
    // The text that is shown has the escape sequences taken out.
    let body = strip_ansi(&split.body);
    let match_text = exact.as_ref().map(|(t, _)| t.clone());
    // The column of the match in the line of the file.
    let column = exact
        .as_ref()
        .map(|(_, start)| start.saturating_sub(split.prefix) + 1);
    let found = GrepMatch {
        file: split.file.clone(),
        line: split.line,
        column: column.or(split.column),
        out_line: 0,
        text: match_text,
    };
    if options.use_headings {
        let mut lines = Vec::new();
        let mut at = 0;
        if current_file != Some(split.file.as_str()) {
            lines.push(split.file.clone());
            at = 1;
        }
        lines.push(format!("{}:{}", split.line, body));
        GrepOutput {
            lines,
            found: Some((at, found)),
        }
    } else {
        GrepOutput {
            lines: vec![strip_ansi(line)],
            found: Some((0, found)),
        }
    }
}

/// The text the match regexp picks out of a line of grep's output, and
/// where it starts (in characters, from the start of the line).
fn exact_match(re: &Regex, line: &str) -> Option<(String, usize)> {
    let caps = re.captures(line)?;
    let whole = caps.get(0)?;
    let group = caps.get(1).unwrap_or(whole);
    let start = strip_ansi(&line[..group.start()]).chars().count();
    let text = strip_ansi(group.as_str());
    Some((text, start))
}

// ----------------------------------------------------------- the command

/// The command a search runs, and the part of it that is left out when
/// it is shown abbreviated.
pub struct GrepCommand {
    /// The whole command line.
    pub full: String,
    /// The command line with the ignored directories concealed.
    pub short: String,
    /// The part that is concealed (empty when there is none).
    pub hidden: String,
}

/// The parts a template is filled with.
pub struct Parts<'a> {
    /// The pattern to search for (`<R>`).
    pub pattern: &'a str,
    /// The files to search (`<F>`).
    pub files: &'a str,
    /// The directory to search (`<D>`).
    pub dir: Option<&'a str>,
    /// The files to leave out (`<X>`).
    pub exclude: &'a str,
    /// The options to add (`<C>`).
    pub options: &'a str,
}

/// The command a template builds: every placeholder in it is replaced
/// with the part it stands for.
pub fn fill(template: &str, parts: &Parts) -> String {
    let pattern = process::shell_quote(parts.pattern);
    let mut out = template.to_string();
    for (placeholder, value) in [
        ("<C>", parts.options),
        ("<D>", parts.dir.unwrap_or("")),
        ("<F>", parts.files),
        ("<N>", ""),
        ("<R>", pattern.as_str()),
        ("<X>", parts.exclude),
    ] {
        if value.is_empty() {
            // A placeholder with nothing in it leaves nothing behind.
            out = out.replace(&format!(" {placeholder}"), "");
            out = out.replace(placeholder, "");
        } else {
            out = out.replace(placeholder, value);
        }
    }
    tidy(&out)
}

/// The command with the spaces that are left over from the placeholders
/// taken out.
fn tidy(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut quote = false;
    let mut space = false;
    for c in command.chars() {
        if c == '\'' {
            quote = !quote;
        }
        if c == ' ' && !quote {
            if space {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// The command a search runs: the template with the pattern, the files,
/// the directory and the options in it.
pub fn build(
    kind: GrepKind,
    pattern: &str,
    files: &str,
    dir: &str,
    options: &GrepOptions,
    case_fold: bool,
) -> GrepCommand {
    let recursive = !matches!(kind, GrepKind::Local);
    let template = if recursive {
        options.find_template.clone()
    } else {
        options.template.clone()
    };
    // The options: `-i` for a search that ignores the case, and the
    // colour that marks the exact match.
    let mut opts: Vec<String> = Vec::new();
    if case_fold && !pattern.chars().any(char::is_uppercase) {
        opts.push("-i".to_string());
    }
    if options.highlight_matches {
        opts.push("--color=always".to_string());
    }
    let opts = opts.join(" ");
    // The files to leave out: the ignored directories, as the `find`
    // predicates that skip them.
    let exclude = if recursive {
        ignores(&options.ignored_directories)
    } else {
        String::new()
    };
    // The files to search: the ones the user named, or the patterns
    // `find` looks for.
    let files = if recursive {
        find_names(files)
    } else {
        files.trim().to_string()
    };
    let parts = Parts {
        pattern,
        files: &files,
        dir: if recursive { Some(dir) } else { None },
        exclude: &exclude,
        options: &opts,
    };
    let mut command = fill(&template, &parts);
    if matches!(kind, GrepKind::Compressed) {
        // zrgrep uses zgrep in place of grep
        command = command.replace(" grep ", " zgrep ");
        if command.contains("grep ") && !command.contains("zgrep ") {
            command = command.replace("grep ", "zgrep ");
        }
    }
    // The part that is left out when the command is shown abbreviated.
    let hidden = if exclude.is_empty() {
        String::new()
    } else {
        format!(" {exclude}")
    };
    let mut short = command.clone();
    if options.abbreviate && !hidden.is_empty() {
        short = short.replacen(&hidden, " …", 1);
    }
    GrepCommand {
        full: command,
        short,
        hidden,
    }
}

/// The `find` predicates that skip the ignored directories.
fn ignores(directories: &[String]) -> String {
    if directories.is_empty() {
        return String::new();
    }
    let paths = directories
        .iter()
        .map(|d| format!("-path {}", process::shell_quote(&format!("*/{d}"))))
        .collect::<Vec<_>>()
        .join(" -o ");
    format!("-type d ( {paths} ) -prune -o")
}

/// The `find` predicates that pick out the files the user named.
fn find_names(files: &str) -> String {
    let names: Vec<String> = files
        .split_whitespace()
        .map(|f| format!("-name {}", process::shell_quote(f)))
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("( {} )", names.join(" -o "))
    }
}

/// The files the user named, with the aliases (`grep-files-aliases`)
/// replaced by the files they stand for.
pub fn expand_aliases(files: &str, options: &GrepOptions) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in files.split_whitespace() {
        let mut replacement = word.to_string();
        for (alias, real) in &options.files_aliases {
            if alias == word {
                replacement = real.to_string();
            }
        }
        out.push(replacement);
    }
    out.join(" ")
}

// ------------------------------------------------------------- the state

/// What a running grep does with its output.
pub struct GrepJob {
    /// The file the last match was in: under a heading, the lines after
    /// it don't name their file.
    pub current_file: Option<String>,
    /// The part of a line that hasn't arrived yet.
    pub pending: String,
    /// The line of the *grep* buffer the next line of output goes to.
    pub next_line: usize,
    /// The options the job started with.
    pub options: GrepOptions,
    /// The command as the *grep* buffer shows it.
    pub display: String,
    /// The command that runs.
    pub command: String,
    /// The lines of the *grep* buffer that are headings.
    pub headings: Vec<usize>,
}

/// What a chunk of grep output became.
pub struct GrepChunk {
    /// The text to show in the *grep* buffer.
    pub text: String,
    /// The matches the chunk reported.
    pub matches: Vec<GrepMatch>,
    /// The lines of the *grep* buffer that are headings.
    pub headings: Vec<usize>,
}

impl GrepJob {
    /// A job for a search that runs `command`.
    pub fn new(options: GrepOptions, command: &str, display: &str, next_line: usize) -> GrepJob {
        GrepJob {
            current_file: None,
            pending: String::new(),
            next_line,
            options,
            display: display.to_string(),
            command: command.to_string(),
            headings: Vec::new(),
        }
    }

    /// Take a chunk of output: the text to show, the matches it reports
    /// and the headings it has.
    pub fn push(&mut self, chunk: &str) -> GrepChunk {
        let mut text = std::mem::take(&mut self.pending);
        text.push_str(chunk);
        let mut out = String::new();
        let mut matches = Vec::new();
        let mut headings = Vec::new();
        // Only the complete lines are handled; the rest waits.
        while let Some(i) = text.find('\n') {
            let line = text[..i]
                .strip_suffix('\r')
                .unwrap_or(&text[..i])
                .to_string();
            text = text[i + 1..].to_string();
            let result = grep_output(&line, self.current_file.as_deref(), &self.options);
            for (at, text_line) in result.lines.iter().enumerate() {
                out.push_str(text_line);
                out.push('\n');
                if let Some((hit, m)) = &result.found {
                    if *hit == at {
                        let mut m = m.clone();
                        m.out_line = self.next_line;
                        matches.push(m);
                    } else {
                        headings.push(self.next_line);
                    }
                }
                self.next_line += 1;
            }
        }
        self.pending = text;
        GrepChunk {
            text: out,
            matches,
            headings,
        }
    }

    /// The last line of the output, when it didn't end with a newline.
    pub fn flush(&mut self) -> GrepChunk {
        let line = std::mem::take(&mut self.pending);
        if line.is_empty() {
            return GrepChunk {
                text: String::new(),
                matches: Vec::new(),
                headings: Vec::new(),
            };
        }
        let result = grep_output(&line, self.current_file.as_deref(), &self.options);
        let mut out = String::new();
        let mut matches = Vec::new();
        let mut headings = Vec::new();
        for (at, text_line) in result.lines.iter().enumerate() {
            out.push_str(text_line);
            out.push('\n');
            if let Some((hit, m)) = &result.found {
                if *hit == at {
                    let mut m = m.clone();
                    m.out_line = self.next_line;
                    matches.push(m);
                } else {
                    headings.push(self.next_line);
                }
            }
            self.next_line += 1;
        }
        GrepChunk {
            text: out,
            matches,
            headings,
        }
    }
}

/// The matches of a grep buffer, in the order they were found.
pub fn matches_of(app: &App, id: BufferId) -> &[GrepMatch] {
    app.jobs
        .grep_matches
        .get(&id)
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

// -------------------------------------------------------------- commands

/// The commands of this module. False if `cmd` isn't one.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "grep" => {
            let pattern = if app.has_arg() {
                crate::compile::word_at_point_pub(app.buf_ref())
            } else {
                String::new()
            };
            let command = default_command(app, &pattern);
            crate::compile::prompt_with(app, "Run grep (like this): ", Purpose::Grep, &command);
        }
        "lgrep" => prompt_for_pattern(app, GrepKind::Local),
        "rgrep" => prompt_for_pattern(app, GrepKind::Recursive),
        "zrgrep" => prompt_for_pattern(app, GrepKind::Compressed),
        "grep-find" | "find-grep" => {
            let command = find_command(app, "");
            crate::compile::prompt_with(
                app,
                "Run grep-find (like this): ",
                Purpose::GrepFind,
                &command,
            );
        }
        "kill-grep" => {
            let id = app.active_id();
            let found: Vec<ProcessId> = app
                .jobs
                .running
                .iter()
                .filter(|(_, j)| j.buffer == id || is_grep(&j.kind))
                .map(|(p, _)| *p)
                .collect();
            match found.first() {
                Some(p) => {
                    app.jobs.processes.kill(*p);
                    app.msg("Killed");
                }
                None => app.error("No grep running"),
            }
        }
        "grep-find-toggle-abbreviation" => toggle_abbreviation(app),
        "grep-edit" => edit(app),
        "grep-edit-exit" => edit_exit(app),
        _ => return false,
    }
    true
}

/// The commands from this module. False if `cmd` isn't one.
pub fn is_grep(kind: &crate::compile::JobKind) -> bool {
    matches!(kind, crate::compile::JobKind::Grep(_))
}

/// The command M-x grep offers: a grep command line with the pattern
/// and the file of the current buffer in it.
pub fn default_command(app: &App, pattern: &str) -> String {
    let mut command = String::from("grep -nH ");
    if !pattern.is_empty() {
        command.push_str(&process::shell_quote(pattern));
        command.push(' ');
    }
    if let Some(path) = app.buf_ref().path() {
        command.push_str(&process::shell_quote(&path.display().to_string()));
    }
    command
}

/// The command M-x grep-find offers: the find command that searches
/// every file in a directory tree.
pub fn find_command(app: &App, pattern: &str) -> String {
    let dir = crate::compile::default_directory(app);
    let dir = dir.display().to_string();
    let parts = Parts {
        pattern,
        files: "",
        dir: Some(&dir),
        exclude: "",
        options: "",
    };
    fill(&app.grep.find_template, &parts)
}

/// M-x lgrep and friends: the pattern first, the files next, the
/// directory last.
fn prompt_for_pattern(app: &mut App, kind: GrepKind) {
    app.prompt(
        "Search for: ",
        Purpose::GrepSearch {
            kind,
            pattern: None,
            files: None,
            dir: None,
        },
    );
}

/// A search that asks for the pattern, then the files, then the
/// directory.
pub fn answered(app: &mut App, purpose: &Purpose, input: &str) {
    match purpose {
        Purpose::Grep | Purpose::GrepFind => {
            if input.trim().is_empty() {
                return;
            }
            let dir = crate::compile::default_directory(app);
            start(app, input, &dir);
        }
        Purpose::GrepSearch {
            kind,
            pattern,
            files,
            dir,
        } => match (pattern, files, dir) {
            (None, _, _) => {
                let pattern = input.to_string();
                app.prompt(
                    "Search files in: ",
                    Purpose::GrepSearch {
                        kind: *kind,
                        pattern: Some(pattern),
                        files: None,
                        dir: None,
                    },
                );
            }
            (Some(_), None, _) => {
                let files = input.to_string();
                let default = crate::compile::default_directory(app);
                let default = default.display().to_string();
                crate::compile::prompt_with(
                    app,
                    "In directory: ",
                    Purpose::GrepSearch {
                        kind: *kind,
                        pattern: pattern.clone(),
                        files: Some(files),
                        dir: None,
                    },
                    &default,
                );
            }
            (Some(pattern), Some(files), None) => {
                let dir = input.trim().to_string();
                let files = expand_aliases(files, &app.grep);
                let command = build(
                    *kind,
                    pattern,
                    &files,
                    &dir,
                    &app.grep,
                    app.case_fold_search,
                )
                .full;
                start(app, &command, Path::new(&dir));
            }
            _ => {}
        },
        _ => {}
    }
}

/// Run a grep command line, offering to save the modified buffers first.
fn start(app: &mut App, command: &str, dir: &Path) {
    let options = app.grep.clone();
    app.jobs.pending_grep = Some((command.to_string(), dir.to_path_buf()));
    let mut saves: Vec<BufferId> = Vec::new();
    for (id, b) in app.editor.state.buffers.iter() {
        if b.is_modified() && b.path().is_some() {
            saves.push(*id);
        }
    }
    if !saves.is_empty() {
        match options.save_buffers {
            SaveBuffers::Never => {}
            SaveBuffers::Always => {
                for id in saves {
                    save(app, id);
                }
            }
            SaveBuffers::Ask => {
                // The first buffer is offered; the rest follow one by
                // one as each answer comes back.
                let id = saves[0];
                let name = app
                    .editor
                    .state
                    .get_buffer(id)
                    .and_then(|b| b.path())
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                app.prompt(
                    format!("Save {name}? "),
                    Purpose::Confirm(ConfirmAction::GrepSave {
                        buffer: id,
                        rest: saves[1..].to_vec(),
                    }),
                );
                return;
            }
        }
    }
    run(app, command, dir, &options);
}

/// Save a buffer, for `grep-save-buffers`.
fn save(app: &mut App, id: BufferId) {
    if let Some(b) = app.editor.state.get_buffer_mut(id) {
        let _ = b.save();
    }
}

/// The answer to "Save the file? ", and the buffers still to ask about.
pub fn save_confirmed(app: &mut App, buffer: BufferId, rest: Vec<BufferId>, answer: bool) {
    if answer {
        save(app, buffer);
    }
    if let Some(next) = rest.first().copied() {
        let name = app
            .editor
            .state
            .get_buffer(next)
            .and_then(|b| b.path())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        app.prompt(
            format!("Save {name}? "),
            Purpose::Confirm(ConfirmAction::GrepSave {
                buffer: next,
                rest: rest[1..].to_vec(),
            }),
        );
        return;
    }
    // The last answer: the search runs.
    let Some((command, dir)) = app.jobs.pending_grep.take() else {
        return;
    };
    let options = app.grep.clone();
    run(app, &command, &dir, &options);
}

/// Run the grep, with the command line as the *grep* buffer shows it.
fn run(app: &mut App, command: &str, dir: &Path, options: &GrepOptions) {
    app.jobs.last_grep = Some(command.to_string());
    let display = abbreviate(command, options);
    let job = GrepJob::new(options.clone(), command, &display, 0);
    crate::compile::start(
        app,
        crate::compile::GREP,
        command,
        dir,
        crate::compile::JobKind::Grep(job),
    );
}

/// The command line as the *grep* buffer shows it: with the ignored
/// directories concealed, when the options say to.
pub fn abbreviate(command: &str, options: &GrepOptions) -> String {
    if !options.abbreviate {
        return command.to_string();
    }
    let ignores = ignores(&options.ignored_directories);
    if ignores.is_empty() {
        return command.to_string();
    }
    let hidden = format!(" {ignores}");
    command.replacen(&hidden, " …", 1)
}

/// Show the whole command, or the abbreviated one again.
fn toggle_abbreviation(app: &mut App) {
    let id = match app.editor.find_buffer(crate::compile::GREP) {
        Some(id) => id,
        None => return app.error("No grep buffer"),
    };
    let abbreviated = app.jobs.grep_abbreviated.get(&id).copied().unwrap_or(true);
    app.jobs.grep_abbreviated.insert(id, !abbreviated);
    let command = app.jobs.commands.get(&id).cloned().unwrap_or_default();
    let options = app.grep.clone();
    let shown = if abbreviated {
        command.clone()
    } else {
        abbreviate(&command, &options)
    };
    replace_command_line(app, id, &shown);
    app.msg(if abbreviated {
        "Showing the whole command"
    } else {
        "Abbreviating the command"
    });
}

/// The line of the *grep* buffer's header the command is on.
const COMMAND_LINE: usize = 3;

/// Put the command line back into the *grep* buffer's header.
fn replace_command_line(app: &mut App, id: BufferId, command: &str) {
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    if b.line_count() <= COMMAND_LINE {
        return;
    }
    let start = b.pos_to_offset(Pos::new(COMMAND_LINE, 0));
    let end = b.pos_to_offset(Pos::new(COMMAND_LINE + 1, 0));
    b.replace_offsets(start, end, &format!("{command}\n"));
    b.set_modified(false);
}

// ---------------------------------------------------------- grep edit

/// Typing `e` in the *grep* buffer: make it writable, so the matches
/// can be edited.
fn edit(app: &mut App) {
    let id = app.active_id();
    if app.buf_ref().name() != crate::compile::GREP {
        return app.error("Not in a grep buffer");
    }
    {
        let b = app.buf();
        b.set_read_only(false);
        b.set_major_mode(EDIT_MODE);
    }
    app.jobs.grep_edit = Some(id);
    app.msg("Grep Edit mode");
}

/// Leaving Grep Edit mode: the buffer is read-only again.
fn edit_exit(app: &mut App) {
    let id = app.active_id();
    if app.jobs.grep_edit != Some(id) {
        return app.error("Not in Grep Edit mode");
    }
    {
        let b = app.buf();
        b.set_read_only(true);
        b.set_major_mode(MODE);
    }
    app.jobs.grep_edit = None;
    app.msg("Grep mode");
}

/// The changes made in the *grep* buffer, put back into the files they
/// came from.  Called when the editor goes idle.
pub fn sync_edits(app: &mut App) {
    let Some(id) = app.jobs.grep_edit else {
        return;
    };
    let dir = app
        .jobs
        .dirs
        .get(&id)
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."));
    let matches = matches_of(app, id).to_vec();
    for m in matches {
        let text = {
            let Some(b) = app.editor.state.get_buffer(id) else {
                continue;
            };
            if m.out_line == 0 || m.out_line > b.line_count() {
                continue;
            }
            b.line(m.out_line - 1)
        };
        // The text of the line in the file, without the `file:line:`
        // prefix that grep put in front of it.
        let Some(split) = split_line(&text, Some(&m.file)) else {
            continue;
        };
        let body = split.body;
        // The line in the file, without the line number in front of it.
        let Some(source) = source_buffer(app, &dir, &m.file) else {
            continue;
        };
        let Some(b) = app.editor.state.get_buffer(source) else {
            continue;
        };
        // The file may have shrunk since the grep ran (it was reloaded
        // from disk, say): the line the match names can be gone, and the
        // buffer has no line for it.
        if m.line == 0 || m.line > b.line_count() {
            continue;
        }
        if b.line(m.line - 1) == body {
            continue;
        }
        let start = b.pos_to_offset(Pos::new(m.line - 1, 0));
        let end = b.pos_to_offset(Pos::new(m.line, 0));
        if let Some(b) = app.editor.state.get_buffer_mut(source) {
            b.replace_offsets(start, end, &format!("{body}\n"));
            b.break_undo_group();
        }
    }
}

/// The buffer visiting `file`, if there is one.
fn source_buffer(app: &App, dir: &Path, file: &str) -> Option<BufferId> {
    let path = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        dir.join(file)
    };
    app.editor
        .state
        .buffers
        .iter()
        .find(|(_, b)| b.path() == Some(path.as_path()))
        .map(|(id, _)| *id)
}

// -------------------------------------------------------- the mode line

/// The number of matches found so far, for the mode line.
pub fn match_count(app: &App) -> Option<usize> {
    // The *grep* buffer: the matches it holds, as they come in.
    let id = app.active_id();
    if app.editor.state.get_buffer(id).map(|b| b.name()) == Some(crate::compile::GREP) {
        return Some(matches_of(app, id).len());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> GrepOptions {
        GrepOptions::default()
    }

    #[test]
    fn strips_the_escape_sequences() {
        assert_eq!(strip_ansi("\x1b[01;31mfoo\x1b[0m"), "foo");
        assert_eq!(strip_ansi("a\x1b[0mb"), "ab");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn finds_the_file_and_line() {
        let out = grep_output("src/main.rs:12:    let x = 1;", None, &options());
        let (_, m) = out.found.expect("a match");
        assert_eq!(m.file, "src/main.rs");
        assert_eq!(m.line, 12);
        assert_eq!(out.lines, vec!["src/main.rs:12:    let x = 1;"]);
    }

    #[test]
    fn shows_the_file_name_in_a_heading() {
        let mut o = options();
        o.use_headings = true;
        let out = grep_output("src/main.rs:12:    let x = 1;", None, &o);
        assert_eq!(out.lines, vec!["src/main.rs", "12:    let x = 1;"]);
        let (at, m) = out.found.expect("a match");
        assert_eq!(at, 1);
        assert_eq!(m.file, "src/main.rs");
        assert_eq!(m.line, 12);
    }

    #[test]
    fn a_line_under_a_heading_names_no_file() {
        let mut o = options();
        o.use_headings = true;
        let out = grep_output("12:    let x = 1;", Some("src/main.rs"), &o);
        assert_eq!(out.lines, vec!["12:    let x = 1;"]);
        let (_, m) = out.found.expect("a match");
        assert_eq!(m.file, "src/main.rs");
    }

    #[test]
    fn builds_the_command_from_the_template() {
        let o = options();
        let c = build(GrepKind::Local, "foo", "a.rs b.rs", "/tmp", &o, false);
        assert_eq!(c.full, "grep -nH -e foo a.rs b.rs");
    }

    #[test]
    fn the_recursive_command_skips_the_ignored_directories() {
        let o = options();
        let c = build(GrepKind::Recursive, "foo", "", "/tmp/x", &o, false);
        assert!(c.full.contains("-path '*/.git'"), "{}", c.full);
        assert!(c.full.contains("-prune -o"), "{}", c.full);
        assert!(c.full.contains("/tmp/x"), "{}", c.full);
        // The ignored directories are concealed in the short command.
        assert!(c.short.contains('…'), "{}", c.short);
        assert!(!c.short.contains(".git"), "{}", c.short);
    }

    #[test]
    fn the_recursive_command_finds_the_named_files() {
        let o = options();
        let c = build(GrepKind::Recursive, "foo", "*.rs", "/tmp/x", &o, false);
        assert!(c.full.contains("-name '*.rs'"), "{}", c.full);
    }

    #[test]
    fn the_pattern_goes_in_the_template() {
        let parts = Parts {
            pattern: "foo",
            files: "",
            dir: Some("/tmp"),
            exclude: "",
            options: "",
        };
        assert_eq!(fill("grep -e <R>", &parts), "grep -e foo");
        assert_eq!(
            fill("find <D> -exec grep -e <R>", &parts),
            "find /tmp -exec grep -e foo"
        );
    }

    #[test]
    fn a_placeholder_with_nothing_in_it_leaves_no_space() {
        let parts = Parts {
            pattern: "foo",
            files: "",
            dir: None,
            exclude: "",
            options: "",
        };
        assert_eq!(fill("grep <C> -e <R> <F>", &parts), "grep -e foo");
    }

    #[test]
    fn the_aliases_are_replaced() {
        let mut o = options();
        o.files_aliases = vec![("proj".to_string(), "src/*.c".to_string())];
        assert_eq!(expand_aliases("proj", &o), "src/*.c");
        assert_eq!(expand_aliases("a.rs proj", &o), "a.rs src/*.c");
    }

    #[test]
    fn the_case_folded_search_gets_a_switch() {
        let o = options();
        let c = build(GrepKind::Local, "foo", "a.rs", "/tmp", &o, true);
        assert!(c.full.contains("-i"), "{}", c.full);
        // A pattern with a capital in it is not folded.
        let c = build(GrepKind::Local, "Foo", "a.rs", "/tmp", &o, true);
        assert!(!c.full.contains("-i"), "{}", c.full);
    }

    #[test]
    fn the_exact_match_is_the_text_between_the_markers() {
        let o = options();
        let out = grep_output(
            "src/main.rs:12:    let x = \x1b[01;31mfoo\x1b[0m;",
            None,
            &o,
        );
        let (_, m) = out.found.expect("a match");
        assert_eq!(m.text.as_deref(), Some("foo"));
        // The column of the match in the line of the file.
        assert_eq!(m.column, Some(13));
        // The escape sequences are not shown.
        assert_eq!(out.lines, vec!["src/main.rs:12:    let x = foo;"]);
    }

    /// A *grep* buffer with one match on `line` of `file`, and the file
    /// open in the editor.
    fn grep_app(file: &Path, line: usize, body: &str) -> (App, BufferId) {
        let mut app = App::new(forge_core::Editor::default());
        let (source, _) = app.editor.open_file(file).unwrap();
        let grep = app.editor.create_buffer(crate::compile::GREP);
        app.buf()
            .set_contents(&format!("{}:{line}:{body}\n", file.display()));
        app.jobs.grep_matches.insert(
            grep,
            vec![GrepMatch {
                file: file.display().to_string(),
                line,
                column: None,
                out_line: 1,
                text: None,
            }],
        );
        app.jobs.grep_edit = Some(grep);
        app.jobs
            .dirs
            .insert(grep, file.parent().unwrap().to_path_buf());
        (app, source)
    }

    fn temp_file(tag: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-grep-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, text).unwrap();
        // The editor opens a file by its canonical name.
        file.canonicalize().unwrap()
    }

    #[test]
    fn sync_edits_skips_a_match_whose_line_the_file_no_longer_has() {
        // The file is shorter than it was when the grep ran: the match
        // names a line that is gone.
        let file = temp_file("short", "one\ntwo\n");
        let (mut app, source) = grep_app(&file, 9, "three");
        sync_edits(&mut app);
        let b = app.editor.state.get_buffer(source).unwrap();
        assert_eq!(b.get_contents(), "one\ntwo\n");
    }

    #[test]
    fn sync_edits_puts_a_changed_line_back_into_the_file() {
        let file = temp_file("changed", "one\ntwo\n");
        let (mut app, source) = grep_app(&file, 2, "TWO");
        sync_edits(&mut app);
        let b = app.editor.state.get_buffer(source).unwrap();
        assert_eq!(b.get_contents(), "one\nTWO\n");
    }
}
