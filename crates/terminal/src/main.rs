// SPDX-License-Identifier: MIT OR Apache-2.0
use ratatui::crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Stdout};

use forge_terminal::app::App;
use forge_terminal::{check, headless, mouse, ui, watchdog};

type Term = Terminal<CrosstermBackend<Stdout>>;

use forge_terminal::cli::{self, Daemon};

const USAGE: &str = cli::USAGE;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Forge {}", forge_core::VERSION);
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }

    if let Some(i) = args.iter().position(|a| a == "--headless") {
        let mut opts = headless::Options {
            steps: Vec::new(),
            width: 80,
            height: 24,
        };
        let mut it = args.iter().skip(i + 1);
        while let Some(a) = it.next() {
            if a == "--screen" {
                opts.steps.push(headless::Step::Screen);
                continue;
            }
            let value = it
                .next()
                .ok_or_else(|| anyhow::anyhow!("{a} needs a value"))?
                .clone();
            match a.as_str() {
                "--init" => opts.steps.push(headless::Step::Init(value)),
                "--plugin" => opts.steps.push(headless::Step::Plugin(value)),
                "--open" => opts.steps.push(headless::Step::Open(value)),
                "--keys" => opts.steps.push(headless::Step::Keys(value)),
                "--type" => opts.steps.push(headless::Step::Type(value)),
                "--wait" => opts.steps.push(headless::Step::Wait(
                    value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("--wait takes seconds"))?,
                )),
                "--test" => opts.steps.push(headless::Step::Test(value)),
                "--size" => {
                    let (w, h) = value
                        .split_once('x')
                        .ok_or_else(|| anyhow::anyhow!("--size is WxH"))?;
                    opts.width = w.parse()?;
                    opts.height = h.parse()?;
                }
                _ => anyhow::bail!("unknown option {a} for --headless\n{USAGE}"),
            }
        }
        std::process::exit(if headless::run(opts) { 0 } else { 1 });
    }
    if let Some(i) = args.iter().position(|a| a == "--package-entry") {
        let Some(file) = args.get(i + 1) else {
            anyhow::bail!("--package-entry needs a .wasm file\n{USAGE}")
        };
        let url = args
            .iter()
            .position(|a| a == "--url")
            .and_then(|j| args.get(j + 1));
        print!(
            "{}",
            forge_core::package::entry_for(std::path::Path::new(file), url.map(String::as_str))
                .map_err(anyhow::Error::msg)?
        );
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--check-plugin") {
        let Some(file) = args.get(i + 1).cloned() else {
            anyhow::bail!("--check-plugin needs a file\n{USAGE}")
        };
        let mut opts = check::Options {
            file,
            steps: Vec::new(),
            answers: Vec::new(),
            text: String::new(),
            cache: forge_core::config::cache_dir(),
        };
        let mut it = args.iter().skip(i + 2);
        while let Some(a) = it.next() {
            let value = it
                .next()
                .ok_or_else(|| anyhow::anyhow!("{a} needs a value"))?
                .clone();
            match a.as_str() {
                "--run" => opts.steps.push(check::Step::Run(value)),
                "--open" => opts.steps.push(check::Step::Open(value)),
                "--point" => {
                    let (l, c) = value.split_once(':').unwrap_or((&value, "0"));
                    let (Ok(l), Ok(c)) = (l.parse(), c.parse()) else {
                        anyhow::bail!("--point takes LINE:COL, not {value}")
                    };
                    opts.steps.push(check::Step::Point(l, c));
                }
                "--wait" => opts.steps.push(check::Step::Wait(
                    value
                        .parse()
                        .map_err(|_| anyhow::anyhow!("--wait takes seconds, not {value}"))?,
                )),
                "--answer" => opts.answers.push(value),
                "--text" => opts.text = value,
                _ => anyhow::bail!("unknown option {a} for --check-plugin\n{USAGE}"),
            }
        }
        std::process::exit(if check::run(opts) { 0 } else { 1 });
    }

    let opts = cli::parse(&args)?;
    if let Some(dir) = &opts.chdir {
        std::env::set_current_dir(dir)
            .map_err(|e| anyhow::anyhow!("cannot change directory to {}: {e}", dir.display()))?;
    }
    // Where init.rhai, themes, plugins and grants are read from.
    if let Some(dir) = &opts.init_directory {
        // SAFETY: still single-threaded; nothing has started yet.
        unsafe { std::env::set_var("FORGE_CONFIG_DIR", dir) };
    } else if let Some(user) = &opts.user {
        let home = if cfg!(target_os = "macos") {
            "/Users"
        } else {
            "/home"
        };
        unsafe { std::env::set_var("FORGE_CONFIG_DIR", format!("{home}/{user}/.config/forge")) };
    }
    if opts.package_doctor {
        std::process::exit(
            if forge_terminal::package_doctor::run_headless(opts.json)? {
                0
            } else {
                1
            },
        );
    }
    if let Some((before, after)) = &opts.package_snapshot_diff {
        let config = forge_core::config::config_dir()
            .ok_or_else(|| anyhow::anyhow!("no config directory for package snapshots"))?;
        let changes = forge_core::LockFile::diff_snapshots(&config, before, after)?;
        print_package_changes(
            &changes,
            opts.json,
            &format!("No changes between package snapshots {before} and {after}"),
            &format!("Package snapshot diff: {before} -> {after}"),
        )?;
        return Ok(());
    }
    if let Some((before, after)) = &opts.package_lockfile_diff {
        let changes = forge_core::LockFile::diff_lockfiles(before, after)?;
        print_package_changes(
            &changes,
            opts.json,
            &format!(
                "No changes between package lockfiles {} and {}",
                before.display(),
                after.display()
            ),
            &format!(
                "Package lockfile diff: {} -> {}",
                before.display(),
                after.display()
            ),
        )?;
        return Ok(());
    }
    let mut plugin_files = opts.plugin_files.clone();
    for dir in &opts.plugin_dirs {
        plugin_files.extend(cli::plugins_in(dir));
    }
    if opts.profile_startup {
        let ok = forge_editor::startup_profile::run(forge_editor::startup_profile::Options {
            no_config: opts.no_config,
            files: opts.files.clone(),
            plugin_files,
            json: opts.json,
        })?;
        std::process::exit(if ok { 0 } else { 1 });
    }
    if let Some(Daemon::Background(name)) = &opts.daemon {
        return spawn_daemon(&args, name);
    }
    // Open files before touching the terminal so errors print normally.
    let mut app = forge_terminal::start(&forge_terminal::StartOptions {
        no_config: opts.no_config,
        files: opts.files.clone(),
        plugin_files,
        graphical: false,
        profile_startup: false,
    })?;
    if let Some((line, column)) = opts.position {
        app.buf().set_point(forge_core::Pos::new(
            line.saturating_sub(1) as usize,
            column as usize,
        ));
    }
    let actions_ok = cli::run_actions(&mut app, &opts.actions, opts.batch);
    if opts.batch || opts.kill {
        let ok = actions_ok && !app.startup_budget_exceeded;
        std::process::exit(if ok { 0 } else { 1 });
    }
    if let Some(Daemon::Foreground(name)) = &opts.daemon {
        forge_editor::server::start(&mut app, name);
        if app.server.is_none() {
            anyhow::bail!("cannot start the server");
        }
        run_daemon(&mut app);
        app.save_clipboard_on_exit();
        app.editor.save_session();
        return Ok(());
    }
    // What this terminal can draw (only the terminal frontend needs to know).

    let mut terminal = setup_terminal()?;

    // Put the terminal back to normal even if we panic.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    watchdog::spawn(app.watch.clone());
    install_fatal_signal_handlers();
    // auto-save everything before going down for any reason
    // other than quitting: a crash, a closed terminal, a kill signal.
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut terminal, &mut app)
    })) {
        Ok(r) => r,
        Err(panic) => {
            app.auto_save_all();
            std::panic::resume_unwind(panic);
        }
    };
    if result.is_err() || FATAL_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) {
        app.auto_save_all();
    }
    app.save_clipboard_on_exit();
    app.editor.save_session();

    restore_terminal()?;
    terminal.show_cursor()?;
    result
}

fn print_package_changes(
    changes: &[forge_core::SnapshotChange],
    json: bool,
    empty: &str,
    heading: &str,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(changes)?);
    } else if changes.is_empty() {
        println!("{empty}");
    } else {
        println!("{heading}");
        for change in changes {
            let revision = |entry: Option<&forge_core::LockEntry>| {
                entry
                    .and_then(|e| {
                        e.commit
                            .as_deref()
                            .or(e.artifact_sha256.as_deref())
                            .or(Some(e.sha256.as_str()))
                    })
                    .unwrap_or("-")
                    .chars()
                    .take(12)
                    .collect::<String>()
            };
            println!(
                "{} {}: {} -> {}",
                change.change,
                change.name,
                revision(change.before.as_ref()),
                revision(change.after.as_ref())
            );
        }
    }
    Ok(())
}

/// The keyboard protocol Forge asks for, if the terminal has it: with
/// this one flag, a terminal that supports it (kitty, ghostty, WezTerm,
/// foot, ...) reports the keys it used to send as an ambiguous control
/// code or a shifted character: Ctrl+I as itself rather than as TAB,
/// Ctrl+M rather than RET, and Shift as a modifier rather than folded into
/// the letter. That is what makes `C-S-a`, Super and Hyper bindable.
///
/// Only this flag: `REPORT_EVENT_TYPES` would add key *release* events,
/// which Forge would have to filter out of every keystroke (the event loop
/// already does, for terminals that report releases on their own).
///
/// A terminal that doesn't know the sequence ignores it, so this is
/// pushed unconditionally. Nothing here queries for support: the query
/// waits two seconds for an answer, and every terminal that would answer
/// has already had the flag pushed by then.
const KEYBOARD_ENHANCEMENT: KeyboardEnhancementFlags =
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;

fn setup_terminal() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Bracketed paste: a paste arrives as one Event::Paste, not as a
    // stream of keystrokes whose control characters (C-x C-c in a
    // pasted URL) would run editor commands. Like the keyboard
    // enhancement flags, a terminal that doesn't know the sequence
    // ignores it and pastes as keys, as before.
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableFocusChange,
        EnableBracketedPaste
    )?;
    // Try to enable keyboard enhancement flags (kitty protocol). On
    // legacy Windows consoles this may not be supported, so ignore
    // errors.
    let _ = execute!(stdout, PushKeyboardEnhancementFlags(KEYBOARD_ENHANCEMENT));
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal() -> io::Result<()> {
    // Before the terminal is handed back: a terminal still waiting for
    // CSI u sequences would send them to whatever runs next.
    let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        SetCursorStyle::DefaultUserShape
    )
}

/// Set by SIGHUP (the terminal went away) and SIGTERM: the event loop
/// notices within a second, auto-saves and exits, and the buffers' file
/// locks are released on the way out.
static FATAL_SIGNAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_fatal_signal(_: libc::c_int) {
    FATAL_SIGNAL.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn install_fatal_signal_handlers() {
    #[cfg(unix)]
    unsafe {
        let handler = on_fatal_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        install_one(libc::SIGHUP, handler);
        install_one(libc::SIGTERM, handler);
    }
}

/// `sigaction` rather than `signal`: it doesn't reset the handler on
/// delivery and its semantics are well defined. `SA_RESTART` keeps
/// interrupted syscalls going, since the handler only sets a flag.
#[cfg(unix)]
unsafe fn install_one(sig: libc::c_int, handler: libc::sighandler_t) {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(sig, &action, std::ptr::null_mut());
    }
}

/// The longest the loop waits without looking for a fatal signal.
const SIGNAL_CHECK: std::time::Duration = std::time::Duration::from_secs(1);

fn run(terminal: &mut Term, app: &mut App) -> anyhow::Result<()> {
    use forge_core::decoration::CursorShape;
    let mut mouse_on = false;
    let mut shape = CursorShape::Box;
    let mut clip_out: Option<String> = None;
    // Inline pictures where the terminal draws them; the display only
    // reserves blank rows for them when they can show.
    let mut gfx =
        forge_terminal::gfx::GfxState::new(forge_terminal::caps::TermCaps::detect().graphics);
    if gfx.proto() != forge_terminal::caps::GraphicsProto::Off {
        forge_core::org_display::set_inline_image_rows(8);
    }
    while !app.quit {
        if FATAL_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        // Copies (M-w and friends) reach the system clipboard, so there
        // is a way to get text out of a terminal Forge.
        sync_clipboard(app, &mut clip_out);
        // Follow xterm-mouse-mode / init.rhai's forge.opt.mouse.
        if app.mouse != mouse_on {
            if app.mouse {
                // crossterm's EnableMouseCapture asks for any-event
                // tracking (?1003h), so moves arrive too, not just clicks.
                execute!(io::stdout(), EnableMouseCapture)?;
            } else {
                execute!(io::stdout(), DisableMouseCapture)?;
            }
            mouse_on = app.mouse;
        }
        // The watchdog wrote on the screen while a plugin was busy.
        if app.watch.take_dirty() {
            terminal.clear()?;
        }
        let mut pictures = Vec::new();
        terminal.draw(|f| {
            pictures = ui::render(f, app);
        })?;
        // Inline pictures over their blank rows; an unchanged screen
        // sends nothing.
        forge_terminal::gfx::sync(&mut io::stdout(), &mut gfx, &pictures)?;
        // Modal editing shows its state with the cursor's shape.
        if app.cursor_shape != shape {
            shape = app.cursor_shape;
            let style = match shape {
                CursorShape::Box => SetCursorStyle::DefaultUserShape,
                CursorShape::Bar => SetCursorStyle::SteadyBar,
                CursorShape::Underline => SetCursorStyle::SteadyUnderScore,
            };
            execute!(io::stdout(), style)?;
        }
        // Keys typed while a plugin was busy come first, in order.
        let stolen = app.watch.take_stolen();
        if !stolen.is_empty() {
            for ev in stolen {
                handle_event(app, ev);
            }
            continue;
        }
        // Half-way through a prefix key, wait only so long before
        // showing which keys can follow it.
        let delay = app.idle_timeout();
        let wait = delay.map_or(SIGNAL_CHECK, |d| d.min(SIGNAL_CHECK));
        if !event::poll(wait)? {
            // Idle work only when its own time has come (the auto-save
            // timer counts from the last key, so it comes round).
            if delay.is_some_and(|d| d <= wait) {
                app.on_idle();
            }
            continue;
        }
        let ev = event::read()?;
        // A trackpad burst queues dozens of wheel events per frame;
        // working through them one frame each is the lag that keeps
        // scrolling after the fingers lift. Fold one burst into this
        // frame's single redisplay instead (capped, so a held-down
        // scroll still draws).
        let wheel = match &ev {
            Event::Mouse(m)
                if matches!(
                    m.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) =>
            {
                Some(m.kind)
            }
            _ => None,
        };
        handle_event(app, ev);
        if let Some(kind) = wheel {
            for _ in 0..32 {
                if !event::poll(std::time::Duration::from_secs(0)).unwrap_or(false) {
                    break;
                }
                match event::read() {
                    Ok(Event::Mouse(m2)) if m2.kind == kind => handle_event(app, Event::Mouse(m2)),
                    Ok(other) => {
                        handle_event(app, other);
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
    }
    // Pictures don't leave with the alternate screen on every terminal.
    gfx.clear(&mut io::stdout())?;
    Ok(())
}

fn handle_event(app: &mut App, ev: Event) {
    match ev {
        // Windows sends both press and release events; only act on presses.
        Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
        // A bracketed paste (Cmd-V, C-S-v): inserted as text, so its
        // control characters can't run editor commands.
        Event::Paste(text) => app.paste_text(&text),
        // Another program may have changed files while we were away.
        Event::FocusGained => app.on_focus(),
        Event::Mouse(m) => mouse::handle(app, m),
        _ => {}
    }
}

/// Copy the kill ring's top to the system clipboard when it changes.
/// OSC52 covers terminals that speak it; macOS's Terminal.app ignores
/// the sequence, so pbcopy carries the same text there (tty Forge
/// shells out the same way). Pastes come back the other way through
/// bracketed paste, as one Event::Paste.
fn sync_clipboard(app: &App, last: &mut Option<String>) {
    if !app.select_enable_clipboard && !app.select_enable_primary {
        return;
    }
    let cur = app.editor.kill_ring.current().map(str::to_string);
    if cur == *last {
        return;
    }
    *last = cur.clone();
    let Some(text) = cur else { return };
    if text.is_empty() {
        return;
    }
    let sanitized = text.replace('\0', "\\0");
    // A huge kill would become a megabytes-long escape sequence.
    if sanitized.len() <= 256 * 1024 {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(sanitized.as_bytes());
        if app.select_enable_clipboard {
            print!("\x1b]52;c;{b64}\x07");
        }
        if app.select_enable_primary {
            print!("\x1b]52;p;{b64}\x07");
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    if app.select_enable_clipboard {
        pbcopy(&sanitized);
    }
}

/// Copy text into the system clipboard via pbcopy, which reaches it
/// even under terminals that ignore OSC52 (Terminal.app, tmux without
/// set-clipboard). No size cap: unlike an escape sequence, a pipe
/// handles a large kill fine.
#[cfg(target_os = "macos")]
fn pbcopy(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() else {
        return;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

#[cfg(not(target_os = "macos"))]
fn pbcopy(_text: &str) {}

/// `--daemon`: start this program again as `--fg-daemon`, detached from
/// the terminal, and return once its server file exists.
fn spawn_daemon(args: &[String], name: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut child_args: Vec<String> = args
        .iter()
        .map(|a| {
            if a == "--daemon" || a == "--bg-daemon" {
                "--fg-daemon".to_string()
            } else if let Some(n) = a
                .strip_prefix("--daemon=")
                .or_else(|| a.strip_prefix("--bg-daemon="))
            {
                format!("--fg-daemon={n}")
            } else {
                a.clone()
            }
        })
        .collect();
    child_args.retain(|a| a != "-daemon");
    let file = forge_editor::server::server_file(name);
    if let Some(f) = &file {
        // A stale file from a daemon that died would end the wait early.
        let _ = std::fs::remove_file(f);
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&child_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if file.as_ref().is_some_and(|f| f.exists()) {
            eprintln!("Forge daemon \"{name}\" started");
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!("the daemon exited at startup ({status})");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    anyhow::bail!("the daemon did not start within 15 seconds")
}

/// The event loop of a daemon: no screen, just timers, processes and the
/// server, until something quits.
fn run_daemon(app: &mut App) {
    install_fatal_signal_handlers();
    while !app.quit && !FATAL_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) {
        app.on_idle();
        let wait = app
            .idle_timeout()
            .unwrap_or(std::time::Duration::from_millis(100))
            .min(std::time::Duration::from_millis(100))
            .max(std::time::Duration::from_millis(10));
        std::thread::sleep(wait);
    }
    app.auto_save_all();
}

#[cfg(test)]
mod cli_tests {
    use forge_terminal::cli::parse as parse_cli;

    #[test]
    fn parses_file_aliases_and_location() {
        let args = [
            "+12:4",
            "--find-file",
            "src/main.rs",
            "--visit",
            "README.md",
        ]
        .map(str::to_string);
        let parsed = parse_cli(&args).unwrap();
        assert_eq!(parsed.files, ["src/main.rs", "README.md"]);
        assert_eq!(
            parsed.position, None,
            "a location applies only to the next file"
        );

        let args = ["+12:4", "--file", "src/main.rs"].map(str::to_string);
        let parsed = parse_cli(&args).unwrap();
        assert_eq!(parsed.position, Some((12, 4)));
    }

    #[test]
    fn rejects_bad_or_unattached_locations() {
        assert!(parse_cli(&["+0".into(), "file".into()]).is_err());
        assert!(parse_cli(&["+12".into()]).is_err());
    }
}
