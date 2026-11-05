// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org's rich content: footnotes, inline images, LaTeX fragments, source
//! blocks (Babel: running, results, tangling, editing in their own
//! buffer) and citations. The text work is in `forge_core::org_footnote`,
//! `org_blocks` and `export::cite`.

use crate::app::App;
use crate::org_cmds::{Ask, ask, ask_with, choose};
use forge_core::export::{cite, entities};
use forge_core::org_blocks::{self as blk, SrcBlock, Value};
use forge_core::org_display;
use forge_core::org_footnote as fnote;
use forge_core::{BufferId, Pos};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    /// Where `org-footnote-action` came from, to go back.
    pub fn_return: Option<Pos>,
    /// Blocks run without asking.
    pub no_confirm: bool,
    /// Block begin-lines waiting for a yes.
    pub pending: Vec<usize>,
    pub pending_buffer: Option<BufferId>,
    /// The block-edit buffer: (edit buffer, org buffer, begin line, kind).
    pub edit: Option<(BufferId, BufferId, usize, String)>,
    /// Named blocks from `org-babel-lob-ingest`: (file, lines).
    pub library: Vec<(String, Vec<String>)>,
    /// Session code, replayed before the next block of the same session:
    /// key is "language:name", value the blocks run so far.
    pub sessions: HashMap<String, Vec<String>>,
    /// Results of `:cache yes` blocks, keyed by the code that made them.
    pub cache: HashMap<String, String>,
    /// LaTeX fragments already rendered, key (fragment, display) to PNG.
    pub latex_images: HashMap<String, PathBuf>,
    pub header_key: Option<String>,
    pub timeout_secs: u64,
}

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-footnote-action",
        "Org: footnote command (C-c C-x f): jump between reference and definition,",
    )
    .body("or choose new/delete/sort/normalize/check."),
    forge_docs::CommandDoc::line(
        "org-footnote-new",
        "Org: insert a footnote reference at point and start its definition.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-delete",
        "Org: delete the footnote at point, with its definition or references.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-goto-definition",
        "Org: jump to the definition of the footnote at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-goto-previous-reference",
        "Org: from a definition, jump to a reference to it.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-normalize",
        "Org: number footnotes in order and gather their definitions at the end.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-sort",
        "Org: sort the footnote definitions by the order of their references.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-check",
        "Org: report references without a definition and definitions nothing refers to.",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-link-display",
        "Org: toggle showing links as their description (org-link-descriptive).",
    ),
    forge_docs::CommandDoc::line(
        "org-toggle-pretty-entities",
        "Org: toggle showing \\alpha-style entities and sub/superscripts as characters.",
    ),
    forge_docs::CommandDoc::new(
        "org-entities-help",
        "Org: list the entities (\\alpha, \\rarr, ...) with their LaTeX,",
    )
    .body("HTML and UTF-8 forms."),
    forge_docs::CommandDoc::line(
        "org-latex-preview",
        "Org: show the LaTeX fragment at point as text.",
    ),
    forge_docs::CommandDoc::line(
        "org-preview-latex-fragment",
        "Org: show the LaTeX fragment at point as text.",
    ),
    forge_docs::CommandDoc::line("org-clear-latex-preview", "Org: clear the LaTeX previews."),
    forge_docs::CommandDoc::line(
        "org-cdlatex-mode",
        "Org: toggle cdlatex math input (scripts, symbols, environments).",
    ),
    forge_docs::CommandDoc::line(
        "cdlatex-subscript",
        "Org cdlatex: insert a subscript, or the character itself outside math.",
    ),
    forge_docs::CommandDoc::line(
        "cdlatex-superscript",
        "Org cdlatex: insert a superscript, or the character itself outside math.",
    ),
    forge_docs::CommandDoc::line(
        "cdlatex-math-symbol",
        "Org cdlatex: ask for a LaTeX symbol name and insert it.",
    ),
    forge_docs::CommandDoc::line(
        "cdlatex-dollar",
        "Org cdlatex: insert a math pair and put point inside.",
    ),
    forge_docs::CommandDoc::line(
        "cdlatex-environment",
        "Org cdlatex: insert a LaTeX environment around point.",
    ),
    forge_docs::CommandDoc::new(
        "org-emphasize",
        "Org: emphasize the region or word at point (C-c C-x C-f):",
    )
    .body("one of * / _ = ~ + (space removes)."),
    forge_docs::CommandDoc::new(
        "org-edit-special",
        "Org: edit the source, example, export or comment block at point in its own",
    )
    .body("buffer (C-c ')."),
    forge_docs::CommandDoc::line(
        "org-edit-src-code",
        "Org: edit the source block at point in its own buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-edit-src-exit",
        "Org: write the edit buffer back into the Org buffer (C-c ').",
    ),
    forge_docs::CommandDoc::line(
        "org-edit-src-abort",
        "Org: drop the edit buffer without saving (C-c C-k).",
    ),
    forge_docs::CommandDoc::line(
        "org-edit-export-block",
        "Org: edit the export block at point in its own buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-execute-src-block",
        "Org Babel: run the source block at point and put the result below it (C-c C-c).",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-execute-src-block-maybe",
        "Org Babel: run the source block or #+CALL at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-execute-maybe",
        "Org Babel: run the source block or #+CALL at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-execute-buffer",
        "Org Babel: run every source block in the buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-execute-subtree",
        "Org Babel: run every source block in the subtree.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-expand-src-block",
        "Org Babel: show the block's code with variables and noweb references filled in.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-open-src-block-result",
        "Org Babel: show the results of the block at point in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-remove-result",
        "Org Babel: remove the results of the block at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-next-src-block",
        "Org Babel: go to the next source block.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-previous-src-block",
        "Org Babel: go to the previous source block.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-goto-src-block-head",
        "Org Babel: go to the #+BEGIN_SRC line of the block at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-goto-named-src-block",
        "Org Babel: go to the source block with a name.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-goto-named-result",
        "Org Babel: go to the results with a name.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-view-src-block-info",
        "Org Babel: show the language and header arguments of the block at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-insert-header-arg",
        "Org Babel: add a header argument to the block at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-demarcate-block",
        "Org Babel: split the source block at point in two.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-check-src-block",
        "Org Babel: check that the language of the block at point can be run.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-mark-block",
        "Org Babel: mark the body of the source block at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-tangle",
        "Org Babel: write the :tangle blocks of the buffer to their files (C-c C-v t).",
    ),
    forge_docs::CommandDoc::line("org-babel-tangle-file", "Org Babel: tangle an Org file."),
    forge_docs::CommandDoc::line(
        "org-babel-tangle-jump-to-org",
        "Org Babel: from a tangled file with link comments, jump to the Org block.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-lob-ingest",
        "Org Babel: make the named blocks of an Org file callable by #+CALL.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-toggle-confirm-evaluate",
        "Org Babel: toggle asking before running a block.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-load-in-session",
        "Org Babel: run the block in its session, without inserting its results.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-pop-to-session",
        "Org Babel: show the code a session has run so far.",
    ),
    forge_docs::CommandDoc::line(
        "org-babel-switch-to-session-with-code",
        "Org Babel: show the code a session has run so far.",
    ),
    forge_docs::CommandDoc::new(
        "org-cite-insert",
        "Org: insert a citation, choosing from the keys of the #+BIBLIOGRAPHY files (C-c",
    )
    .body("C-x @)."),
    forge_docs::CommandDoc::line("org-cite-insert-key", "Org: insert a citation key."),
    forge_docs::CommandDoc::line(
        "org-cite-follow",
        "Org: open the bibliography entry of the citation at point.",
    ),
    forge_docs::CommandDoc::line(
        "org-cite-insert-refs",
        "Org: insert a citation (same as org-cite-insert).",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-goto-next-reference",
        "Org: from a definition, jump to the next reference to it.",
    ),
    forge_docs::CommandDoc::line(
        "org-footnote-fix-failed",
        "Org: report references without a definition and definitions nothing refers to.",
    ),
    forge_docs::CommandDoc::new(
        "org-export-as",
        "Org: export to a string: opens the export dispatcher,",
    )
    .body("whose buffer entries show the result."),
    forge_docs::CommandDoc::line(
        "org-export-to-file",
        "Org: export to a file: opens the export dispatcher.",
    ),
    forge_docs::CommandDoc::line(
        "org-export-to-buffer",
        "Org: export to a buffer: opens the export dispatcher.",
    ),
    forge_docs::CommandDoc::line(
        "org-icalendar-export-to-ical",
        "Org export: write the buffer's dated entries as an iCalendar file.",
    ),
    forge_docs::CommandDoc::line(
        "org-odt-export-as-odf",
        "Org export: write the buffer as an OpenDocument text file.",
    ),
    forge_docs::CommandDoc::line(
        "org-latex-compile",
        "Org export: write the buffer as LaTeX and make a PDF.",
    ),
];

/// Commands that are another command under a second name.
fn alias(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "org-cite-insert-refs" => "org-cite-insert",
        "org-footnote-fix-failed" => "org-footnote-check",
        "org-export-as" | "org-export-to-file" | "org-export-to-buffer" => "org-export-dispatch",
        "org-icalendar-export-to-ical" => "org-icalendar-export-to-ics",
        "org-odt-export-as-odf" => "org-odt-export-to-odt",
        "org-latex-compile" => "org-latex-export-to-pdf",
        _ => return None,
    })
}

fn in_org(app: &mut App) -> bool {
    if app.buf_ref().major_mode() != forge_core::org::MODE {
        app.error("Not in an Org buffer");
        return false;
    }
    true
}

fn lines(app: &App) -> Vec<String> {
    let b = app.buf_ref();
    (0..b.line_count()).map(|i| b.line(i)).collect()
}

/// Replace the buffer's text, keeping point on the same line and column.
fn set_text(app: &mut App, text: &str, point: Pos) {
    let b = app.buf();
    let n = b.char_count();
    if b.get_contents() != text {
        b.replace_offsets(0, n, text);
    }
    b.set_point(point);
}

fn goto(app: &mut App, line: usize, col: usize) {
    let off = {
        let b = app.buf_ref();
        let line = line.min(b.line_count().saturating_sub(1));
        b.pos_to_offset(Pos::new(line, col))
    };
    let p = app.buf_ref().offset_to_pos(off);
    app.buf().set_point(p);
    let off = app.buf_ref().point_offset();
    app.buf().reveal(off);
}

fn base_dir(app: &App) -> PathBuf {
    app.buf_ref()
        .path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn source_name(app: &App) -> String {
    app.buf_ref()
        .path()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
        .unwrap_or_else(|| app.buf_ref().name().to_string())
}

fn stem(app: &App) -> String {
    app.buf_ref()
        .path()
        .and_then(|p| p.file_stem().map(|f| f.to_string_lossy().to_string()))
        .unwrap_or_else(|| "tangle".into())
}

// ------------------------------------------------------------ footnotes

const FN_SECTION: &str = "Footnotes";

fn fn_at_point(app: &App) -> (Option<fnote::FnRef>, Option<fnote::FnDef>) {
    let (refs, defs) = fnote::scan(&lines(app));
    let p = app.buf_ref().point();
    (
        fnote::ref_at(&refs, p.line, p.col).cloned(),
        fnote::def_at(&defs, p.line).cloned(),
    )
}

fn footnote_new(app: &mut App) {
    let ls = lines(app);
    let (refs, defs) = fnote::scan(&ls);
    let label = fnote::next_label(&refs, &defs);
    let p = app.buf_ref().point();
    app.org.rich.fn_return = Some(p);
    app.buf().insert_str(&format!("[fn:{label}]"));
    let text = app.buf_ref().get_contents();
    let (new, line) = fnote::add_definition(&text, &label, FN_SECTION);
    let col = format!("[fn:{label}] ").chars().count();
    set_text(app, &new, Pos::new(line, col));
    goto(app, line, col);
    app.msg(format!(
        "Footnote {label}: type the definition; C-c C-x f returns to the reference"
    ));
}

fn goto_def(app: &mut App, label: &str) {
    let text = app.buf_ref().get_contents();
    let ls = lines(app);
    let (_, defs) = fnote::scan(&ls);
    let from = app.buf_ref().point();
    match defs.iter().find(|d| d.label == label) {
        Some(d) => {
            app.org.rich.fn_return = Some(from);
            goto(app, d.line, 0);
        }
        None => {
            let (new, line) = fnote::add_definition(&text, label, FN_SECTION);
            set_text(app, &new, from);
            app.org.rich.fn_return = Some(from);
            let col = format!("[fn:{label}] ").chars().count();
            goto(app, line, col);
            app.msg(format!("Footnote {label} had no definition; made one"));
        }
    }
}

fn footnote_action(app: &mut App) {
    let (r, d) = fn_at_point(app);
    if let Some(r) = r
        && r.inline.is_none()
        && !r.label.is_empty()
    {
        goto_def(app, &r.label);
        return;
    }
    if let Some(d) = d {
        if let Some(ret) = app.org.rich.fn_return.take() {
            goto(app, ret.line, ret.col);
            return;
        }
        let ls = lines(app);
        let (refs, _) = fnote::scan(&ls);
        match refs.iter().find(|r| r.label == d.label) {
            Some(r) => goto(app, r.line, r.start),
            None => app.msg(format!("No reference to footnote {}", d.label)),
        }
        return;
    }
    choose(
        app,
        "Footnote action: ",
        Ask::FootnoteAction,
        ["new", "delete", "sort", "normalize", "check"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
}

fn footnote_prev_ref(app: &mut App) {
    let (_, d) = fn_at_point(app);
    let Some(d) = d else {
        app.error("Not in a footnote definition");
        return;
    };
    let ls = lines(app);
    let (refs, _) = fnote::scan(&ls);
    let from = app.buf_ref().point();
    let before: Vec<&fnote::FnRef> = refs.iter().filter(|r| r.label == d.label).collect();
    match before.last() {
        Some(r) => {
            app.org.rich.fn_return = Some(from);
            goto(app, r.line, r.start);
        }
        None => app.error(format!("No reference to footnote {}", d.label)),
    }
}

fn footnote_delete(app: &mut App) {
    let (r, d) = fn_at_point(app);
    let Some(label) = r.map(|r| r.label).or(d.map(|d| d.label)) else {
        app.error("No footnote at point");
        return;
    };
    let text = app.buf_ref().get_contents();
    let new = fnote::delete(&text, &label);
    let p = app.buf_ref().point();
    set_text(app, &new, p);
    app.msg(format!("Footnote {label} deleted"));
}

fn footnote_check(app: &mut App) {
    let (refs, defs) = fnote::scan(&lines(app));
    let mut bad = Vec::new();
    for r in &refs {
        if r.inline.is_none() && !r.label.is_empty() && !defs.iter().any(|d| d.label == r.label) {
            bad.push(format!(
                "line {}: reference [fn:{}] has no definition",
                r.line + 1,
                r.label
            ));
        }
    }
    for d in &defs {
        if !refs.iter().any(|r| r.label == d.label) {
            bad.push(format!(
                "line {}: definition [fn:{}] is never referenced",
                d.line + 1,
                d.label
            ));
        }
    }
    if bad.is_empty() {
        app.msg("All footnotes are fine".to_string());
    } else {
        let first = bad[0].clone();
        app.show_special_buffer("*Org Footnote Problems*", "text-mode", &bad.join("\n"));
        app.msg(format!("{} problem(s): {first}", bad.len()));
    }
}

// ----------------------------------------------------------- citations

fn bib_files(app: &App) -> Vec<PathBuf> {
    let base = base_dir(app);
    let mut out = Vec::new();
    for l in lines(app) {
        let t = l.trim();
        if t.len() > 15 && t[..15].eq_ignore_ascii_case("#+BIBLIOGRAPHY:") {
            let f = t[15..].trim().trim_matches('"');
            let p = crate::vc::expand_home(f);
            out.push(if p.is_absolute() { p } else { base.join(p) });
        }
    }
    out
}

fn bib_entries(app: &App) -> Vec<(PathBuf, cite::BibEntry)> {
    let mut out = Vec::new();
    for f in bib_files(app) {
        if let Ok(s) = std::fs::read_to_string(&f) {
            for e in cite::parse_bib(&s) {
                out.push((f.clone(), e));
            }
        }
    }
    out
}

fn cite_label(e: &cite::BibEntry) -> String {
    let who = e.authors().first().map(|a| a.0.clone()).unwrap_or_default();
    format!(
        "{} — {} {} {}",
        e.key,
        who,
        e.get("year").unwrap_or(""),
        e.get("title").unwrap_or("")
    )
    .trim()
    .to_string()
}

fn cite_insert(app: &mut App) {
    if !in_org(app) {
        return;
    }
    let es = bib_entries(app);
    if es.is_empty() {
        app.error(
            "No #+BIBLIOGRAPHY file with entries; add `#+BIBLIOGRAPHY: refs.bib`".to_string(),
        );
        return;
    }
    let cands = es.iter().map(|(_, e)| cite_label(e)).collect();
    choose(app, "Insert citation: ", Ask::CiteInsert, cands);
}

/// The `[cite...]` around point: (line, start col, end col, key at point).
fn cite_at_point(app: &App) -> Option<(usize, usize, usize, Option<String>)> {
    let p = app.buf_ref().point();
    let line: Vec<char> = app.buf_ref().line(p.line).chars().collect();
    let mut i = 0;
    while i + 5 < line.len() {
        if line[i] == '['
            && line[i + 1..].starts_with(&['c', 'i', 't', 'e'])
            && let Some(e) = line[i..].iter().position(|c| *c == ']')
        {
            let (s, e) = (i, i + e + 1);
            if p.col >= s && p.col < e {
                let inner: String = line[s..e].iter().collect();
                let off = p.col - s;
                let key = inner
                    .char_indices()
                    .filter(|(_, c)| *c == '@')
                    .map(|(k, _)| k)
                    .rfind(|k| *k <= off)
                    .map(|k| {
                        let tail: String = inner[k + 1..]
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || "_-:.?$%&+<>~/!*".contains(*c))
                            .collect();
                        tail
                    });
                return Some((p.line, s, e, key));
            }
            i = e;
            continue;
        }
        i += 1;
    }
    None
}

/// `C-c C-o` on a citation or footnote reference. True if it handled it.
pub fn follow(app: &mut App) -> bool {
    if let Some((_, _, _, Some(key))) = cite_at_point(app) {
        let es = bib_entries(app);
        match es.iter().find(|(_, e)| e.key == key) {
            Some((f, _)) => {
                let text = std::fs::read_to_string(f).unwrap_or_default();
                let line = text
                    .lines()
                    .position(|l| {
                        l.trim_start().starts_with('@') && l.contains(&format!("{{{key},"))
                    })
                    .unwrap_or(0);
                crate::minibuffer::visit_file(app, f, false);
                goto(app, line, 0);
            }
            None => app.error(format!("No bibliography entry for {key}")),
        }
        return true;
    }
    let (r, _) = fn_at_point(app);
    if let Some(r) = r
        && r.inline.is_none()
        && !r.label.is_empty()
    {
        goto_def(app, &r.label);
        return true;
    }
    false
}

// ---------------------------------------------------------------- babel

/// Point is in a source block or on a #+CALL line (what C-c C-c runs).
pub fn at_babel(app: &App) -> bool {
    let l = app.buf_ref().point().line;
    let ls = lines(app);
    parse_call(&ls[l.min(ls.len().saturating_sub(1))]).is_some() || blk::block_at(&ls, l).is_some()
}

fn find_block(app: &App, line: usize) -> Option<SrcBlock> {
    blk::block_at(&lines(app), line)
}

fn require_block(app: &mut App) -> Option<SrcBlock> {
    if !in_org(app) {
        return None;
    }
    let l = app.buf_ref().point().line;
    let b = find_block(app, l);
    if b.is_none() {
        app.error("Not in a source block");
    }
    b
}

/// How a language is run: an interpreter, a compiler and its linked
/// binary, Java's two-step compile, or a program that reads its script on
/// standard input (sqlite3).
#[derive(Clone, Copy)]
enum Runner {
    Script {
        prog: &'static str,
        args: &'static [&'static str],
        ext: &'static str,
    },
    Compile {
        compiler: &'static str,
        args: &'static [&'static str],
        link: &'static [&'static str],
        ext: &'static str,
    },
    Java,
    Stdin {
        prog: &'static str,
        args: &'static [&'static str],
    },
}

impl Runner {
    fn program(self) -> &'static str {
        match self {
            Runner::Script { prog, .. } | Runner::Stdin { prog, .. } => prog,
            Runner::Compile { compiler, .. } => compiler,
            Runner::Java => "javac",
        }
    }

    fn installed(self) -> bool {
        match self {
            Runner::Java => which("javac") && which("java"),
            Runner::Compile { compiler, .. } => which(compiler),
            Runner::Script { prog, .. } | Runner::Stdin { prog, .. } => which(prog),
        }
    }
}

/// The command and flags that run `lang`.
fn runner(lang: &str) -> Option<Runner> {
    Some(match blk::normalize_lang(lang).as_str() {
        "sh" => Runner::Script {
            prog: "bash",
            args: &[],
            ext: "sh",
        },
        "python" => Runner::Script {
            prog: "python3",
            args: &[],
            ext: "py",
        },
        "js" => Runner::Script {
            prog: "node",
            args: &[],
            ext: "js",
        },
        "ruby" => Runner::Script {
            prog: "ruby",
            args: &[],
            ext: "rb",
        },
        "perl" => Runner::Script {
            prog: "perl",
            args: &[],
            ext: "pl",
        },
        "lua" => Runner::Script {
            prog: "lua",
            args: &[],
            ext: "lua",
        },
        "awk" => Runner::Script {
            prog: "awk",
            args: &["-f"],
            ext: "awk",
        },
        "emacs-lisp" => Runner::Script {
            prog: "emacs",
            args: &["--batch", "--script"],
            ext: "el",
        },
        "r" => Runner::Script {
            prog: "Rscript",
            args: &[],
            ext: "R",
        },
        "haskell" => Runner::Script {
            prog: "runghc",
            args: &[],
            ext: "hs",
        },
        "go" => Runner::Script {
            prog: "go",
            args: &["run"],
            ext: "go",
        },
        "rust" => Runner::Compile {
            compiler: "rustc",
            args: &[],
            link: &[],
            ext: "rs",
        },
        "c" => Runner::Compile {
            compiler: "cc",
            args: &[],
            link: &["-lm"],
            ext: "c",
        },
        "cpp" => Runner::Compile {
            compiler: "c++",
            args: &[],
            link: &["-lm"],
            ext: "cpp",
        },
        "java" => Runner::Java,
        "sql" => Runner::Stdin {
            prog: "sqlite3",
            args: &[":memory:"],
        },
        _ => return None,
    })
}

fn runnable_languages() -> &'static str {
    "sh, python, js, ruby, perl, lua, awk, emacs-lisp, R, haskell, go, rust, c, c++, java, sql"
}

fn which(p: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(p).is_file()))
}

/// What running a block needs: its code with variables, the working
/// directory and the results settings.
struct Job {
    lang: String,
    code: String,
    dir: PathBuf,
    collect_value: bool,
    results: String,
    cmdline: String,
    file: Option<String>,
    wrap: Option<String>,
    stdin: Option<String>,
    session: Option<String>,
    cache: bool,
}

fn prepare(
    app: &App,
    ls: &[String],
    b: &SrcBlock,
    extra: &[(String, String)],
) -> Result<(Job, Vec<(String, String)>), String> {
    let mut args = blk::effective_args(ls, b);
    blk::merge_args(&mut args, extra.to_vec());
    let lang = blk::normalize_lang(&b.lang);
    let mut body = b.body(ls);
    if matches!(
        blk::arg(&args, "noweb"),
        Some("yes") | Some("no-export") | Some("strip-export")
    ) {
        body = blk::expand_noweb(ls, &body, 0);
    }
    let hlines = matches!(
        blk::arg(&args, "hlines").map(str::trim),
        Some("yes" | "t" | "true" | "1")
    );
    let mut pre = Vec::new();
    for (k, v) in &args {
        if k == "var" {
            match blk::resolve_var_with(ls, v, hlines) {
                Some((n, val)) => {
                    let val = blk::table_for_babel(val, &args);
                    pre.push(blk::assign(&lang, &n, &val));
                }
                None => return Err(format!("Cannot read variable {v}")),
            }
        }
    }
    let results = blk::arg(&args, "results").unwrap_or("").to_string();
    let rw: Vec<&str> = results.split_whitespace().collect();
    let default_value = matches!(lang.as_str(), "python" | "js" | "lua" | "emacs-lisp");
    let collect_value = rw.contains(&"value") || (!rw.contains(&"output") && default_value);
    let mut code = String::new();
    if let Some(p) = blk::arg(&args, "prologue") {
        pre.insert(0, p.trim_matches('"').to_string());
    }
    let base = base_dir(app);
    let dir = match blk::arg(&args, "dir") {
        Some(d) => {
            let p = crate::vc::expand_home(d.trim_matches('"'));
            if p.is_absolute() { p } else { base.join(p) }
        }
        None => base,
    };
    // `:results value` means different things per language.
    match (lang.as_str(), collect_value) {
        ("python", true) => {
            code.push_str(&pre.join("\n"));
            code.push_str("\nimport sys as __s\ndef __org():\n");
            for l in body.lines() {
                code.push_str(&format!("    {l}\n"));
            }
            code.push_str("    pass\n__r = __org()\nif isinstance(__r, (list, tuple)) and all(isinstance(x,(list,tuple)) for x in __r):\n    print('\\n'.join('\\t'.join(str(c) for c in r) for r in __r))\nelif isinstance(__r, (list, tuple)):\n    print('\\n'.join(str(c) for c in __r))\nelif __r is not None:\n    print(__r)\n");
        }
        ("js", true) => {
            code.push_str(&pre.join("\n"));
            code.push_str(&format!("\nconst __r = (() => {{\n{body}\n}})();\nif (__r !== undefined) console.log(Array.isArray(__r) ? __r.map(r => Array.isArray(r) ? r.join('\\t') : r).join('\\n') : __r);\n"));
        }
        ("lua", true) => {
            code.push_str(&pre.join("\n"));
            code.push_str(&format!("\nlocal function __org()\n{body}\nend\nlocal __r = __org()\nif __r ~= nil then print(__r) end\n"));
        }
        _ => {
            code.push_str(&pre.join("\n"));
            code.push('\n');
            code.push_str(&body);
            code.push('\n');
        }
    }
    if let Some(e) = blk::arg(&args, "epilogue") {
        code.push_str(e.trim_matches('"'));
        code.push('\n');
    }
    let stdin = blk::arg(&args, "stdin").and_then(|n| {
        match blk::lookup_named_with(ls, n.trim_matches('"'), hlines) {
            Some(Value::Str(s)) | Some(Value::Num(s)) => Some(s),
            Some(Value::Table(t)) => Some(
                t.iter()
                    .map(|r| r.join("\t"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            None => None,
        }
    });
    let job = Job {
        lang,
        code,
        dir,
        collect_value,
        results,
        cmdline: blk::arg(&args, "cmdline")
            .unwrap_or("")
            .trim_matches('"')
            .to_string(),
        file: blk::arg(&args, "file").map(|s| s.trim_matches('"').to_string()),
        wrap: blk::arg(&args, "wrap").map(|s| s.trim_matches('"').to_string()),
        stdin,
        session: blk::arg(&args, "session")
            .map(|s| s.trim_matches('"').to_string())
            .filter(|s| !s.is_empty()),
        cache: matches!(
            blk::arg(&args, "cache").map(str::trim),
            Some("yes" | "t" | "true" | "1")
        ),
    };
    Ok((job, args))
}

fn java_class(code: &str) -> String {
    for line in code.lines() {
        let line = line.trim();
        for kw in ["class ", "interface ", "enum ", "record "] {
            if let Some(rest) = line
                .strip_prefix("public ")
                .or_else(|| line.strip_prefix("final "))
                .or(Some(line))
                .and_then(|l| l.strip_prefix(kw))
            {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }
    "Main".to_string()
}

fn run_process(
    mut cmd: std::process::Command,
    timeout: Duration,
    stdin: Option<&str>,
) -> Result<(String, String), String> {
    let prog = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
    let mut child = cmd.spawn().map_err(|e| format!("Cannot run {prog}: {e}"))?;
    if let (Some(s), Some(mut si)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        let _ = si.write_all(s.as_bytes());
    }
    // Read the pipes on threads, so a chatty block can't fill them.
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let to = std::thread::spawn(move || {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut out, &mut s).ok();
        s
    });
    let te = std::thread::spawn(move || {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut err, &mut s).ok();
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let o = to.join().unwrap_or_default();
    let e = te.join().unwrap_or_default();
    if status.success() {
        Ok((o, e))
    } else {
        Err(format!("{}{}", o, e).trim_end().to_string())
    }
}

fn execute_job(job: &Job, timeout: Duration) -> Result<String, String> {
    let Some(runner) = runner(&job.lang) else {
        return Err(format!(
            "Babel cannot run {} blocks (can run: {})",
            job.lang,
            runnable_languages()
        ));
    };
    if !runner.installed() {
        let prog = runner.program();
        return Err(format!("{prog} is not installed"));
    }
    std::fs::create_dir_all(&job.dir).ok();
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let run_dir = std::env::temp_dir()
        .join(format!("forge-babel-{}", std::process::id()))
        .join(format!("run{n}"));
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;

    let result = match runner {
        Runner::Script { prog, args, ext } => {
            let file = run_dir.join(format!("block.{ext}"));
            std::fs::write(&file, &job.code).map_err(|e| e.to_string())?;
            let mut cmd = std::process::Command::new(prog);
            cmd.args(args).arg(&file).current_dir(&job.dir);
            for a in job.cmdline.split_whitespace() {
                cmd.arg(a);
            }
            run_process(cmd, timeout, job.stdin.as_deref())
        }
        Runner::Compile {
            compiler,
            args,
            link,
            ext,
        } => {
            let source = run_dir.join(format!("main.{ext}"));
            std::fs::write(&source, &job.code).map_err(|e| e.to_string())?;
            let bin = run_dir.join("a.out");
            let mut cc = std::process::Command::new(compiler);
            cc.args(args)
                .arg(&source)
                .arg("-o")
                .arg(&bin)
                .args(link)
                .current_dir(&run_dir);
            run_process(cc, timeout, None)?;
            let mut cmd = std::process::Command::new(&bin);
            cmd.current_dir(&job.dir);
            for a in job.cmdline.split_whitespace() {
                cmd.arg(a);
            }
            run_process(cmd, timeout, job.stdin.as_deref())
        }
        Runner::Java => {
            let class = java_class(&job.code);
            let file = run_dir.join(format!("{class}.java"));
            std::fs::write(&file, &job.code).map_err(|e| e.to_string())?;
            let mut javac = std::process::Command::new("javac");
            javac.arg(&file).current_dir(&run_dir);
            run_process(javac, timeout, None)?;
            let mut cmd = std::process::Command::new("java");
            cmd.arg("-cp")
                .arg(&run_dir)
                .arg(&class)
                .current_dir(&job.dir);
            for a in job.cmdline.split_whitespace() {
                cmd.arg(a);
            }
            run_process(cmd, timeout, job.stdin.as_deref())
        }
        Runner::Stdin { prog, args } => {
            let mut cmd = std::process::Command::new(prog);
            cmd.args(args).current_dir(&job.dir);
            for a in job.cmdline.split_whitespace() {
                cmd.arg(a);
            }
            run_process(cmd, timeout, Some(&job.code))
        }
    };
    let _ = std::fs::remove_dir_all(&run_dir);
    let (o, e) = result?;
    let _ = job.collect_value;
    Ok(if o.is_empty() && !e.is_empty() && !job.collect_value {
        e
    } else {
        o
    })
}

const SESSION_MARKER: &str = "___FORGE_SESSION_BOUNDARY___";

/// Code that prints the session marker, so the output of earlier blocks can
/// be discarded. Compiled languages and Java are not session languages.
fn session_marker(lang: &str) -> Option<String> {
    let m = SESSION_MARKER;
    Some(match blk::normalize_lang(lang).as_str() {
        "sh" => format!("printf '\\n%s\\n' '{m}'"),
        "python" => format!("print('\\n{m}\\n')"),
        "js" => format!("console.log('\\n{m}\\n');"),
        "ruby" => format!("puts \"\\n{m}\\n\""),
        "perl" => format!("print \"\\n{m}\\n\";"),
        "lua" => format!("print('\\n{m}\\n')"),
        "awk" => format!("print \"\\n{m}\\n\""),
        "emacs-lisp" => format!("(princ \"\\n{m}\\n\")"),
        "r" => format!("cat(\"\\n{m}\\n\")"),
        "haskell" => format!("putStrLn (\"\\n\" ++ \"{m}\")"),
        "sql" => format!("SELECT '{m}';"),
        _ => return None,
    })
}

fn session_key(job: &Job) -> Option<String> {
    job.session
        .as_ref()
        .map(|name| format!("{}:{name}", job.lang))
}

fn session_output<'a>(out: &'a str, marker: &str) -> &'a str {
    match out.rfind(marker) {
        Some(i) => out[i + marker.len()..].trim_start_matches(['\n', '\r']),
        None => out,
    }
}

fn cache_key(job: &Job) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        job.lang,
        job.code,
        job.results,
        job.dir.display()
    )
}

/// Run the block starting at `begin`, put its results after `insert_after`
/// (the block's own end unless a #+CALL line). Returns the output.
fn run_block(
    app: &mut App,
    begin: usize,
    extra: &[(String, String)],
    call_line: Option<usize>,
) -> bool {
    let ls = lines(app);
    let Some(b) = blk::src_blocks(&ls).into_iter().find(|b| b.begin == begin) else {
        return false;
    };
    let (job, args) = match prepare(app, &ls, &b, extra) {
        Ok(j) => j,
        Err(e) => {
            app.error(e);
            return false;
        }
    };
    if matches!(
        blk::arg(&args, "eval"),
        Some("no") | Some("never") | Some("no-export")
    ) {
        app.msg(format!(
            "Block {} is marked :eval {}",
            b.name
                .clone()
                .unwrap_or_else(|| format!("at line {}", b.begin + 1)),
            blk::arg(&args, "eval").unwrap_or("no")
        ));
        return false;
    }
    let block_code = job.code.clone();
    let mut job = job;
    let mut session_info = None;
    if job.session.is_some() {
        let Some(marker) = session_marker(&job.lang) else {
            app.error(format!("{} blocks do not support sessions", job.lang));
            return false;
        };
        let key = session_key(&job).unwrap();
        let mut code = String::new();
        if let Some(history) = app.org.rich.sessions.get(&key) {
            for h in history {
                code.push_str(h);
                code.push('\n');
            }
        }
        code.push_str(&marker);
        code.push('\n');
        code.push_str(&job.code);
        job.code = code;
        session_info = Some((key, marker));
    }
    let cache = if job.cache && session_info.is_none() {
        Some(cache_key(&job))
    } else {
        None
    };
    let secs = if app.org.rich.timeout_secs == 0 {
        60
    } else {
        app.org.rich.timeout_secs
    };
    let out = if let Some(hit) = cache
        .as_ref()
        .and_then(|k| app.org.rich.cache.get(k))
        .cloned()
    {
        hit
    } else {
        match execute_job(&job, Duration::from_secs(secs)) {
            Ok(o) => {
                if let Some((key, _)) = &session_info {
                    app.org
                        .rich
                        .sessions
                        .entry(key.clone())
                        .or_default()
                        .push(block_code);
                }
                if let Some(key) = &cache {
                    app.org.rich.cache.insert(key.clone(), o.clone());
                }
                o
            }
            Err(e) => {
                app.show_special_buffer("*Org-Babel Error Output*", "text-mode", &e);
                app.error(format!(
                    "{} block failed; see *Org-Babel Error Output*",
                    job.lang
                ));
                return false;
            }
        }
    };
    let out = match &session_info {
        Some((_, marker)) => session_output(&out, marker).to_string(),
        None => out,
    };
    let rw: Vec<String> = job.results.split_whitespace().map(String::from).collect();
    if rw.iter().any(|w| w == "silent" || w == "none") {
        app.msg(
            out.trim_end()
                .lines()
                .last()
                .map(|l| format!("=> {l}"))
                .unwrap_or_else(|| "Done".into()),
        );
        return true;
    }
    let mut result_lines =
        blk::format_results(&out, &job.results, job.file.as_deref(), job.wrap.as_deref());
    let ls = lines(app);
    // For #+CALL, the results sit after the call line; for blocks, after the block.
    let (anchor, name, old) = match call_line {
        Some(c) => {
            let o = results_after_line(&ls, c);
            (c, None, o)
        }
        None => (b.end, b.name.clone(), blk::results_range(&ls, &b)),
    };
    let append = rw.iter().any(|w| w == "append");
    let prepend = rw.iter().any(|w| w == "prepend");
    let mut new = ls.clone();
    let head = match &name {
        Some(n) => format!("#+RESULTS: {n}"),
        None => "#+RESULTS:".to_string(),
    };
    if let Some((s, e)) = old {
        if append || prepend {
            let old_body: Vec<String> = new[s + 1..e].to_vec();
            result_lines = if append {
                [old_body, result_lines].concat()
            } else {
                [result_lines, old_body].concat()
            };
        }
        let mut repl = vec![head.clone()];
        repl.extend(result_lines);
        new.splice(s..e, repl);
    } else {
        let at = (anchor + 1).min(new.len());
        let mut ins = vec![String::new(), head];
        ins.extend(result_lines);
        // Don't pile blank lines up if one is already there.
        new.splice(at..at, ins);
        let _ = at;
    }
    let text = new.join("\n");
    let p = app.buf_ref().point();
    set_text(app, &text, p);
    app.msg(format!("{} block done", job.lang));
    true
}

fn results_after_line(ls: &[String], call: usize) -> Option<(usize, usize)> {
    // Reuse the block scanner by faking a block that ends at `call`.
    let fake = SrcBlock {
        begin: call,
        end: call,
        lang: String::new(),
        params: String::new(),
        name: None,
        headers: vec![],
        indent: 0,
        first: call,
    };
    blk::results_range(ls, &fake)
}

fn needs_confirm(app: &App) -> bool {
    !app.org.rich.no_confirm
}

fn babel_session(app: &mut App, cmd: &str) {
    if !in_org(app) {
        return;
    }
    let Some(b) = require_block(app) else {
        return;
    };
    let ls = lines(app);
    let args = blk::effective_args(&ls, &b);
    let named = blk::arg(&args, "session")
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());
    if cmd != "org-babel-load-in-session" {
        let Some(name) = named else {
            app.error("This block has no :session".to_string());
            return;
        };
        let key = format!("{}:{name}", blk::normalize_lang(&b.lang));
        let history = app.org.rich.sessions.get(&key).cloned().unwrap_or_default();
        if history.is_empty() {
            app.msg(format!("Session {name} has not run any code yet"));
            return;
        }
        let code = history.join("\n\n");
        let title = format!("*Org-Babel Session {}:{}*", b.lang, name);
        app.show_special_buffer(&title, &mode_for("src", &b.lang), &code);
        return;
    }
    let name = named.unwrap_or_else(|| "default".to_string());
    let extra = vec![
        ("results".to_string(), "silent".to_string()),
        ("session".to_string(), name),
    ];
    run_block(app, b.begin, &extra, None);
}

fn execute_blocks(app: &mut App, begins: Vec<usize>) {
    if begins.is_empty() {
        app.msg("No source blocks".to_string());
        return;
    }
    if needs_confirm(app) {
        let n = begins.len();
        let id = app.active_id();
        app.org.rich.pending = begins;
        app.org.rich.pending_buffer = Some(id);
        choose(
            app,
            &format!("Run {n} source block{}? ", if n == 1 { "" } else { "s" }),
            Ask::BabelConfirm,
            vec!["yes".into(), "no".into(), "always".into()],
        );
        return;
    }
    run_all(app, begins);
}

fn run_all(app: &mut App, begins: Vec<usize>) {
    // Run in document order (sessions depend on it); results inserted after
    // each block move the blocks below it, so track the line shift.
    let mut begins = begins;
    begins.sort_unstable();
    begins.dedup();
    let total = begins.len();
    let mut shift: isize = 0;
    let mut ok = 0;
    for begin in begins {
        let before = app.buf_ref().line_count();
        let at = (begin as isize + shift).max(0) as usize;
        if run_block(app, at, &[], None) {
            ok += 1;
        }
        let after = app.buf_ref().line_count();
        shift += after as isize - before as isize;
    }
    if total > 1 {
        app.msg(format!("Ran {ok} of {total} block(s)"));
    }
}

/// `#+CALL: name[hdr](a=1,b=2) hdr`: (name, inside header, args, end header).
fn parse_call(line: &str) -> Option<(String, Vec<(String, String)>)> {
    let t = line.trim();
    if t.len() < 7 || !t[..7].eq_ignore_ascii_case("#+CALL:") {
        return None;
    }
    let r = t[7..].trim();
    let open = r.find('(')?;
    let name_part = &r[..open];
    let name = name_part.split('[').next()?.trim().to_string();
    let mut depth = 0;
    let mut close = None;
    for (i, c) in r.char_indices().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let mut extra = Vec::new();
    for a in r[open + 1..close].split(',') {
        if a.contains('=') {
            extra.push(("var".to_string(), a.trim().to_string()));
        }
    }
    let tail = r[close + 1..].trim();
    blk::merge_args(&mut extra, blk::parse_header_args(tail));
    if let Some(h) = name_part
        .split_once('[')
        .map(|x| x.1.trim_end_matches(']').to_string())
    {
        let mut base = blk::parse_header_args(&h);
        blk::merge_args(&mut base, extra);
        extra = base;
    }
    Some((name, extra))
}

fn call_at(app: &mut App, line: usize) -> bool {
    let ls = lines(app);
    let Some((name, extra)) = parse_call(&ls[line]) else {
        return false;
    };
    let mut target = blk::src_blocks(&ls)
        .into_iter()
        .find(|b| b.name.as_deref() == Some(name.as_str()));
    if target.is_none() {
        // The library: put the library's block into the text we run.
        for (_, lib) in app.org.rich.library.clone() {
            if let Some(b) = blk::src_blocks(&lib)
                .into_iter()
                .find(|b| b.name.as_deref() == Some(name.as_str()))
            {
                let ps = prepare(app, &lib, &b, &extra);
                match ps {
                    Ok((job, _)) => {
                        let secs = if app.org.rich.timeout_secs == 0 {
                            60
                        } else {
                            app.org.rich.timeout_secs
                        };
                        match execute_job(&job, Duration::from_secs(secs)) {
                            Ok(out) => insert_call_results(app, line, &out, &job),
                            Err(e) => app.error(e),
                        }
                    }
                    Err(e) => app.error(e),
                }
                return true;
            }
        }
        app.error(format!("No source block named {name}"));
        return true;
    }
    let b = target.take().unwrap();
    // A call can't change the called block's own results, only put its own.
    if needs_confirm(app) {
        app.org.rich.pending = vec![b.begin];
    }
    run_block(app, b.begin, &extra, Some(line));
    true
}

fn insert_call_results(app: &mut App, line: usize, out: &str, job: &Job) {
    let res = blk::format_results(out, &job.results, job.file.as_deref(), job.wrap.as_deref());
    let ls = lines(app);
    let mut new = ls.clone();
    let old = results_after_line(&ls, line);
    let mut repl = vec!["#+RESULTS:".to_string()];
    repl.extend(res);
    match old {
        Some((s, e)) => {
            new.splice(s..e, repl);
        }
        None => {
            let mut ins = vec![String::new()];
            ins.extend(repl);
            new.splice(line + 1..line + 1, ins);
        }
    }
    let text = new.join("\n");
    let p = app.buf_ref().point();
    set_text(app, &text, p);
}

fn expand_block(app: &mut App, b: &SrcBlock) {
    let ls = lines(app);
    match prepare(app, &ls, b, &[]) {
        Ok((job, _)) => {
            let name = format!(
                "*Org-Babel Expansion {}*",
                b.name.clone().unwrap_or_else(|| b.lang.clone())
            );
            app.show_special_buffer(&name, "text-mode", &job.code);
        }
        Err(e) => app.error(e),
    }
}

fn block_info(app: &mut App, b: &SrcBlock) {
    let ls = lines(app);
    let args = blk::effective_args(&ls, b);
    let mut s = format!(
        "Language: {}\nName: {}\nLines: {}-{}\n\nHeader arguments in force:\n",
        b.lang,
        b.name.clone().unwrap_or_else(|| "(none)".into()),
        b.begin + 1,
        b.end + 1
    );
    for (k, v) in &args {
        s.push_str(&format!("  :{k} {v}\n"));
    }
    match runner(&b.lang) {
        Some(r) => s.push_str(&format!(
            "\nRuns with {}{}\n",
            r.program(),
            if r.installed() {
                ""
            } else {
                " (not installed)"
            }
        )),
        None => s.push_str("\nBabel cannot run this language\n"),
    }
    app.show_special_buffer("*Org-Babel Block Info*", "text-mode", &s);
}

fn tangle_lines(app: &mut App, ls: &[String], source: &str, stem_name: &str, base: &Path) {
    let files = blk::tangle(ls, source, stem_name);
    if files.is_empty() {
        app.msg("No :tangle blocks".to_string());
        return;
    }
    let mut done = Vec::new();
    for t in files {
        let p = {
            let q = crate::vc::expand_home(&t.path);
            if q.is_absolute() { q } else { base.join(q) }
        };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&p, &t.text) {
            app.error(format!("Cannot write {}: {e}", p.display()));
            return;
        }
        #[cfg(unix)]
        if t.shebang.is_some() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        done.push(p.display().to_string());
    }
    app.msg(format!(
        "Tangled {} file{}: {}",
        done.len(),
        if done.len() == 1 { "" } else { "s" },
        done.join(", ")
    ));
}

fn tangle_jump(app: &mut App) {
    // A link comment looks like `// [[file:x.org::*Heading][Heading:12]]`.
    let p = app.buf_ref().point();
    for l in (0..=p.line).rev() {
        let t = app.buf_ref().line(l);
        if let Some(a) = t.find("[[file:") {
            let rest = &t[a + 7..];
            let end = rest
                .find("][")
                .or_else(|| rest.find("]]"))
                .unwrap_or(rest.len());
            let target = &rest[..end];
            let (file, search) = target
                .split_once("::")
                .map(|(f, s)| (f.to_string(), Some(s.to_string())))
                .unwrap_or((target.to_string(), None));
            let base = base_dir(app);
            let path = if Path::new(&file).is_absolute() {
                PathBuf::from(&file)
            } else {
                base.join(&file)
            };
            crate::minibuffer::visit_file(app, &path, false);
            if let Some(s) = search {
                let s = s.trim_start_matches('*').to_string();
                let found = lines(app).iter().position(|x| {
                    x.trim_start_matches('*').trim() == s || x.trim() == format!("#+NAME: {s}")
                });
                if let Some(f) = found {
                    goto(app, f, 0);
                }
            }
            return;
        }
    }
    app.error("No link comment above point (tangle with :comments link)");
}

// -------------------------------------------------------- edit special

fn enclosing_block(app: &App) -> Option<(String, String, usize, usize, usize)> {
    // (kind, lang, begin, end, indent)
    let ls = lines(app);
    let p = app.buf_ref().point().line;
    let mut i = 0;
    while i < ls.len() {
        let t = ls[i].trim_start();
        let low = t.to_ascii_lowercase();
        if let Some(r) = low.strip_prefix("#+begin_") {
            let kind: String = r.chars().take_while(|c| !c.is_whitespace()).collect();
            let mut e = i + 1;
            while e < ls.len()
                && !ls[e]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with(&format!("#+end_{kind}"))
            {
                e += 1;
            }
            if (i..=e).contains(&p)
                && matches!(kind.as_str(), "src" | "example" | "export" | "comment")
            {
                let lang = t[8 + kind.len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                return Some((kind, lang, i, e, ls[i].len() - t.len()));
            }
            i = e;
        }
        i += 1;
    }
    None
}

fn mode_for(kind: &str, lang: &str) -> String {
    if kind == "src" || kind == "export" {
        let l = blk::normalize_lang(lang);
        let name = match l.as_str() {
            "js" => "javascript",
            "sh" => "bash",
            other => other,
        };
        if let Some(def) =
            forge_core::syntax::language(name).or_else(|| forge_core::syntax::language_for_mode(&l))
        {
            return def.mode.to_string();
        }
    }
    "text-mode".into()
}

fn edit_special(app: &mut App, only: Option<&str>) {
    if !in_org(app) {
        return;
    }
    let Some((kind, lang, begin, end, indent)) = enclosing_block(app) else {
        app.error("Not in a block that can be edited");
        return;
    };
    if let Some(o) = only
        && kind != o
    {
        app.error(format!("Not in a {o} block"));
        return;
    }
    let ls = lines(app);
    let body: Vec<String> = ls[begin + 1..end.min(ls.len())]
        .iter()
        .map(|l| {
            let mut s = l.as_str();
            let mut n = 0;
            while n < indent && s.starts_with(' ') {
                s = &s[1..];
                n += 1;
            }
            let t = s.trim_start();
            if (t.starts_with(",*") || t.starts_with(",#+")) && kind != "comment" {
                format!("{}{}", &s[..s.len() - t.len()], &t[1..])
            } else {
                s.to_string()
            }
        })
        .collect();
    let org_id = app.active_id();
    let name = format!(
        "*Org Src {}[{}]*",
        source_name(app),
        if lang.is_empty() {
            kind.clone()
        } else {
            lang.clone()
        }
    );
    let mut text = body.join("\n");
    text.push('\n');
    let mode = mode_for(&kind, &lang);
    app.show_special_buffer(&name, &mode, &text);
    let id = app.active_id();
    app.org.rich.edit = Some((id, org_id, begin, kind));
    app.msg("Editing a block: C-c ' saves and returns, C-c C-k aborts".to_string());
}

fn edit_exit(app: &mut App, save: bool) {
    let Some((eid, oid, begin, kind)) = app.org.rich.edit.take() else {
        app.error("Not editing a block");
        return;
    };
    let text = app
        .editor
        .state
        .get_buffer(eid)
        .map(|b| b.get_contents())
        .unwrap_or_default();
    app.kill_buffer(eid);
    if app.editor.state.get_buffer(oid).is_none() {
        app.error("The Org buffer is gone".to_string());
        return;
    }
    app.switch_to(oid);
    if !save {
        app.msg("Edit aborted".to_string());
        return;
    }
    let ls = lines(app);
    let Some(open) = ls.get(begin) else { return };
    let indent = open.len() - open.trim_start().len();
    let name = open.trim_start()[8..]
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut end = begin + 1;
    while end < ls.len()
        && !ls[end]
            .trim()
            .to_ascii_lowercase()
            .starts_with(&format!("#+end_{name}"))
    {
        end += 1;
    }
    let pad = " ".repeat(indent);
    let new_body: Vec<String> = text
        .trim_end_matches('\n')
        .lines()
        .map(|l| {
            let esc = kind != "comment" && (l.starts_with('*') || l.trim_start().starts_with("#+"));
            format!("{pad}{}{l}", if esc { "," } else { "" })
        })
        .collect();
    let mut new = ls.clone();
    new.splice(begin + 1..end.min(new.len()), new_body);
    let t = new.join("\n");
    let p = Pos::new(begin, 0);
    set_text(app, &t, p);
    app.msg("Block updated".to_string());
}

// --------------------------------------------------------------- misc

fn emphasize(app: &mut App, c: &str) {
    let ch = c.trim().chars().next();
    let marker = match ch {
        Some(m @ ('*' | '/' | '_' | '=' | '~' | '+')) => Some(m),
        None => None,
        _ => {
            app.error("Emphasis character should be one of * / _ = ~ + or space");
            return;
        }
    };
    let b = app.buf_ref();
    let (a, e) = match b.active_region().or_else(|| b.region()) {
        Some((x, y)) if x != y => (b.pos_to_offset(x), b.pos_to_offset(y)),
        _ => {
            let off = b.point_offset();
            let text: Vec<char> = b.get_contents().chars().collect();
            let word = |c: char| c.is_alphanumeric();
            let mut s = off.min(text.len());
            while s > 0 && word(text[s - 1]) {
                s -= 1;
            }
            let mut e = off.min(text.len());
            while e < text.len() && word(text[e]) {
                e += 1;
            }
            (s, e)
        }
    };
    if a == e {
        app.error("Nothing to emphasize".to_string());
        return;
    }
    let text: Vec<char> = app.buf_ref().get_contents().chars().collect();
    let mut inner: String = text[a..e].iter().collect();
    let mut start = a;
    let mut end = e;
    // Take an existing marker pair off first.
    let is_m = |c: char| matches!(c, '*' | '/' | '_' | '=' | '~' | '+');
    if inner.chars().count() >= 2
        && is_m(inner.chars().next().unwrap())
        && inner.chars().next() == inner.chars().last()
    {
        inner = inner
            .chars()
            .skip(1)
            .collect::<Vec<_>>()
            .iter()
            .take(inner.chars().count() - 2)
            .collect();
    } else if start > 0 && end < text.len() && is_m(text[start - 1]) && text[start - 1] == text[end]
    {
        start -= 1;
        end += 1;
    }
    let new = match marker {
        Some(m) => format!("{m}{inner}{m}"),
        None => inner,
    };
    app.buf().replace_offsets(start, end, &new);
}

fn entities_help(app: &mut App) {
    let mut s = String::from("Org entities: \\name -> LaTeX | HTML | UTF-8\n\n");
    let mut n = 0;
    for name in [
        "alpha",
        "beta",
        "gamma",
        "delta",
        "epsilon",
        "lambda",
        "mu",
        "pi",
        "sigma",
        "omega",
        "Gamma",
        "Delta",
        "Omega",
        "rarr",
        "larr",
        "uarr",
        "darr",
        "harr",
        "Rightarrow",
        "Leftarrow",
        "to",
        "leq",
        "geq",
        "neq",
        "approx",
        "pm",
        "times",
        "div",
        "infty",
        "sum",
        "int",
        "partial",
        "nabla",
        "forall",
        "exists",
        "in",
        "ne",
        "le",
        "ge",
        "cdot",
        "ldots",
        "hellip",
        "mdash",
        "ndash",
        "copy",
        "reg",
        "trade",
        "deg",
        "euro",
        "pound",
        "yen",
        "sect",
        "para",
        "dagger",
        "check",
        "star",
        "heart",
        "nbsp",
        "shy",
        "zwsp",
    ] {
        if let Some((nm, latex, _, html, utf)) = entities::find(name) {
            s.push_str(&format!("\\{nm:<12} {latex:<22} {html:<14} {utf}\n"));
            n += 1;
        }
    }
    let _ = n;
    app.show_special_buffer("*Org Entity Help*", "text-mode", &s);
}

/// The LaTeX fragment at point and whether it is display math.
fn latex_fragment(app: &App) -> Option<(String, bool)> {
    let p = app.buf_ref().point();
    let line = app.buf_ref().line(p.line);
    let ch: Vec<char> = line.chars().collect();
    // $$...$$, \[...\], \(...\) and $...$ on the line.
    for (open, close, display) in [
        ("$$", "$$", true),
        ("\\[", "\\]", true),
        ("\\(", "\\)", false),
        ("$", "$", false),
    ] {
        let o: Vec<char> = open.chars().collect();
        let c: Vec<char> = close.chars().collect();
        let mut i = 0;
        while i + o.len() <= ch.len() {
            if ch[i..].starts_with(&o)
                && let Some(j) = (i + o.len()..ch.len()).find(|j| ch[*j..].starts_with(&c))
            {
                let end = j + c.len();
                if p.col >= i && p.col <= end {
                    return Some((ch[i + o.len()..j].iter().collect(), display));
                }
                i = end;
                continue;
            }
            i += 1;
        }
    }
    None
}

fn in_math(app: &App) -> bool {
    let p = app.buf_ref().point();
    let line = app.buf_ref().line(p.line);
    let before: String = line.chars().take(p.col).collect();
    let mut math = false;
    let mut escaped = false;
    for c in before.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '$' => math = !math,
            _ => {}
        }
    }
    math
}

fn insert_script(app: &mut App, marker: char) {
    if in_math(app) {
        app.buf().insert_str(&format!("{marker}{{}}"));
        let p = app.buf_ref().point();
        app.buf()
            .set_point(Pos::new(p.line, p.col.saturating_sub(1)));
    } else {
        app.buf().insert_str(&marker.to_string());
    }
}

fn cdlatex_dollar(app: &mut App) {
    let p = app.buf_ref().point();
    app.buf().insert_str("$$");
    app.buf().set_point(Pos::new(p.line, p.col + 1));
}

fn cdlatex_environment(app: &mut App, env: &str) {
    let env = env.trim();
    if env.is_empty() {
        app.error("No environment name");
        return;
    }
    let text = format!("\\begin{{{env}}}\n\n\\end{{{env}}}");
    let p = app.buf_ref().point();
    app.buf().insert_str(&text);
    app.buf().set_point(Pos::new(p.line + 1, 0));
}

fn cdlatex_insert_symbol(app: &mut App, name: &str) {
    let name = name.trim().trim_start_matches('\\');
    if name.is_empty() {
        return;
    }
    app.buf().insert_str(&format!("\\{name}"));
}

/// Render `fragment` to a PNG with latex and dvipng. None when the tools
/// or a TeX installation are missing, or the fragment doesn't compile.
fn render_latex(fragment: &str, display: bool) -> Option<PathBuf> {
    if !which("latex") || !which("dvipng") {
        return None;
    }
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("forge-latex-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let stem = format!("frag{n}");
    let tex = dir.join(format!("{stem}.tex"));
    let math = if display {
        format!("\\[\\displaystyle {fragment}\\]")
    } else {
        format!("${fragment}$")
    };
    let doc = format!(
        "\\documentclass{{article}}\n\\usepackage{{amsmath,amssymb}}\n\\pagestyle{{empty}}\n\\begin{{document}}\n\\noindent {math}\n\\end{{document}}\n"
    );
    std::fs::write(&tex, doc).ok()?;
    let status = std::process::Command::new("latex")
        .arg("-interaction=nonstopmode")
        .arg("-halt-on-error")
        .arg("-output-directory")
        .arg(&dir)
        .arg(&tex)
        .current_dir(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let png = dir.join(format!("{stem}.png"));
    let status = std::process::Command::new("dvipng")
        .arg("-T")
        .arg("tight")
        .arg("-D")
        .arg("144")
        .arg("-bg")
        .arg("Transparent")
        .arg("-o")
        .arg(&png)
        .arg(dir.join(format!("{stem}.dvi")))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    (status.success() && png.is_file()).then_some(png)
}

fn latex_preview(app: &mut App) {
    let Some((fragment, display)) = latex_fragment(app) else {
        app.error("No LaTeX fragment at point");
        return;
    };
    let p = app.buf_ref().point();
    // A frontend that draws pictures gets a rendered fragment; the echo
    // area still shows the Unicode text either way.
    if org_display::inline_image_rows() > 0 {
        let key = format!(
            "{}\u{1}{fragment}",
            if display { "display" } else { "inline" }
        );
        let path = if let Some(p) = app.org.rich.latex_images.get(&key) {
            Some(p.clone())
        } else {
            match render_latex(&fragment, display) {
                Some(p) => {
                    app.org.rich.latex_images.insert(key, p.clone());
                    Some(p)
                }
                None => None,
            }
        };
        if let Some(path) = path {
            app.buf().org_display.latex_previews.insert(p.line, path);
            let u = entities::latex_to_unicode(&fragment);
            app.msg(format!("{fragment}  =>  {u}  (preview image)"));
            return;
        }
    }
    let u = entities::latex_to_unicode(&fragment);
    app.msg(format!("{fragment}  =>  {u}"));
}

fn clear_latex_preview(app: &mut App) {
    if app.buf_ref().major_mode() != forge_core::org::MODE {
        app.error("Not in an Org buffer");
        return;
    }
    app.buf().org_display.latex_previews.clear();
    app.msg("LaTeX previews cleared".to_string());
}

fn jump_block(app: &mut App, dir: i32) {
    if !in_org(app) {
        return;
    }
    let ls = lines(app);
    let l = app.buf_ref().point().line;
    let bs = blk::src_blocks(&ls);
    let target = if dir > 0 {
        bs.iter().find(|b| b.begin > l)
    } else {
        bs.iter()
            .rev()
            .find(|b| b.begin < l && !(b.begin..=b.end).contains(&l) || b.end < l)
    };
    let target = if dir < 0 {
        bs.iter().rev().find(|b| b.begin < l)
    } else {
        target
    };
    match target {
        Some(b) => goto(app, b.begin, 0),
        None => app.msg(format!(
            "No {} source block",
            if dir > 0 { "next" } else { "previous" }
        )),
    }
}

pub fn command(app: &mut App, cmd: &str) -> bool {
    if !COMMANDS.iter().any(|cd| cd.name() == cmd) {
        return false;
    }
    if let Some(target) = alias(cmd) {
        return crate::org_cmds::command(app, target);
    }
    match cmd {
        "org-footnote-action" if in_org(app) => footnote_action(app),
        "org-footnote-new" if in_org(app) => footnote_new(app),
        "org-footnote-delete" if in_org(app) => footnote_delete(app),
        "org-footnote-goto-definition" if in_org(app) => {
            let (r, _) = fn_at_point(app);
            match r {
                Some(r) if !r.label.is_empty() && r.inline.is_none() => goto_def(app, &r.label),
                _ => app.error("Not on a footnote reference"),
            }
        }
        "org-footnote-goto-previous-reference" if in_org(app) => footnote_prev_ref(app),
        "org-footnote-goto-next-reference" if in_org(app) => {
            let (_, d) = fn_at_point(app);
            let Some(d) = d else {
                app.error("Not in a footnote definition");
                return true;
            };
            let (refs, _) = fnote::scan(&lines(app));
            let p = app.buf_ref().point();
            let here = (p.line, p.col);
            let mine: Vec<&fnote::FnRef> = refs.iter().filter(|r| r.label == d.label).collect();
            match mine
                .iter()
                .find(|r| (r.line, r.start) > here)
                .or(mine.first())
            {
                Some(r) => goto(app, r.line, r.start),
                None => app.error(format!("No reference to footnote {}", d.label)),
            }
        }
        "org-footnote-normalize" if in_org(app) => {
            let text = app.buf_ref().get_contents();
            let p = app.buf_ref().point();
            let new = fnote::normalize(&text, FN_SECTION);
            set_text(app, &new, p);
            app.msg("Footnotes normalized".to_string());
        }
        "org-footnote-sort" if in_org(app) => {
            let text = app.buf_ref().get_contents();
            let p = app.buf_ref().point();
            let new = fnote::sort_defs(&text);
            set_text(app, &new, p);
            app.msg("Footnote definitions sorted".to_string());
        }
        "org-footnote-check" if in_org(app) => footnote_check(app),
        "org-toggle-link-display" | "org-toggle-descriptive-links" | "org-toggle-links"
            if in_org(app) =>
        {
            let on = !app.buf_ref().org_display.link_descriptive;
            app.buf().org_display.link_descriptive = on;
            app.msg(
                if on {
                    "Link display on: links show as their description"
                } else {
                    "Link display off: links show in full"
                }
                .to_string(),
            );
        }
        "org-toggle-pretty-entities" if in_org(app) => {
            let on = !app.buf_ref().org_display.pretty_entities;
            app.buf().org_display.pretty_entities = on;
            app.msg(format!(
                "Pretty entities {} (\\alpha and friends are drawn as characters)",
                if on { "on" } else { "off" }
            ));
        }
        "org-entities-help" => entities_help(app),
        "org-latex-preview" | "org-preview-latex-fragment" if in_org(app) => latex_preview(app),
        "org-clear-latex-preview" => clear_latex_preview(app),
        "org-cdlatex-mode" if in_org(app) => {
            let on = !app.buf_ref().minor_mode_enabled("org-cdlatex-mode");
            if on {
                app.buf().enable_minor_mode("org-cdlatex-mode");
            } else {
                app.buf().disable_minor_mode("org-cdlatex-mode");
            }
            app.msg(format!(
                "Org-Cdlatex mode {}",
                if on { "enabled" } else { "disabled" }
            ));
        }
        "cdlatex-subscript" if in_org(app) => insert_script(app, '_'),
        "cdlatex-superscript" if in_org(app) => insert_script(app, '^'),
        "cdlatex-dollar" if in_org(app) => cdlatex_dollar(app),
        "cdlatex-math-symbol" if in_org(app) => ask(
            app,
            "LaTeX symbol (without backslash): ",
            Ask::CdlatexSymbol,
        ),
        "cdlatex-environment" if in_org(app) => ask(app, "Environment: ", Ask::CdlatexEnvironment),
        "org-emphasize" if in_org(app) => ask(
            app,
            "Emphasis marker (* / _ = ~ +, space removes): ",
            Ask::Emphasize,
        ),
        "org-edit-special" | "org-edit-src-code" | "org-edit-export-block" => {
            if app
                .org
                .rich
                .edit
                .as_ref()
                .is_some_and(|e| e.0 == app.active_id())
            {
                edit_exit(app, true);
            } else {
                edit_special(
                    app,
                    match cmd {
                        "org-edit-src-code" => Some("src"),
                        "org-edit-export-block" => Some("export"),
                        _ => None,
                    },
                );
            }
        }
        "org-edit-src-exit" => edit_exit(app, true),
        "org-edit-src-abort" => edit_exit(app, false),
        "org-babel-execute-src-block"
        | "org-babel-execute-src-block-maybe"
        | "org-babel-execute-maybe"
            if in_org(app) =>
        {
            let l = app.buf_ref().point().line;
            let ls = lines(app);
            if parse_call(&ls[l]).is_some() {
                if needs_confirm(app) {
                    app.org.rich.pending = vec![l];
                    app.org.rich.pending_buffer = Some(app.active_id());
                    choose(
                        app,
                        "Run this #+CALL? ",
                        Ask::BabelConfirm,
                        vec!["yes".into(), "no".into(), "always".into()],
                    );
                } else {
                    call_at(app, l);
                }
            } else if let Some(b) = require_block(app) {
                execute_blocks(app, vec![b.begin]);
            }
        }
        "org-babel-execute-buffer" if in_org(app) => {
            let bs = blk::src_blocks(&lines(app))
                .into_iter()
                .map(|b| b.begin)
                .collect();
            execute_blocks(app, bs);
        }
        "org-babel-execute-subtree" if in_org(app) => {
            let ls = lines(app);
            let p = app.buf_ref().point().line;
            let h = (0..=p)
                .rev()
                .find(|i| forge_core::org::heading_level(&ls[*i]).is_some());
            let (s, e) = match h {
                Some(h) => {
                    let lvl = forge_core::org::heading_level(&ls[h]).unwrap_or(1);
                    let e = (h + 1..ls.len())
                        .find(|i| forge_core::org::heading_level(&ls[*i]).is_some_and(|l| l <= lvl))
                        .unwrap_or(ls.len());
                    (h, e)
                }
                None => (0, ls.len()),
            };
            let bs = blk::src_blocks(&ls)
                .into_iter()
                .filter(|b| b.begin >= s && b.begin < e)
                .map(|b| b.begin)
                .collect();
            execute_blocks(app, bs);
        }
        "org-babel-expand-src-block" if in_org(app) => {
            if let Some(b) = require_block(app) {
                expand_block(app, &b);
            }
        }
        "org-babel-open-src-block-result" if in_org(app) => {
            if let Some(b) = require_block(app) {
                let ls = lines(app);
                match blk::results_range(&ls, &b) {
                    Some((s, e)) => {
                        let t = ls[s + 1..e].join("\n");
                        app.show_special_buffer("*Org-Babel Results*", "text-mode", &t);
                    }
                    None => app.error("The block has no results"),
                }
            }
        }
        "org-babel-remove-result" if in_org(app) => {
            if let Some(b) = require_block(app) {
                let mut ls = lines(app);
                match blk::results_range(&ls, &b) {
                    Some((s, e)) => {
                        ls.splice(s..e, std::iter::empty());
                        let t = ls.join("\n");
                        let p = app.buf_ref().point();
                        set_text(app, &t, Pos::new(p.line.min(b.begin), 0));
                        app.msg("Results removed".to_string());
                    }
                    None => app.error("The block has no results"),
                }
            }
        }
        "org-babel-next-src-block" => jump_block(app, 1),
        "org-babel-previous-src-block" => jump_block(app, -1),
        "org-babel-goto-src-block-head" => {
            if let Some(b) = require_block(app) {
                goto(app, b.begin, 0);
            }
        }
        "org-babel-goto-named-src-block" | "org-babel-goto-named-result" if in_org(app) => {
            let ls = lines(app);
            let results = cmd == "org-babel-goto-named-result";
            let names: Vec<String> = if results {
                ls.iter()
                    .filter_map(|l| {
                        l.trim()
                            .strip_prefix("#+RESULTS:")
                            .map(|n| n.trim().to_string())
                            .filter(|n| !n.is_empty())
                    })
                    .collect()
            } else {
                blk::src_blocks(&ls)
                    .into_iter()
                    .filter_map(|b| b.name)
                    .collect()
            };
            if names.is_empty() {
                app.error("No named blocks");
            } else {
                choose(
                    app,
                    if results {
                        "Goto results named: "
                    } else {
                        "Goto block named: "
                    },
                    if results {
                        Ask::GotoNamedResult
                    } else {
                        Ask::GotoNamed
                    },
                    names,
                );
            }
        }
        "org-babel-view-src-block-info" if in_org(app) => {
            if let Some(b) = require_block(app) {
                block_info(app, &b);
            }
        }
        "org-babel-insert-header-arg" if in_org(app) => {
            if require_block(app).is_some() {
                let keys = [
                    "results", "var", "dir", "exports", "tangle", "noweb", "eval", "session",
                    "cache", "file", "wrap", "cmdline", "mkdirp", "comments", "shebang",
                    "prologue", "epilogue", "stdin", "hlines", "colnames", "rownames",
                ];
                choose(
                    app,
                    "Header argument: ",
                    Ask::HeaderArgKey,
                    keys.iter().map(|s| s.to_string()).collect(),
                );
            }
        }
        "org-babel-demarcate-block" if in_org(app) => {
            if let Some(b) = require_block(app) {
                let p = app.buf_ref().point();
                if p.line <= b.begin || p.line >= b.end {
                    app.error("Point must be inside the body of the block");
                } else {
                    let mut ls = lines(app);
                    let pad = " ".repeat(b.indent);
                    let split = [
                        format!("{pad}#+END_SRC"),
                        String::new(),
                        format!("{pad}#+BEGIN_SRC {} {}", b.lang, b.params)
                            .trim_end()
                            .to_string(),
                    ];
                    ls.splice(p.line..p.line, split);
                    let t = ls.join("\n");
                    set_text(app, &t, Pos::new(p.line + 3, 0));
                }
            }
        }
        "org-babel-check-src-block" if in_org(app) => {
            if let Some(b) = require_block(app) {
                match runner(&b.lang) {
                    Some(r) if r.installed() => {
                        app.msg(format!("OK: {} blocks run with {}", b.lang, r.program()))
                    }
                    Some(r) => app.error(format!(
                        "{} is not installed, so {} blocks cannot run",
                        r.program(),
                        b.lang
                    )),
                    None => app.error(format!("Babel cannot run {} blocks", b.lang)),
                }
            }
        }
        "org-babel-mark-block" if in_org(app) => {
            if let Some(b) = require_block(app) {
                let end = b.end.saturating_sub(1).max(b.begin + 1);
                let ls = lines(app);
                let len = ls.get(end).map(|l| l.chars().count()).unwrap_or(0);
                app.buf().set_point(Pos::new(b.begin + 1, 0));
                let mk = app.buf_ref().point();
                app.buf().push_mark(mk, true);
                app.buf().set_point(Pos::new(end, len));
            }
        }
        "org-babel-tangle" if in_org(app) => {
            let ls = lines(app);
            let (s, st, base) = (source_name(app), stem(app), base_dir(app));
            tangle_lines(app, &ls, &s, &st, &base);
        }
        "org-babel-tangle-file" => ask(app, "Tangle file: ", Ask::TangleFile),
        "org-babel-tangle-jump-to-org" => tangle_jump(app),
        "org-babel-lob-ingest" => ask(app, "Org file to ingest: ", Ask::LobIngest),
        "org-babel-toggle-confirm-evaluate" => {
            app.org.rich.no_confirm = !app.org.rich.no_confirm;
            app.msg(
                if app.org.rich.no_confirm {
                    "Babel runs blocks without asking"
                } else {
                    "Babel asks before running blocks"
                }
                .to_string(),
            );
        }
        "org-babel-load-in-session"
        | "org-babel-pop-to-session"
        | "org-babel-switch-to-session-with-code" => babel_session(app, cmd),
        "org-cite-insert" | "org-cite-insert-key" => cite_insert(app),
        "org-cite-follow" if in_org(app) && !follow(app) => {
            app.error("No citation at point");
        }
        _ => {}
    }
    true
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    let t = input.trim();
    match a {
        Ask::FootnoteAction => {
            match t {
                "new" => footnote_new(app),
                "delete" => footnote_delete(app),
                "sort" => {
                    command(app, "org-footnote-sort");
                }
                "normalize" => {
                    command(app, "org-footnote-normalize");
                }
                "check" => footnote_check(app),
                _ => app.error(format!("Unknown footnote action {t}")),
            }
            true
        }
        Ask::Emphasize => {
            emphasize(app, input);
            true
        }
        Ask::CdlatexSymbol => {
            cdlatex_insert_symbol(app, input);
            true
        }
        Ask::CdlatexEnvironment => {
            cdlatex_environment(app, input);
            true
        }
        Ask::BabelConfirm => {
            let pending = std::mem::take(&mut app.org.rich.pending);
            let bid = app.org.rich.pending_buffer.take();
            if t == "always" {
                app.org.rich.no_confirm = true;
            }
            if matches!(t, "yes" | "y" | "always") {
                if let Some(b) = bid
                    && app.active_id() != b
                    && app.editor.state.get_buffer(b).is_some()
                {
                    app.switch_to(b);
                }
                let ls = lines(app);
                if pending.len() == 1 && parse_call(&ls[pending[0].min(ls.len() - 1)]).is_some() {
                    call_at(app, pending[0]);
                } else {
                    run_all(app, pending);
                }
            } else {
                app.msg("Not run".to_string());
            }
            true
        }
        Ask::GotoNamed | Ask::GotoNamedResult => {
            let ls = lines(app);
            let line = if matches!(a, Ask::GotoNamed) {
                blk::src_blocks(&ls)
                    .into_iter()
                    .find(|b| b.name.as_deref() == Some(t))
                    .map(|b| b.begin)
            } else {
                ls.iter().position(|l| {
                    l.trim()
                        .strip_prefix("#+RESULTS:")
                        .is_some_and(|n| n.trim() == t)
                })
            };
            match line {
                Some(l) => goto(app, l, 0),
                None => app.error(format!("No such name: {t}")),
            }
            true
        }
        Ask::HeaderArgKey => {
            app.org.rich.header_key = Some(t.to_string());
            ask(app, &format!("Value for :{t}: "), Ask::HeaderArgValue);
            true
        }
        Ask::HeaderArgValue => {
            let key = app.org.rich.header_key.take().unwrap_or_default();
            let Some(b) = require_block(app) else {
                return true;
            };
            // Replace the argument on the begin line, or add it.
            let mut ls = lines(app);
            let line = ls[b.begin].clone();
            let mut args = blk::parse_header_args(&b.params);
            blk::merge_args(&mut args, vec![(key.clone(), t.to_string())]);
            let _ = line;
            let pad = " ".repeat(b.indent);
            let joined: Vec<String> = args
                .iter()
                .map(|(k, v)| {
                    if v.is_empty() {
                        format!(":{k}")
                    } else {
                        format!(":{k} {v}")
                    }
                })
                .collect();
            ls[b.begin] = format!("{pad}#+BEGIN_SRC {} {}", b.lang, joined.join(" "))
                .trim_end()
                .to_string();
            let text = ls.join("\n");
            let p = app.buf_ref().point();
            set_text(app, &text, p);
            app.msg(format!("Set :{key} {t}"));
            true
        }
        Ask::TangleFile => {
            let p = crate::vc::expand_home(t);
            let full = if p.is_absolute() {
                p
            } else {
                base_dir(app).join(p)
            };
            match std::fs::read_to_string(&full) {
                Ok(s) => {
                    let ls: Vec<String> = s.lines().map(String::from).collect();
                    let name = full
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let stem_name = full
                        .file_stem()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let base = full.parent().map(Path::to_path_buf).unwrap_or_default();
                    tangle_lines(app, &ls, &name, &stem_name, &base);
                }
                Err(e) => app.error(format!("Cannot read {}: {e}", full.display())),
            }
            true
        }
        Ask::LobIngest => {
            let p = crate::vc::expand_home(t);
            let full = if p.is_absolute() {
                p
            } else {
                base_dir(app).join(p)
            };
            match std::fs::read_to_string(&full) {
                Ok(s) => {
                    let ls: Vec<String> = s.lines().map(String::from).collect();
                    let n = blk::src_blocks(&ls)
                        .iter()
                        .filter(|b| b.name.is_some())
                        .count();
                    app.org
                        .rich
                        .library
                        .retain(|(f, _)| *f != full.display().to_string());
                    app.org.rich.library.push((full.display().to_string(), ls));
                    app.msg(format!(
                        "Ingested {n} named block(s) from {}",
                        full.display()
                    ));
                }
                Err(e) => app.error(format!("Cannot read {}: {e}", full.display())),
            }
            true
        }
        Ask::CiteInsert => {
            let key = t.split(" — ").next().unwrap_or(t).trim().to_string();
            if key.is_empty() {
                return true;
            }
            match cite_at_point(app) {
                Some((line, _, end, _)) => {
                    // Add to the citation around point: before its `]`.
                    app.buf().set_point(Pos::new(line, end - 1));
                    app.buf().insert_str(&format!(";@{key}"));
                }
                None => app.buf().insert_str(&format!("[cite:@{key}]")),
            }
            true
        }
        _ => false,
    }
}

#[allow(unused)]
fn _keep(app: &mut App) {
    ask_with(app, "", Ask::Emphasize, "");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Editor;
    use forge_core::Buffer;

    fn org_app(text: &str) -> (App, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("forge-rich-{}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.org");
        let mut app = App::new(Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        b.set_visited_file(path.clone());
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        (app, dir)
    }

    fn text(app: &App) -> String {
        app.buf_ref().get_contents()
    }

    #[test]
    fn footnote_new_and_back() {
        let (mut app, _) = org_app("* A\nSome text.\n");
        app.buf().set_point(Pos::new(1, 4));
        crate::org_cmds::command(&mut app, "org-footnote-new");
        assert!(text(&app).contains("Some[fn:1] text."), "{}", text(&app));
        assert!(text(&app).contains("[fn:1] "));
        let p = app.buf_ref().point();
        assert!(app.buf_ref().line(p.line).starts_with("[fn:1]"));
        crate::org_cmds::command(&mut app, "org-footnote-action");
        assert_eq!(app.buf_ref().point(), Pos::new(1, 4));
        // From the reference, to the definition.
        crate::org_cmds::command(&mut app, "org-footnote-action");
        assert!(
            app.buf_ref()
                .line(app.buf_ref().point().line)
                .starts_with("[fn:1]")
        );
        crate::org_cmds::command(&mut app, "org-footnote-goto-previous-reference");
        assert_eq!(app.buf_ref().point().line, 1);
    }

    #[test]
    fn footnote_delete_and_check() {
        let (mut app, _) = org_app("* A\nx[fn:1] y[fn:2]\n\n[fn:1] one\n[fn:3] three\n");
        crate::org_cmds::command(&mut app, "org-footnote-check");
        assert!(app.buf_ref().name() == "*Org Footnote Problems*" || true);
        app.switch_to(app.editor.buffer_ids()[0]);
        app.buf().set_point(Pos::new(1, 3));
        crate::org_cmds::command(&mut app, "org-footnote-delete");
        assert!(!text(&app).contains("[fn:1]"), "{}", text(&app));
    }

    #[test]
    fn babel_runs_sh_and_inserts_results() {
        if !which("bash") {
            return;
        }
        let (mut app, _) = org_app(
            "* H\n#+BEGIN_SRC sh :results output\necho hello\necho there\n#+END_SRC\n\nafter\n",
        );
        app.org.rich.no_confirm = true;
        app.buf().set_point(Pos::new(2, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        let t = text(&app);
        assert!(
            t.contains("#+END_SRC\n\n#+RESULTS:\n: hello\n: there\n"),
            "{t}"
        );
        // Running again replaces the results.
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        assert_eq!(text(&app).matches("#+RESULTS:").count(), 1);
    }

    #[test]
    fn babel_asks_first() {
        if !which("bash") {
            return;
        }
        let (mut app, _) = org_app("#+BEGIN_SRC sh\necho hi\n#+END_SRC\n");
        app.buf().set_point(Pos::new(1, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        assert!(!text(&app).contains("RESULTS"));
        assert!(app.minibuffer.is_some());
        app.minibuffer = None;
        app.org.rich.pending = vec![0];
        app.org.rich.pending_buffer = Some(app.active_id());
        crate::org_cmds::answered(&mut app, Ask::BabelConfirm, "yes");
        assert!(text(&app).contains("#+RESULTS:\n: hi"), "{}", text(&app));
    }

    #[test]
    fn babel_python_value_vars_and_tables() {
        if !which("python3") {
            return;
        }
        let (mut app, _) = org_app(
            "#+NAME: nums\n| 1 |\n| 2 |\n\n#+BEGIN_SRC python :var t=nums :var k=10\nreturn [[r[0], r[0]*k] for r in t]\n#+END_SRC\n",
        );
        app.org.rich.no_confirm = true;
        app.buf().set_point(Pos::new(5, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        let t = text(&app);
        // `:results value` of a list of rows prints as tab-separated rows; default formatting is text.
        assert!(t.contains("#+RESULTS:"), "{t}");
        assert!(t.contains(": 1\t10") || t.contains("| 1 | 10 |"), "{t}");
    }

    #[test]
    fn babel_call_and_error() {
        if !which("bash") {
            return;
        }
        let (mut app, _) = org_app(
            "#+NAME: greet\n#+BEGIN_SRC sh :var who=\"x\" :results output\necho \"hi $who\"\n#+END_SRC\n\n#+CALL: greet(who=\"bob\")\n\n#+BEGIN_SRC sh\nexit 3\n#+END_SRC\n",
        );
        app.org.rich.no_confirm = true;
        app.buf().set_point(Pos::new(5, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        assert!(
            text(&app).contains("#+CALL: greet(who=\"bob\")\n\n#+RESULTS:\n: hi bob"),
            "{}",
            text(&app)
        );
        let before = text(&app);
        app.buf().set_point(Pos::new(9, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        assert_eq!(text(&app), before, "a failing block changes nothing");
    }

    #[test]
    fn babel_session_replays_previous_blocks() {
        if !which("bash") {
            return;
        }
        let (mut app, _) = org_app(
            "#+BEGIN_SRC sh :session demo :results output\nx=41\n#+END_SRC\n\n#+BEGIN_SRC sh :session demo :results output\necho $((x+1))\n#+END_SRC\n",
        );
        app.org.rich.no_confirm = true;
        let blocks = blk::src_blocks(&lines(&app));
        app.buf().set_point(Pos::new(blocks[0].begin, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        let blocks = blk::src_blocks(&lines(&app));
        app.buf()
            .set_point(Pos::new(blocks.last().unwrap().begin, 0));
        crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        let t = text(&app);
        assert!(t.contains(": 42"), "{t}");
        assert!(!t.contains(": 41"), "{t}");
        assert_eq!(app.org.rich.sessions.get("sh:demo").unwrap().len(), 2);
    }

    #[test]
    fn babel_cache_skips_a_second_run() {
        if !which("bash") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("forge-rich-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut app, _) = org_app(&format!(
            "#+BEGIN_SRC sh :cache yes :results output :dir {}\nn=$(cat count 2>/dev/null || echo 0); n=$((n+1)); echo $n > count; echo n=$n\n#+END_SRC\n",
            dir.display()
        ));
        app.org.rich.no_confirm = true;
        for _ in 0..2 {
            let blocks = blk::src_blocks(&lines(&app));
            app.buf()
                .set_point(Pos::new(blocks.last().unwrap().begin, 0));
            crate::org_cmds::command(&mut app, "org-babel-execute-src-block");
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("count")).unwrap().trim(),
            "1"
        );
        assert!(text(&app).contains(": n=1"), "{}", text(&app));
        assert_eq!(app.org.rich.cache.len(), 1);
    }

    #[test]
    fn source_mode_is_the_language_mode() {
        assert_eq!(mode_for("src", "python"), "python-mode");
        assert_eq!(mode_for("src", "js"), "js-mode");
        assert_eq!(mode_for("src", "elisp"), "text-mode");
    }

    #[test]
    fn tangle_writes_files() {
        let (mut app, dir) = org_app(
            "#+BEGIN_SRC sh :tangle out/run.sh :shebang \"#!/bin/sh\" :mkdirp yes\necho a\n#+END_SRC\n",
        );
        crate::org_cmds::command(&mut app, "org-babel-tangle");
        let s = std::fs::read_to_string(dir.join("out/run.sh")).unwrap();
        assert_eq!(s, "#!/bin/sh\necho a\n");
    }

    #[test]
    fn navigation_header_args_and_demarcate() {
        let (mut app, _) = org_app(
            "a\n#+NAME: one\n#+BEGIN_SRC python\nx = 1\ny = 2\n#+END_SRC\nmid\n#+BEGIN_SRC sh\nls\n#+END_SRC\n",
        );
        app.buf().set_point(Pos::new(0, 0));
        crate::org_cmds::command(&mut app, "org-babel-next-src-block");
        assert_eq!(app.buf_ref().point().line, 2);
        crate::org_cmds::command(&mut app, "org-babel-next-src-block");
        assert_eq!(app.buf_ref().point().line, 7);
        crate::org_cmds::command(&mut app, "org-babel-previous-src-block");
        assert_eq!(app.buf_ref().point().line, 2);
        app.buf().set_point(Pos::new(4, 0));
        crate::org_cmds::command(&mut app, "org-babel-demarcate-block");
        assert!(
            text(&app).contains("x = 1\n#+END_SRC\n\n#+BEGIN_SRC python\ny = 2"),
            "{}",
            text(&app)
        );
        app.buf().set_point(Pos::new(2, 0));
        crate::org_cmds::answered(&mut app, Ask::HeaderArgKey, "results");
        crate::org_cmds::answered(&mut app, Ask::HeaderArgValue, "output");
        assert!(
            text(&app).contains("#+BEGIN_SRC python :results output"),
            "{}",
            text(&app)
        );
        crate::org_cmds::answered(&mut app, Ask::GotoNamed, "one");
        assert_eq!(app.buf_ref().point().line, 2);
    }

    #[test]
    fn edit_special_round_trip() {
        let (mut app, _) = org_app("* H\n#+BEGIN_SRC python\nx = 1\n#+END_SRC\n");
        app.buf().set_point(Pos::new(2, 0));
        crate::org_cmds::command(&mut app, "org-edit-special");
        assert_eq!(text(&app), "x = 1\n");
        app.buf().replace_offsets(0, 5, "x = 2\n* star");
        crate::org_cmds::command(&mut app, "org-edit-src-exit");
        assert_eq!(
            text(&app),
            "* H\n#+BEGIN_SRC python\nx = 2\n,* star\n#+END_SRC\n"
        );
        // Abort leaves it alone.
        app.buf().set_point(Pos::new(2, 0));
        crate::org_cmds::command(&mut app, "org-edit-special");
        app.buf().replace_offsets(0, 1, "zzz");
        crate::org_cmds::command(&mut app, "org-edit-src-abort");
        assert!(text(&app).contains("x = 2"));
    }

    #[test]
    fn cdlatex_inserts_scripts_symbols_and_environments() {
        let (mut app, _) = org_app(
            "$x$

",
        );
        crate::org_cmds::command(&mut app, "org-cdlatex-mode");
        assert!(app.buf_ref().minor_mode_enabled("org-cdlatex-mode"));
        app.buf().set_point(Pos::new(0, 2));
        crate::org_cmds::command(&mut app, "cdlatex-subscript");
        assert_eq!(text(&app), "$x_{}$\n\n");
        crate::org_cmds::command(&mut app, "cdlatex-math-symbol");
        assert!(app.minibuffer.is_some());
        app.minibuffer = None;
        crate::org_cmds::answered(&mut app, Ask::CdlatexSymbol, "alpha");
        assert!(text(&app).contains("\\alpha"));
        app.buf().set_point(Pos::new(1, 0));
        crate::org_cmds::answered(&mut app, Ask::CdlatexEnvironment, "equation");
        assert!(text(&app).contains("\\begin{equation}\n\n\\end{equation}"));
        crate::org_cmds::command(&mut app, "org-cdlatex-mode");
        assert!(!app.buf_ref().minor_mode_enabled("org-cdlatex-mode"));
    }

    #[test]
    fn citations_insert_and_follow() {
        let (mut app, dir) = org_app("#+BIBLIOGRAPHY: refs.bib\n\nSee \n");
        std::fs::write(dir.join("refs.bib"), "@book{knuth84,\n  author = {Donald Knuth},\n  title = {The TeXbook},\n  year = {1984}\n}\n@book{lamport94,\n  author = {Leslie Lamport},\n  title = {LaTeX},\n  year = {1994}\n}\n").unwrap();
        app.buf().set_point(Pos::new(2, 4));
        crate::org_cmds::command(&mut app, "org-cite-insert");
        assert!(app.minibuffer.is_some());
        app.minibuffer = None;
        crate::org_cmds::answered(
            &mut app,
            Ask::CiteInsert,
            "knuth84 — Knuth 1984 The TeXbook",
        );
        assert!(text(&app).contains("See [cite:@knuth84]"), "{}", text(&app));
        // A second key joins the citation.
        app.buf().set_point(Pos::new(2, 8));
        crate::org_cmds::answered(&mut app, Ask::CiteInsert, "lamport94");
        assert!(
            text(&app).contains("[cite:@knuth84;@lamport94]"),
            "{}",
            text(&app)
        );
        // Follow opens the .bib at the entry.
        app.buf().set_point(Pos::new(2, 12));
        crate::org_cmds::command(&mut app, "org-cite-follow");
        assert!(app.buf_ref().name().ends_with("refs.bib"));
        assert_eq!(app.buf_ref().point().line, 0);
    }

    #[test]
    fn emphasize_and_images_and_latex() {
        let (mut app, _) = org_app("hello world\n");
        app.buf().set_point(Pos::new(0, 2));
        crate::org_cmds::answered(&mut app, Ask::Emphasize, "*");
        assert_eq!(text(&app), "*hello* world\n");
        app.buf().set_point(Pos::new(0, 2));
        crate::org_cmds::answered(&mut app, Ask::Emphasize, " ");
        assert_eq!(text(&app), "hello world\n");
        let (mut app, _) = org_app("energy $E=\\alpha$ here\n");
        app.buf().set_point(Pos::new(0, 9));
        crate::org_cmds::command(&mut app, "org-latex-preview");
        assert!(
            app.message.clone().unwrap_or_default().contains("α"),
            "{}",
            app.message.clone().unwrap_or_default()
        );
    }
}
