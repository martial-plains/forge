// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org export commands: `org-export-dispatch` and the per-backend
//! `org-*-export-to-*` / `org-*-export-as-*` commands (the engine is in
//! `forge_core::export`).

use crate::app::App;
use crate::org_cmds::{Ask, choose, now};
use forge_core::export::{self, Backend, Output, Settings};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What an export does with its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    File,
    Buffer,
    FileOpen,
    Pdf,
    PdfOpen,
    Info,
}

/// The dispatcher's switches.
#[derive(Debug, Default, Clone)]
pub struct State {
    pub subtree: bool,
    pub body_only: bool,
    /// The last export, for `org-export-repeat`.
    pub last: Option<(String, Act)>,
}

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "org-export-dispatch",
        "Org: choose an export (C-u: just the subtree at point).",
    )
    .body("Also toggles for subtree and body-only exports."),
    forge_docs::CommandDoc::line("org-export-repeat", "Org: do the last export again."),
    forge_docs::CommandDoc::new(
        "org-export-insert-default-template",
        "Org: insert the export header (title, author, date,",
    )
    .body("options) at the top of the buffer."),
    forge_docs::CommandDoc::line(
        "org-ascii-export-to-ascii",
        "Org export: write the buffer as plain ASCII text to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-ascii-export-as-ascii",
        "Org export: show the buffer as plain ASCII text in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-utf8-export-to-ascii",
        "Org export: write the buffer as plain UTF-8 text to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-utf8-export-as-ascii",
        "Org export: show the buffer as plain UTF-8 text in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-html-export-to-html",
        "Org export: write the buffer as HTML to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-html-export-as-html",
        "Org export: show the buffer as HTML in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-html-export-to-html-and-open",
        "Org export: write the buffer as HTML to a file and open it.",
    ),
    forge_docs::CommandDoc::line(
        "org-icalendar-export-to-ics",
        "Org export: write the buffer's dated entries as an iCalendar file.",
    ),
    forge_docs::CommandDoc::line(
        "org-icalendar-export-agenda-files",
        "Org export: write an iCalendar file next to each agenda file.",
    ),
    forge_docs::CommandDoc::line(
        "org-icalendar-combine-agenda-files",
        "Org export: write one iCalendar file with the entries of all the agenda files.",
    ),
    forge_docs::CommandDoc::line(
        "org-latex-export-to-latex",
        "Org export: write the buffer as LaTeX to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-latex-export-as-latex",
        "Org export: show the buffer as LaTeX in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-latex-export-to-pdf",
        "Org export: write the buffer as LaTeX and make a PDF with latexmk or pdflatex.",
    ),
    forge_docs::CommandDoc::line(
        "org-latex-export-to-pdf-and-open",
        "Org export: make a PDF of the buffer and open it.",
    ),
    forge_docs::CommandDoc::line(
        "org-beamer-export-to-latex",
        "Org export: write the buffer as a Beamer presentation (LaTeX) to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-beamer-export-as-latex",
        "Org export: show the buffer as a Beamer presentation (LaTeX) in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-beamer-export-to-pdf",
        "Org export: make a PDF presentation of the buffer with Beamer.",
    ),
    forge_docs::CommandDoc::line(
        "org-md-export-to-markdown",
        "Org export: write the buffer as Markdown to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-md-export-as-markdown",
        "Org export: show the buffer as Markdown in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-odt-export-to-odt",
        "Org export: write the buffer as an OpenDocument text file.",
    ),
    forge_docs::CommandDoc::line(
        "org-odt-export-to-odt-and-open",
        "Org export: write the buffer as an OpenDocument text file and open it.",
    ),
    forge_docs::CommandDoc::new(
        "org-org-export-to-org",
        "Org export: write the buffer as Org with includes and macros expanded and",
    )
    .body("noexport parts removed."),
    forge_docs::CommandDoc::line(
        "org-org-export-as-org",
        "Org export: show the buffer as expanded Org in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-texinfo-export-to-texinfo",
        "Org export: write the buffer as Texinfo to a file.",
    ),
    forge_docs::CommandDoc::line(
        "org-texinfo-export-as-texinfo",
        "Org export: show the buffer as Texinfo in a buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-texinfo-export-to-info",
        "Org export: write the buffer as Texinfo and make an Info file with makeinfo.",
    ),
    forge_docs::CommandDoc::line(
        "org-html-convert-region-to-html",
        "Org export: replace the active region with its HTML export.",
    ),
    forge_docs::CommandDoc::line(
        "org-md-convert-region-to-md",
        "Org export: replace the active region with its Markdown export.",
    ),
    forge_docs::CommandDoc::line(
        "org-export-region-to-html",
        "Org export: replace the active region with its HTML export.",
    ),
    forge_docs::CommandDoc::line(
        "org-export-region-to-md",
        "Org export: replace the active region with its Markdown export.",
    ),
    forge_docs::CommandDoc::line(
        "org-odt-convert",
        "Org export: convert an ODT file to another format with LibreOffice.",
    ),
];

/// (keys, backend, action, label) for the dispatcher.
const ACTIONS: &[(&str, &str, Act, &str)] = &[
    ("t a", "ascii", Act::File, "ASCII text to file"),
    ("t A", "ascii", Act::Buffer, "ASCII text to buffer"),
    ("t u", "utf8", Act::File, "UTF-8 text to file"),
    ("t U", "utf8", Act::Buffer, "UTF-8 text to buffer"),
    ("h h", "html", Act::File, "HTML to file"),
    ("h H", "html", Act::Buffer, "HTML to buffer"),
    ("h o", "html", Act::FileOpen, "HTML to file and open"),
    ("c f", "icalendar", Act::File, "iCalendar to file"),
    (
        "c i",
        "icalendar-agenda",
        Act::File,
        "iCalendar for each agenda file",
    ),
    (
        "c c",
        "icalendar-combine",
        Act::File,
        "iCalendar combining the agenda files",
    ),
    ("l l", "latex", Act::File, "LaTeX to file"),
    ("l L", "latex", Act::Buffer, "LaTeX to buffer"),
    ("l p", "latex", Act::Pdf, "LaTeX to PDF file"),
    ("l o", "latex", Act::PdfOpen, "LaTeX to PDF file and open"),
    ("l P", "beamer", Act::File, "Beamer to LaTeX file"),
    ("l B", "beamer", Act::Buffer, "Beamer to LaTeX buffer"),
    ("l O", "beamer", Act::PdfOpen, "Beamer to PDF file and open"),
    ("m m", "md", Act::File, "Markdown to file"),
    ("m M", "md", Act::Buffer, "Markdown to buffer"),
    ("m o", "md", Act::FileOpen, "Markdown to file and open"),
    ("o o", "odt", Act::File, "OpenDocument text to file"),
    (
        "o O",
        "odt",
        Act::FileOpen,
        "OpenDocument text to file and open",
    ),
    ("O o", "org", Act::File, "Org to file"),
    ("O O", "org", Act::Buffer, "Org to buffer"),
    ("i t", "texinfo", Act::File, "Texinfo to file"),
    ("i T", "texinfo", Act::Buffer, "Texinfo to buffer"),
    ("i i", "texinfo", Act::Info, "Texinfo to Info file"),
];

fn base_dir(app: &App) -> PathBuf {
    app.buf_ref()
        .path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn resolve(base: &Path, f: &str) -> PathBuf {
    let p = crate::vc::expand_home(f);
    if p.is_absolute() { p } else { base.join(p) }
}

fn settings(app: &App, body_only: bool) -> Settings {
    let (d, _) = now();
    let base = base_dir(app);
    let src = app
        .buf_ref()
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| app.buf_ref().name().to_string());
    Settings {
        body_only,
        source: src,
        today: format!("{:04}-{:02}-{:02}", d.y, d.m, d.d),
        images: Some(Rc::new(move |f: &str| {
            std::fs::read(resolve(&base, f)).ok()
        })),
    }
}

/// Dispatcher entries as shown in the prompt.
fn candidates(st: &State) -> Vec<String> {
    let mut c = vec![
        format!(
            "[{}] subtree only  (toggle)",
            if st.subtree { "x" } else { " " }
        ),
        format!(
            "[{}] body only  (toggle)",
            if st.body_only { "x" } else { " " }
        ),
    ];
    for (k, b, _, label) in ACTIONS {
        let _ = b;
        c.push(format!("{k}  {label}"));
    }
    c
}

fn dispatch(app: &mut App) {
    if app.buf_ref().major_mode() != forge_core::org::MODE {
        app.error("Not in an Org buffer".to_string());
        return;
    }
    let st = app.org.export.clone();
    choose(
        app,
        "Export (key or choice): ",
        Ask::ExportDispatch,
        candidates(&st),
    );
}

fn find_action(input: &str) -> Option<(&'static str, Act)> {
    let t = input.trim();
    let toks: Vec<&str> = t.split_whitespace().collect();
    // "h h", "hh" or a full candidate line "h h  HTML to file".
    let keys = if toks.len() >= 2 && toks[0].chars().count() == 1 && toks[1].chars().count() == 1 {
        Some(format!("{} {}", toks[0], toks[1]))
    } else if toks.len() == 1 && toks[0].chars().count() == 2 {
        let mut c = toks[0].chars();
        Some(format!("{} {}", c.next()?, c.next()?))
    } else {
        None
    };
    if let Some(k) = keys
        && let Some((_, b, a, _)) = ACTIONS.iter().find(|(kk, ..)| *kk == k)
    {
        return Some((b, *a));
    }
    ACTIONS
        .iter()
        .find(|(_, _, _, label)| t.ends_with(label))
        .map(|(_, b, a, _)| (*b, *a))
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    match a {
        Ask::ExportDispatch => {
            let t = input.trim();
            if t.contains("subtree only") {
                app.org.export.subtree = !app.org.export.subtree;
                dispatch(app);
                return true;
            }
            if t.contains("body only") {
                app.org.export.body_only = !app.org.export.body_only;
                dispatch(app);
                return true;
            }
            match find_action(t) {
                Some((b, act)) => run(app, b, act),
                None => app.error(format!("No such export: {t}")),
            }
            true
        }
        Ask::OdtFile => {
            let t = input.trim();
            if !t.is_empty() {
                choose_odt_format(app, t.to_string());
            }
            true
        }
        Ask::OdtFormat { file } => {
            let t = input.trim();
            let fmt = if t.is_empty() { "pdf" } else { t };
            odt_convert(app, file, fmt);
            true
        }
        _ => false,
    }
}

/// The per-backend commands.
fn named(cmd: &str) -> Option<(&'static str, Act)> {
    Some(match cmd {
        "org-ascii-export-to-ascii" => ("ascii", Act::File),
        "org-ascii-export-as-ascii" => ("ascii", Act::Buffer),
        "org-utf8-export-to-ascii" => ("utf8", Act::File),
        "org-utf8-export-as-ascii" => ("utf8", Act::Buffer),
        "org-html-export-to-html" => ("html", Act::File),
        "org-html-export-as-html" => ("html", Act::Buffer),
        "org-html-export-to-html-and-open" => ("html", Act::FileOpen),
        "org-icalendar-export-to-ics" => ("icalendar", Act::File),
        "org-icalendar-export-agenda-files" => ("icalendar-agenda", Act::File),
        "org-icalendar-combine-agenda-files" => ("icalendar-combine", Act::File),
        "org-latex-export-to-latex" => ("latex", Act::File),
        "org-latex-export-as-latex" => ("latex", Act::Buffer),
        "org-latex-export-to-pdf" => ("latex", Act::Pdf),
        "org-latex-export-to-pdf-and-open" => ("latex", Act::PdfOpen),
        "org-beamer-export-to-latex" => ("beamer", Act::File),
        "org-beamer-export-as-latex" => ("beamer", Act::Buffer),
        "org-beamer-export-to-pdf" => ("beamer", Act::Pdf),
        "org-md-export-to-markdown" => ("md", Act::File),
        "org-md-export-as-markdown" => ("md", Act::Buffer),
        "org-odt-export-to-odt" => ("odt", Act::File),
        "org-odt-export-to-odt-and-open" => ("odt", Act::FileOpen),
        "org-org-export-to-org" => ("org", Act::File),
        "org-org-export-as-org" => ("org", Act::Buffer),
        "org-texinfo-export-to-texinfo" => ("texinfo", Act::File),
        "org-texinfo-export-as-texinfo" => ("texinfo", Act::Buffer),
        "org-texinfo-export-to-info" => ("texinfo", Act::Info),
        _ => return None,
    })
}

/// Replace the active region with its export to `backend` (body only),
/// the `org-export-region-to-*` / `org-*-convert-region-to-*` commands.
fn convert_region(app: &mut App, backend: &str) {
    let Some((a, b)) = app.buf_ref().active_region() else {
        app.error("No active region to convert".to_string());
        return;
    };
    let Some(be) = export::backend(backend) else {
        app.error(format!("No export backend called {backend}"));
        return;
    };
    let text = app.buf_ref().text_range(a, b);
    let s = settings(app, true);
    let read = reader(app);
    let out = match export::export(be.name, &text, &read, &s) {
        Ok(o) => o,
        Err(e) => {
            app.error(e);
            return;
        }
    };
    let Output::Text(t) = out else {
        app.error(format!("{} cannot convert a region", be.label));
        return;
    };
    app.buf().replace_range(a, b, &t);
    app.buf().deactivate_mark();
    app.msg(format!("Converted region to {}", be.label));
}

/// Formats org-odt-convert offers (LibreOffice's `--convert-to` names).
const ODT_FORMATS: &[&str] = &["pdf", "doc", "docx", "html", "fodt", "rtf", "txt", "epub"];

fn choose_odt_format(app: &mut App, file: String) {
    choose(
        app,
        "Convert ODT to: ",
        Ask::OdtFormat { file },
        ODT_FORMATS.iter().map(|f| f.to_string()).collect(),
    );
}

/// `org-odt-convert`: run LibreOffice's headless converter on `file`.
fn odt_convert(app: &mut App, file: &str, fmt: &str) {
    let path = crate::vc::expand_home(file);
    if !path.is_file() {
        app.error(format!("No such file: {}", path.display()));
        return;
    }
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let prog = if which("soffice") {
        "soffice"
    } else if which("libreoffice") {
        "libreoffice"
    } else {
        app.error("soffice (LibreOffice) is not installed".to_string());
        return;
    };
    match run_tool(&dir, prog, &["--headless", "--convert-to", fmt, &name]) {
        Ok(()) => {
            let out = path.with_extension(fmt);
            app.msg(format!("Converted to {}", out.display()));
        }
        Err(e) => app.show_special_buffer("*Org ODT Convert Output*", "text-mode", &e),
    }
}

pub fn command(app: &mut App, cmd: &str) -> bool {
    if let Some((b, act)) = named(cmd) {
        if app.buf_ref().major_mode() != forge_core::org::MODE {
            app.error("Not in an Org buffer".to_string());
            return true;
        }
        if app.has_arg() {
            app.org.export.subtree = true;
        }
        run(app, b, act);
        return true;
    }
    match cmd {
        "org-export-dispatch" => {
            if app.has_arg() {
                app.org.export.subtree = true;
            }
            dispatch(app);
        }
        "org-export-repeat" => match app.org.export.last.clone() {
            Some((b, act)) => run(app, &b, act),
            None => app.error("No export to repeat".to_string()),
        },
        "org-html-convert-region-to-html" | "org-export-region-to-html" => {
            convert_region(app, "html")
        }
        "org-md-convert-region-to-md" | "org-export-region-to-md" => convert_region(app, "md"),
        "org-odt-convert" => {
            let f = app
                .buf_ref()
                .path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if f.to_ascii_lowercase().ends_with(".odt") {
                choose_odt_format(app, f);
            } else {
                let initial = if f.is_empty() {
                    String::new()
                } else {
                    Path::new(&f)
                        .with_extension("odt")
                        .to_string_lossy()
                        .to_string()
                };
                crate::org_cmds::ask_with(app, "ODT file: ", Ask::OdtFile, &initial);
            }
        }
        "org-export-insert-default-template" => {
            let name = app.buf_ref().name().to_string();
            let title = name
                .rsplit_once('.')
                .map(|(a, _)| a.to_string())
                .unwrap_or(name);
            let (d, _) = now();
            let author = std::env::var("USER").unwrap_or_default();
            let t = export::default_template(
                &title,
                &author,
                &format!("{:04}-{:02}-{:02}", d.y, d.m, d.d),
            );
            app.buf().set_point(forge_core::Pos::new(0, 0));
            app.buf().insert_str(&t);
            app.msg("Inserted the export template".to_string());
        }
        _ => return false,
    }
    true
}

/// The export source: the whole buffer, or the subtree at point.
fn source(app: &mut App) -> Option<String> {
    let text = app.buf_ref().get_contents();
    if app.org.export.subtree {
        let line = app.buf_ref().point().line;
        match export::subtree_source(&text, line) {
            Some(s) => Some(s),
            None => {
                app.error("Point is not in a subtree".to_string());
                None
            }
        }
    } else {
        Some(text)
    }
}

fn reader(app: &App) -> impl Fn(&str) -> Option<String> + use<> {
    let base = base_dir(app);
    move |f: &str| std::fs::read_to_string(resolve(&base, f)).ok()
}

fn buffer_name(b: &Backend) -> String {
    format!(
        "*Org {} Export*",
        b.label.split(" (").next().unwrap_or(b.name)
    )
}

fn open_path(app: &mut App, p: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener)
        .arg(p)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {}
        Err(e) => app.error(format!("Cannot open {}: {e}", p.display())),
    }
}

fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(prog).is_file()))
}

/// Run `prog args` in `dir`; the output on failure.
fn run_tool(dir: &Path, prog: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Cannot run {prog}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let tail: Vec<&str> = log.lines().rev().take(25).collect();
        Err(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
    }
}

fn run(app: &mut App, backend_name: &str, act: Act) {
    let st = app.org.export.clone();
    app.org.export.last = Some((backend_name.to_string(), act));
    if backend_name == "icalendar-agenda" || backend_name == "icalendar-combine" {
        return agenda_calendars(app, backend_name == "icalendar-combine");
    }
    let Some(b) = export::backend(backend_name) else {
        app.error(format!("No export backend called {backend_name}"));
        return;
    };
    let Some(text) = source(app) else { return };
    let s = settings(app, st.body_only);
    let read = reader(app);
    let out = match export::export(b.name, &text, &read, &s) {
        Ok(o) => o,
        Err(e) => {
            app.error(e);
            return;
        }
    };
    if act == Act::Buffer {
        let Output::Text(t) = out else {
            app.error(format!("{} cannot be shown in a buffer", b.label));
            return;
        };
        let mode = if b.name == "org" {
            forge_core::org::MODE
        } else {
            "text-mode"
        };
        app.show_special_buffer(&buffer_name(b), mode, &t);
        app.msg(format!("Exported to {}", buffer_name(b)));
        return;
    }
    let base = base_dir(app);
    let src = s.source.clone();
    let name = export::output_name(&src, &text, b);
    let path = {
        let p = PathBuf::from(&name);
        if p.is_absolute() {
            p
        } else if Path::new(&src).is_absolute() {
            Path::new(&src).parent().map(|d| d.join(&name)).unwrap_or(p)
        } else {
            base.join(&name)
        }
    };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = std::fs::write(&path, out.bytes()) {
        app.error(format!("Cannot write {}: {e}", path.display()));
        return;
    }
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| base.clone());
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    match act {
        Act::File => app.msg(format!("Exported to {}", path.display())),
        Act::FileOpen => {
            open_path(app, &path);
            app.msg(format!("Exported to {} and opened", path.display()));
        }
        Act::Pdf | Act::PdfOpen => {
            let pdf = path.with_extension("pdf");
            let res = if which("latexmk") {
                run_tool(
                    &dir,
                    "latexmk",
                    &["-pdf", "-interaction=nonstopmode", "-halt-on-error", &file],
                )
            } else if which("pdflatex") {
                run_tool(
                    &dir,
                    "pdflatex",
                    &["-interaction=nonstopmode", "-halt-on-error", &file],
                )
                .and_then(|_| run_tool(&dir, "pdflatex", &["-interaction=nonstopmode", &file]))
            } else {
                Err("Neither latexmk nor pdflatex is installed".to_string())
            };
            match res {
                Ok(()) => {
                    if act == Act::PdfOpen {
                        open_path(app, &pdf);
                    }
                    app.msg(format!("Exported to {}", pdf.display()));
                }
                Err(e) => {
                    app.show_special_buffer("*Org PDF LaTeX Output*", "text-mode", &e);
                    app.error(format!(
                        "PDF export failed; the LaTeX file is {}",
                        path.display()
                    ));
                }
            }
        }
        Act::Info => {
            let info = path.with_extension("info");
            if !which("makeinfo") {
                app.error(format!(
                    "makeinfo is not installed; the Texinfo file is {}",
                    path.display()
                ));
                return;
            }
            match run_tool(
                &dir,
                "makeinfo",
                &["--no-split", "-o", &info.to_string_lossy(), &file],
            ) {
                Ok(()) => app.msg(format!("Exported to {}", info.display())),
                Err(e) => {
                    app.show_special_buffer("*Org Texinfo Output*", "text-mode", &e);
                    app.error("makeinfo failed".to_string());
                }
            }
        }
        Act::Buffer => {}
    }
}

fn agenda_calendars(app: &mut App, combine: bool) {
    let files = crate::org_flow::agenda_files(app);
    if files.is_empty() {
        app.error("There are no agenda files".to_string());
        return;
    }
    let (d, _) = now();
    let s = Settings {
        today: format!("{:04}-{:02}-{:02}", d.y, d.m, d.d),
        ..Default::default()
    };
    let mut parts = Vec::new();
    let mut written = 0;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let dir = f.parent().map(Path::to_path_buf).unwrap_or_default();
        let read = move |n: &str| std::fs::read_to_string(resolve(&dir, n)).ok();
        let Ok(Output::Text(ics)) = export::export("icalendar", &text, &read, &s) else {
            continue;
        };
        if combine {
            parts.push(ics);
        } else {
            let p = f.with_extension("ics");
            if std::fs::write(&p, ics).is_ok() {
                written += 1;
            }
        }
    }
    if combine {
        let dir = app
            .org
            .directory
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_default();
        let p = dir.join("agenda.ics");
        match std::fs::write(&p, export::combine_calendars(&parts)) {
            Ok(()) => app.msg(format!(
                "Exported {} agenda files to {}",
                parts.len(),
                p.display()
            )),
            Err(e) => app.error(format!("Cannot write {}: {e}", p.display())),
        }
    } else {
        app.msg(format!("Wrote {written} calendar files"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{Buffer, Pos};

    fn app_with(text: &str, path: &Path) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        b.set_visited_file(path.to_path_buf());
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-export-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn export_to_file_writes_next_to_the_source() {
        let dir = tmp("file");
        let mut app = app_with("#+TITLE: Hi\n* A\ntext *b*\n", &dir.join("t.org"));
        assert!(command(&mut app, "org-html-export-to-html"));
        let h = std::fs::read_to_string(dir.join("t.html")).unwrap();
        assert!(h.contains("<h1 class=\"title\">Hi</h1>") && h.contains("<b>b</b>"));
        assert!(command(&mut app, "org-md-export-to-markdown"));
        assert!(
            std::fs::read_to_string(dir.join("t.md"))
                .unwrap()
                .contains("# Hi")
        );
        assert!(command(&mut app, "org-odt-export-to-odt"));
        let z = std::fs::read(dir.join("t.odt")).unwrap();
        assert!(
            forge_core::export::common::unzip(&z)
                .iter()
                .any(|(n, _)| n == "content.xml")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_to_buffer_shows_the_result() {
        let dir = tmp("buf");
        let mut app = app_with("* A\ntext\n", &dir.join("t.org"));
        assert!(command(&mut app, "org-latex-export-as-latex"));
        assert!(app.buf_ref().name().starts_with("*Org LaTeX Export"));
        assert!(app.buf_ref().get_contents().contains("\\section{A}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn includes_resolve_relative_to_the_file() {
        let dir = tmp("inc");
        std::fs::write(dir.join("part.org"), "included words\n").unwrap();
        let mut app = app_with("#+INCLUDE: \"part.org\"\n", &dir.join("t.org"));
        command(&mut app, "org-ascii-export-as-ascii");
        assert!(app.buf_ref().get_contents().contains("included words"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subtree_export_uses_only_that_subtree() {
        let dir = tmp("sub");
        let mut app = app_with("* One\nfirst\n* Two\nsecond\n", &dir.join("t.org"));
        app.buf().set_point(Pos::new(3, 0));
        app.org.export.subtree = true;
        command(&mut app, "org-ascii-export-as-ascii");
        let t = app.buf_ref().get_contents();
        assert!(t.contains("second") && !t.contains("first"), "{t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn convert_region_replaces_the_active_region() {
        let dir = tmp("convert");
        let mut app = app_with("*bold*\n* A\nkeep\n", &dir.join("t.org"));
        app.buf().set_point(Pos::new(0, 0));
        app.buf().push_mark(Pos::new(1, 0), true);
        assert!(command(&mut app, "org-html-convert-region-to-html"));
        let t = app.buf_ref().get_contents();
        assert!(t.contains("<b>bold</b>"), "{t}");
        assert!(!t.contains("*bold*") && t.contains("* A"), "{t}");
        assert!(!app.buf_ref().mark_active());
        // No active region: the command refuses and changes nothing.
        let before = app.buf_ref().get_contents();
        command(&mut app, "org-md-convert-region-to-md");
        assert_eq!(app.buf_ref().get_contents(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn odt_convert_prompts_for_the_file_and_format() {
        let dir = tmp("odtconv");
        // A buffer visiting a .odt file asks for the format right away.
        let odt = dir.join("doc.odt");
        std::fs::write(&odt, b"x").unwrap();
        let mut app = app_with("", &odt);
        assert!(command(&mut app, "org-odt-convert"));
        // A missing file is refused (and we never reach LibreOffice).
        answer(
            &mut app,
            &Ask::OdtFormat {
                file: dir.join("nope.odt").display().to_string(),
            },
            "pdf",
        );
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("No such file")),
            "{:?}",
            app.message
        );
        // Answering a file name opens the format chooser.
        assert!(answer(
            &mut app,
            &Ask::OdtFile,
            &dir.join("other.odt").display().to_string()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn convert_region_aliases_agree() {
        let dir = tmp("alias");
        let mut app = app_with("plain *b*\n", &dir.join("t.org"));
        app.buf().set_point(Pos::new(0, 0));
        app.buf().push_mark(Pos::new(1, 0), true);
        assert!(command(&mut app, "org-export-region-to-md"));
        assert!(app.buf_ref().get_contents().contains("**b**"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dispatcher_accepts_keys_and_toggles() {
        let dir = tmp("disp");
        let mut app = app_with("* A\ntext\n", &dir.join("t.org"));
        assert_eq!(find_action("h H"), Some(("html", Act::Buffer)));
        assert_eq!(find_action("hh"), Some(("html", Act::File)));
        assert_eq!(find_action("h h  HTML to file"), Some(("html", Act::File)));
        assert_eq!(find_action("zz"), None);
        assert!(answer(
            &mut app,
            &Ask::ExportDispatch,
            "[ ] body only  (toggle)"
        ));
        assert!(app.org.export.body_only);
        // The prompt is open again; answer it.
        assert!(answer(&mut app, &Ask::ExportDispatch, "m M"));
        let t = app.buf_ref().get_contents();
        assert!(t.contains("# 1 A"), "{t}");
        assert!(!t.contains("Table of Contents"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeat_and_default_template() {
        let dir = tmp("rep");
        let mut app = app_with("* A\n", &dir.join("t.org"));
        assert!(command(&mut app, "org-export-repeat"));
        assert!(!dir.join("t.html").exists());
        command(&mut app, "org-html-export-to-html");
        std::fs::remove_file(dir.join("t.html")).unwrap();
        command(&mut app, "org-export-repeat");
        assert!(dir.join("t.html").exists());
        assert!(command(&mut app, "org-export-insert-default-template"));
        assert!(app.buf_ref().get_contents().starts_with("#+TITLE: t\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agenda_calendars_are_written() {
        let dir = tmp("ics");
        let f = dir.join("plan.org");
        std::fs::write(&f, "* Meet\nSCHEDULED: <2024-05-06 Mon 10:00>\n").unwrap();
        let mut app = app_with("", &dir.join("t.org"));
        app.editor.org_files_loaded = true;
        app.editor.org_files = vec![f.clone()];
        assert!(command(&mut app, "org-icalendar-export-agenda-files"));
        assert!(
            std::fs::read_to_string(dir.join("plan.ics"))
                .unwrap()
                .contains("SUMMARY:S: Meet")
        );
        app.org.directory = Some(dir.clone());
        assert!(command(&mut app, "org-icalendar-combine-agenda-files"));
        assert!(
            std::fs::read_to_string(dir.join("agenda.ics"))
                .unwrap()
                .contains("BEGIN:VEVENT")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_org_buffers_are_refused() {
        let mut app = App::new(forge_core::Editor::default());
        app.editor.create_buffer("x");
        command(&mut app, "org-html-export-to-html");
        assert!(app.buf_ref().get_contents().is_empty());
    }
}
