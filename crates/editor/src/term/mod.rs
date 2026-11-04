// SPDX-License-Identifier: MIT OR Apache-2.0
//! `term`: a real terminal inside Forge, in a buffer.
//!
//! M-x term runs your shell on a pseudo-terminal and draws what it shows
//! in a `*term*` buffer: full xterm emulation (256 and true colours,
//! bold/italic/underline/reverse, the alternate screen of vim, htop and
//! less, scroll regions, wide characters, bracketed paste, resize), with
//! the scrollback in the buffer itself, so searching and copying work the
//! way they do anywhere else.
//!
//! The buffer has two modes, as in term.el:
//!
//! * char mode (the start): every key goes to the program, except C-c
//!   (the prefix for term's own keys), C-x (Forge's prefix, so C-x b, C-x o,
//!   C-x 4 b and the rest work; C-c C-x sends a real C-x to the program),
//!   and the Meta keys Forge keeps (M-x, M-:, M-!, M-&); a key sequence that
//!   has started is finished by Forge whatever its length;
//! * copy mode (C-c C-j): the buffer is ordinary text to move through,
//!   search and copy; C-c C-k goes back.
//!
//! Which shell: `forge.opt.term_shell = "/bin/zsh"` (arguments allowed:
//! `"fish -l"`); unset, the shell you log in with (`$SHELL`, else the
//! password database's). C-u M-x term, or M-x term-choose-shell, asks.
//! `forge.opt.term_scrollback` is how many lines are kept (5000).
//!
//! The shell can tell Forge where it is: print OSC 7 after each command
//! (`printf '\e]7;file://%s%s\a' "$HOST" "$PWD"`) and the buffer's
//! default directory follows; OSC 133;A at each prompt makes C-c C-p and
//! C-c C-n jump between prompts. `forge-term-integration` prints the
//! snippet for bash, zsh and fish.

pub mod pty;
pub mod vt;

use crate::app::App;
use crate::org_cmds::{Ask, choose};
use forge_core::BufferId;
use forge_core::decoration::{DecoFace, DecoSpec, TermFace};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::path::PathBuf;
use vt::{Cell, Color, Vt};

pub const MODE: &str = "term-mode";

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new("term", "Run a shell in a terminal buffer.")
        .body("Every key goes to the program; C-c is the prefix for term's own keys (C-c C-j for copy mode) and C-x for Forge's (C-c C-x sends C-x to the program). With C-u, ask which shell. forge.opt.term_shell picks one for good; unset, it is your login shell."),
    forge_docs::CommandDoc::line("term-char-mode", "Send keys to the program again (C-c C-k)."),
    forge_docs::CommandDoc::line("term-choose-shell", "Run a terminal with a shell you pick."),
    forge_docs::CommandDoc::line("term-copy-mode", "Treat the terminal buffer as text to move through and copy (C-c C-j)."),
    forge_docs::CommandDoc::line("term-interrupt-subjob", "Send C-c to the program (C-c C-c)."),
    forge_docs::CommandDoc::line("term-kill-subjob", "Stop the shell and its programs (C-c C-q)."),
    forge_docs::CommandDoc::line("term-next-prompt", "Move to the next shell prompt (C-c C-n)."),
    forge_docs::CommandDoc::line("term-paste", "Send the last kill to the program (C-y, C-c C-y)."),
    forge_docs::CommandDoc::line("term-previous-prompt", "Move to the previous shell prompt (C-c C-p)."),
    forge_docs::CommandDoc::line("term-send-ctrl-x", "Send C-x to the program (C-c C-x)."),
    forge_docs::CommandDoc::line("term-show-integration", "Show the lines that make your shell report its directory and prompts."),
];

pub const BINDINGS: &[(&str, &str)] = &[
    ("C-c C-j", "term-copy-mode"),
    ("C-c C-k", "term-char-mode"),
    ("C-c C-c", "term-interrupt-subjob"),
    ("C-c C-q", "term-kill-subjob"),
    ("C-c C-n", "term-next-prompt"),
    ("C-c C-p", "term-previous-prompt"),
    ("C-c C-y", "term-paste"),
    ("C-c C-x", "term-send-ctrl-x"),
];

pub fn install_keymaps(keymaps: &mut forge_core::Keymaps) {
    use forge_core::keymap::{Origin, Scope};
    for (k, c) in BINDINGS {
        keymaps
            .bind(&Scope::Mode(MODE.into()), k, c, Origin::Package)
            .expect("valid term binding");
    }
}

pub struct Term {
    pty: pty::Pty,
    vt: Vt,
    /// Copy mode: the buffer is left alone for the user to move in.
    copy: bool,
    /// C-c was typed in char mode.
    exited: bool,
    /// Redraw the buffer when leaving copy mode.
    stale: bool,
    prompts_seen: usize,
}

#[derive(Default)]
pub struct TermState {
    terms: BTreeMap<BufferId, Term>,
}

impl TermState {
    pub fn needs_poll(&self) -> bool {
        self.terms.values().any(|t| !t.exited)
    }

    pub fn is_term(&self, id: BufferId) -> bool {
        self.terms.contains_key(&id)
    }
}

// ------------------------------------------------------------ starting

fn shell_setting(app: &App) -> String {
    crate::org_flow::setting_string(app, "term_shell").unwrap_or_default()
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    if !cmd.starts_with("term") {
        return false;
    }
    match cmd {
        "term" => {
            if app.has_arg() {
                choose_shell(app);
            } else {
                let shell = pty::default_shell(&shell_setting(app));
                start(app, &shell);
            }
        }
        "term-choose-shell" => choose_shell(app),
        "term-show-integration" => show_integration(app),
        _ => return term_command(app, cmd),
    }
    true
}

fn choose_shell(app: &mut App) {
    choose(app, "Shell: ", Ask::TermShell, pty::installed_shells());
}

pub fn answer(app: &mut App, a: &Ask, input: &str) -> bool {
    match a {
        Ask::TermShell => {
            let t = input.trim();
            let shell = pty::default_shell(t);
            start(app, &shell);
            true
        }
        _ => false,
    }
}

fn unique_name(app: &App) -> String {
    let taken = |n: &str| app.editor.state.buffers.values().any(|b| b.name() == n);
    if !taken("*term*") {
        return "*term*".into();
    }
    (2..)
        .map(|i| format!("*term*<{i}>"))
        .find(|n| !taken(n))
        .unwrap()
}

pub fn start(app: &mut App, shell: &str) {
    let dir = crate::compile::default_directory(app);
    let dir = if dir.is_dir() {
        dir
    } else {
        PathBuf::from("/")
    };
    let (rows, cols) = (
        app.win().height.max(4).min(500) as u16,
        app.win().width.max(10).min(1000) as u16,
    );
    let pty = match pty::Pty::spawn(shell, &dir, rows, cols, &[]) {
        Ok(p) => p,
        Err(e) => return app.error(format!("Can't run {shell}: {e}")),
    };
    let mut vt = Vt::new(rows as usize, cols as usize);
    vt.scrollback_limit = crate::org_flow::setting_number(app, "term_scrollback")
        .map_or(5000, |n| n.max(0.0) as usize);
    let name = unique_name(app);
    let id = app.editor.create_buffer(name);
    {
        let b = app.editor.state.get_buffer_mut(id).expect("just made");
        b.set_major_mode(MODE);
    }
    app.jobs.dirs.insert(id, dir);
    app.term.terms.insert(
        id,
        Term {
            pty,
            vt,
            copy: false,
            exited: false,
            stale: true,
            prompts_seen: 0,
        },
    );
    app.switch_to(id);
    app.msg(format!("term: {shell}"));
}

fn show_integration(app: &mut App) {
    let text = "\
# Make your shell tell Forge where it is, and where its prompts are.

# bash (~/.bashrc)
PROMPT_COMMAND='printf \"\\e]7;file://%s%s\\a\\e]133;A\\a\" \"$HOSTNAME\" \"$PWD\"'\"${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"

# zsh (~/.zshrc)
precmd() { printf '\\e]7;file://%s%s\\a\\e]133;A\\a' \"$HOST\" \"$PWD\" }

# fish (~/.config/fish/config.fish)
function __forge_osc --on-event fish_prompt
    printf '\\e]7;file://%s%s\\a\\e]133;A\\a' (hostname) $PWD
end

# Only inside Forge: wrap any of them in  if test -n \"$INSIDE_FORGE\"
";
    app.show_special_buffer("*term integration*", "special-mode", text);
}

// ---------------------------------------------------------------- keys

/// Keys in a term buffer in char mode. True when the key was consumed.
pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    let id = app.active_id();
    // A key sequence Forge has started (C-c C-j, C-x 4 b, M-g g, C-u 3 ...)
    // is finished by Forge, however long it is: the keys after the first
    // must not reach the program.
    let in_sequence = !app.at_top_level();
    let Some(t) = app.term.terms.get_mut(&id) else {
        return false;
    };
    if t.copy {
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if in_sequence {
        return false;
    }
    if ctrl && matches!(key.code, KeyCode::Char('c' | 'x')) {
        // Forge's prefixes: hand the key to the keymap machinery.
        return false;
    }
    if alt && matches!(key.code, KeyCode::Char('x' | ':' | '!' | '&')) {
        return false;
    }
    if t.exited {
        if key.code == KeyCode::Enter {
            app.kill_buffer(id);
            return true;
        }
        return false;
    }
    let bytes = encode(&key, t.vt.app_cursor);
    match bytes {
        Some(b) => {
            t.pty.write(&b);
            true
        }
        None => false,
    }
}

/// The bytes a program expects for `key`.
pub fn encode(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // xterm's modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi = |fin: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{fin}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{fin}").into_bytes()
        } else {
            format!("\x1b[{fin}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let mut out = match key.code {
        KeyCode::Char(c) => {
            let b = if ctrl {
                match c {
                    ' ' | '@' | '2' => vec![0],
                    'a'..='z' => vec![c as u8 - b'a' + 1],
                    'A'..='Z' => vec![c as u8 - b'A' + 1],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    '^' | '6' => vec![0x1e],
                    '_' | '/' => vec![0x1f],
                    '?' => vec![0x7f],
                    _ => c.to_string().into_bytes(),
                }
            } else {
                c.to_string().into_bytes()
            };
            if alt {
                let mut v = vec![0x1b];
                v.extend(b);
                return Some(v);
            }
            return Some(b);
        }
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![if ctrl { 0x08 } else { 0x7f }],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi('A'),
        KeyCode::Down => csi('B'),
        KeyCode::Right => csi('C'),
        KeyCode::Left => csi('D'),
        KeyCode::Home => csi('H'),
        KeyCode::End => csi('F'),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::F(n @ 1..=4) => {
            let c = (b'P' + n - 1) as char;
            if m > 1 {
                format!("\x1b[1;{m}{c}").into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n) - 5]),
        _ => return None,
    };
    if alt && m == 1 {
        out.insert(0, 0x1b);
    }
    Some(out)
}

fn term_command(app: &mut App, cmd: &str) -> bool {
    let id = app.active_id();
    if !app.term.terms.contains_key(&id) {
        if cmd.starts_with("term-") {
            app.error("Not in a terminal buffer");
            return true;
        }
        return false;
    }
    match cmd {
        "term-copy-mode" => {
            if let Some(t) = app.term.terms.get_mut(&id) {
                t.copy = true;
            }
            app.msg("Copy mode: C-c C-k returns to the program");
        }
        "term-char-mode" => {
            if let Some(t) = app.term.terms.get_mut(&id) {
                t.copy = false;
                t.stale = true;
            }
            refresh(app, id);
        }
        "term-interrupt-subjob" => send(app, id, b"\x03"),
        "term-send-ctrl-x" => send(app, id, b"\x18"),
        "term-kill-subjob" => {
            if let Some(t) = app.term.terms.get_mut(&id) {
                t.pty.kill();
                t.exited = true;
            }
            app.msg("Terminal process killed");
        }
        "term-paste" => {
            let text = app
                .editor
                .kill_ring
                .current()
                .map(str::to_string)
                .unwrap_or_default();
            paste(app, id, &text);
        }
        "term-next-prompt" | "term-previous-prompt" => {
            prompt_jump(app, id, cmd == "term-next-prompt")
        }
        _ => return false,
    }
    true
}

fn send(app: &mut App, id: BufferId, bytes: &[u8]) {
    if let Some(t) = app.term.terms.get_mut(&id)
        && !t.exited
    {
        t.pty.write(bytes);
    }
}

/// Send text as a paste: bracketed when the program asked for it.
pub fn paste(app: &mut App, id: BufferId, text: &str) {
    let Some(t) = app.term.terms.get_mut(&id) else {
        return;
    };
    if t.exited || text.is_empty() {
        return;
    }
    if t.vt.bracketed_paste {
        t.pty.write(b"\x1b[200~");
        t.pty.write(text.as_bytes());
        t.pty.write(b"\x1b[201~");
    } else {
        t.pty.write(text.replace('\n', "\r").as_bytes());
    }
}

fn prompt_jump(app: &mut App, id: BufferId, forward: bool) {
    let Some(t) = app.term.terms.get(&id) else {
        return;
    };
    let rows: Vec<usize> = t.vt.prompt_rows.clone();
    let b = app.buf_ref();
    let here = b.point().line;
    let target = if forward {
        rows.iter().copied().find(|&r| r > here)
    } else {
        rows.iter().copied().rev().find(|&r| r < here)
    };
    match target {
        Some(line) => {
            let b = app.buf();
            b.set_point(forge_core::Pos { line, col: 0 });
        }
        None => app.msg(if forward {
            "No next prompt"
        } else {
            "No previous prompt"
        }),
    }
}

// ------------------------------------------------------------- output

/// Called when idle: read what the shells wrote, answer their queries,
/// and redraw the buffers that changed.
pub fn poll(app: &mut App) {
    let ids: Vec<BufferId> = app.term.terms.keys().copied().collect();
    for id in ids {
        if !app.editor.state.buffers.contains_key(&id) {
            // The buffer was killed: the Pty's drop stops the shell.
            app.term.terms.remove(&id);
            continue;
        }
        sync_size(app, id);
        let mut gone = false;
        let mut cwd = None;
        {
            let t = app.term.terms.get_mut(&id).expect("listed");
            for ev in t.pty.poll() {
                match ev {
                    pty::Event::Data(d) => t.vt.feed(&d),
                    pty::Event::Closed => gone = true,
                }
            }
            if !t.vt.reply.is_empty() {
                let r = std::mem::take(&mut t.vt.reply);
                t.pty.write(&r);
            }
            if let Some(c) = t.vt.cwd.take() {
                cwd = Some(c);
            }
            if gone && !t.exited {
                t.exited = true;
            }
        }
        if let Some(c) = cwd {
            let p = PathBuf::from(c);
            if p.is_dir() {
                app.jobs.dirs.insert(id, p);
            }
        }
        let (dirty, copy) = {
            let t = app.term.terms.get_mut(&id).expect("listed");
            let d = t.vt.take_dirty() || t.stale;
            (d, t.copy)
        };
        if dirty {
            if copy {
                app.term.terms.get_mut(&id).expect("listed").stale = true;
            } else {
                refresh(app, id);
            }
        }
        if gone {
            let code = app.term.terms.get_mut(&id).and_then(|t| t.pty.exited());
            let msg = match code {
                Some(Some(0)) | Some(None) | None => "Process finished".to_string(),
                Some(Some(n)) => format!("Process exited with status {n}"),
            };
            app.msg(format!("{msg}; Enter closes the buffer"));
            if let Some(t) = app.term.terms.get_mut(&id) {
                t.vt.feed(format!("\r\n[{msg}]\r\n").as_bytes());
            }
            refresh(app, id);
        }
    }
}

fn sync_size(app: &mut App, id: BufferId) {
    let win = app
        .editor
        .windows
        .iter()
        .find(|w| w.buffer == id)
        .map(|w| (w.height, w.width));
    let Some((h, w)) = win else { return };
    if h < 2 || w < 4 {
        return;
    }
    let t = app.term.terms.get_mut(&id).expect("listed");
    if (t.vt.rows, t.vt.cols) != (h, w) {
        t.vt.resize(h, w);
        t.pty.resize(h as u16, w as u16);
        t.stale = true;
    }
}

fn face_of(c: &Cell) -> Option<TermFace> {
    let rgb = |c: Color| match c {
        Color::Default => None,
        Color::Rgb(v) => Some(v),
    };
    let f = TermFace {
        fg: rgb(c.fg),
        bg: rgb(c.bg),
        attrs: c.attrs,
    };
    (f != TermFace::default()).then_some(f)
}

/// Is this cell the end-of-line padding every unwritten cell is?
fn blank_tail(c: &Cell) -> bool {
    c.ch == ' ' && c.bg == Color::Default && c.attrs & (vt::REVERSE | vt::UNDERLINE) == 0
}

/// The buffer text, its colour runs, and where the cursor is.
pub fn render(vt: &Vt) -> (String, Vec<DecoSpec>, usize) {
    let mut text = String::new();
    let mut runs: Vec<DecoSpec> = Vec::new();
    let mut at = 0usize;
    let first_screen = if vt.on_alt_screen() {
        0
    } else {
        vt.scrollback.len()
    };
    let mut cursor = 0usize;
    for (i, line) in vt.all_lines().enumerate() {
        let is_cursor_row = i == first_screen + vt.row;
        let mut end = line.len();
        while end > 0 && blank_tail(&line[end - 1]) {
            end -= 1;
        }
        let mut n = 0usize;
        let mut run: Option<(usize, TermFace)> = None;
        let flush = |run: &mut Option<(usize, TermFace)>, upto: usize, runs: &mut Vec<DecoSpec>| {
            if let Some((s, f)) = run.take() {
                runs.push(DecoSpec::new(s, upto, DecoFace::Term(f)));
            }
        };
        for (col, cell) in line[..end].iter().enumerate() {
            if is_cursor_row && col == vt.col {
                cursor = at + n;
            }
            if cell.ch == Cell::CONT {
                continue;
            }
            let f = face_of(cell);
            match (&run, f) {
                (Some((_, rf)), Some(nf)) if *rf == nf => {}
                (_, nf) => {
                    flush(&mut run, at + n, &mut runs);
                    run = nf.map(|f| (at + n, f));
                }
            }
            text.push(cell.ch);
            n += 1;
        }
        flush(&mut run, at + n, &mut runs);
        if is_cursor_row {
            // The cursor can be past the last written cell: pad to it.
            let written = line[..end.min(vt.col)]
                .iter()
                .filter(|c| c.ch != Cell::CONT)
                .count();
            if vt.col >= end {
                let pad = vt.col - line[..end].len();
                for _ in 0..pad {
                    text.push(' ');
                    n += 1;
                }
                cursor = at + n;
            } else {
                cursor = at + written;
            }
        }
        text.push('\n');
        at += n + 1;
    }
    // No trailing newline after the last screen line.
    if text.ends_with('\n') {
        text.pop();
    }
    (text, runs, cursor)
}

pub fn refresh(app: &mut App, id: BufferId) {
    let Some(t) = app.term.terms.get_mut(&id) else {
        return;
    };
    t.stale = false;
    t.prompts_seen = t.vt.prompt_rows.len();
    let (text, runs, cursor) = render(&t.vt);
    let visible = t.vt.cursor_visible;
    let _ = visible;
    let windows: Vec<_> = app
        .editor
        .windows
        .iter()
        .filter(|w| w.buffer == id)
        .map(|w| w.id)
        .collect();
    let Some(b) = app.editor.state.get_buffer_mut(id) else {
        return;
    };
    b.set_read_only(false);
    b.set_contents(&text);
    b.set_decorations("term", runs);
    b.set_offset(cursor.min(b.char_count()));
    b.set_modified(false);
    b.set_read_only(true);
    let cursor = cursor.min(b.char_count());
    for w in windows {
        app.editor.set_window_point(w, cursor);
    }
}

// --------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn keys_become_the_bytes_programs_expect() {
        let n = KeyModifiers::NONE;
        assert_eq!(
            encode(&k(KeyCode::Char('a'), n), false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            encode(&k(KeyCode::Char('d'), KeyModifiers::CONTROL), false),
            Some(vec![4])
        );
        assert_eq!(
            encode(&k(KeyCode::Char('f'), KeyModifiers::ALT), false),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(encode(&k(KeyCode::Up, n), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(&k(KeyCode::Up, n), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(
            encode(&k(KeyCode::Left, KeyModifiers::CONTROL), false),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(encode(&k(KeyCode::Backspace, n), false), Some(vec![0x7f]));
        assert_eq!(
            encode(&k(KeyCode::F(5), n), false),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn a_term_buffer_runs_a_shell_and_follows_it() {
        let mut app = App::new(forge_core::Editor::default());
        let scratch = app.editor.add_buffer(forge_core::Buffer::new("*scratch*"));
        app.editor.show_buffer(scratch);
        start(&mut app, "/bin/sh");
        let id = app.active_id();
        assert_eq!(app.buf_ref().major_mode(), MODE);
        for c in "echo term-$((6*7))\r".chars() {
            let key = if c == '\r' {
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            } else {
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
            };
            assert!(handle_key(&mut app, key));
        }
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < end && !app.buf_ref().get_contents().contains("term-42\n")
        {
            poll(&mut app);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(app.buf_ref().get_contents().contains("term-42\n"));
        // C-c is the prefix: it is not sent, and C-c C-j enters copy mode.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!handle_key(&mut app, ctrl_c));
        assert!(app.term.is_term(id));
        // C-x is Forge's too; a started sequence is finished by Forge, so
        // the third key of C-x 4 b never reaches the program.
        let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert!(!handle_key(&mut app, ctrl_x));
        app.pending = vec!["C-x".into(), "4".into()];
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)
        ));
        app.pending.clear();
        // M-f and the like still go to the shell; M-: stays Forge's.
        assert!(handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT)
        ));
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(':'), KeyModifiers::ALT)
        ));
        execute(&mut app, "term-copy-mode");
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
        ));
        execute(&mut app, "term-kill-subjob");
    }

    #[test]
    fn the_screen_becomes_text_with_colour_runs_and_a_cursor() {
        let mut v = Vt::new(3, 10);
        v.feed(b"\x1b[31mred\x1b[0m ok\r\nnext");
        let (text, runs, cursor) = render(&v);
        assert_eq!(text, "red ok\nnext\n");
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].start, runs[0].end), (0, 3));
        assert_eq!(cursor, 11);
    }

    #[test]
    fn a_cursor_past_the_text_is_padded_to() {
        let mut v = Vt::new(2, 10);
        v.feed(b"$ ");
        let (text, _, cursor) = render(&v);
        assert!(text.starts_with("$ "), "{text:?}");
        assert_eq!(cursor, 2);
    }
}
