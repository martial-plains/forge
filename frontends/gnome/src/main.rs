// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forge for GNOME.
//!
//! Forge draws its whole interface (windows, mode lines, menu bar, tool
//! bar, tabs, popups) as a grid of styled text runs and picture slots
//! (`forge_ffi::Grid`); this window draws that grid with Pango and Cairo,
//! sends GTK's keys, text and mouse to the editor, and shows GTK's file
//! picker when the editor asks for one (File ▸ Open File with Picker…).
//! Canvases arrive as scenes (`scene.rs` draws them with Cairo and Pango).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use forge_ffi::{Forge, Grid, GridDialog, Key, KeyInput, MouseKind};
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};

mod keys;
mod scene;

const APP_ID: &str = "com.isaiahharvey.forge";

/// Everything the window needs between frames.
struct State {
    grid: Option<Grid>,
    cell: (f64, f64),
    /// Pictures decoded once per file and modification time.
    textures: std::collections::HashMap<String, (std::time::SystemTime, Option<gdk::Texture>)>,
    last_dialog: u32,
    last_clip: Option<String>,
    dragging: bool,
    /// Canvases (scenes) and the pictures inside them.
    scenes: scene::Cache,
}

fn main() -> glib::ExitCode {
    let app = gtk4::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    let files: Rc<RefCell<Vec<String>>> = Rc::default();
    {
        let files = files.clone();
        app.connect_open(move |app, opened, _| {
            files.borrow_mut().extend(opened.iter().filter_map(|f| f.path()).map(|p| p.display().to_string()));
            app.activate();
        });
    }
    app.connect_activate(move |app| {
        if app.active_window().is_some() {
            return;
        }
        let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
        let mut open = files.borrow().clone();
        open.extend(args);
        let no_config = std::env::args().any(|a| a == "-q" || a == "--no-config");
        match Forge::new_grid(open, no_config) {
            Ok(forge) => {
                forge.set_native_scenes(true);
                window(app, forge)
            }
            Err(e) => {
                eprintln!("forge: {e}");
                app.quit();
            }
        }
    });
    app.run_with_args::<&str>(&[])
}

fn prefers_dark() -> bool {
    if let Ok(v) = std::env::var("FORGE_THEME") {
        return !v.eq_ignore_ascii_case("light");
    }
    let Some(settings) = gtk4::Settings::default() else { return true };
    settings.is_gtk_application_prefer_dark_theme()
        || settings.gtk_theme_name().is_some_and(|n| n.to_lowercase().contains("dark"))
        // GTK 4.20 and later report the desktop's colour scheme (2 is dark).
        || (settings.find_property("gtk-interface-color-scheme").is_some() && settings.property::<i32>("gtk-interface-color-scheme") == 2)
}

fn window(app: &gtk4::Application, forge: Arc<Forge>) {
    let win = gtk4::ApplicationWindow::builder().application(app).default_width(1100).default_height(720).title("Forge").build();
    let area = gtk4::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).can_focus(true).build();
    let state = Rc::new(RefCell::new(State { grid: None, cell: (9.0, 18.0), textures: Default::default(), last_dialog: 0, last_clip: None, dragging: false, scenes: Default::default() }));
    let size = Rc::new(Cell::new((100u16, 30u16)));
    let dark = Rc::new(Cell::new(prefers_dark()));

    // --- header bar: the things a GNOME user expects to find
    let header = gtk4::HeaderBar::new();
    let button = |label: &str, tip: &str, command: &'static str| {
        let b = gtk4::Button::with_label(label);
        b.set_tooltip_text(Some(tip));
        let forge = forge.clone();
        let area = area.clone();
        b.connect_clicked(move |_| {
            forge.run_command(command);
            area.grab_focus();
        });
        b
    };
    header.pack_start(&button("Open", "Open a file with the system picker", "find-file-dialog"));
    header.pack_start(&button("Folder", "Open a folder with the system picker", "find-directory-dialog"));
    header.pack_start(&button("Save", "Save this buffer", "save-buffer"));
    let menu = gio::Menu::new();
    for (label, action) in [("Run Command…", "win.m-x"), ("Save As…", "win.save-as"), ("Quit", "win.quit")] {
        menu.append(Some(label), Some(action));
    }
    let menu_button = gtk4::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).build();
    header.pack_end(&menu_button);
    win.set_titlebar(Some(&header));
    for (name, command) in [("m-x", "execute-extended-command"), ("save-as", "write-file-dialog"), ("quit", "save-buffers-kill-terminal")] {
        let action = gio::SimpleAction::new(name, None);
        let forge = forge.clone();
        action.connect_activate(move |_, _| forge.run_command(command));
        win.add_action(&action);
    }

    // --- drawing
    {
        let state = state.clone();
        area.set_draw_func(move |area, cr, w, h| draw(area, cr, w, h, &mut state.borrow_mut()));
    }

    // --- keys, text, mouse
    let im = gtk4::IMMulticontext::new();
    {
        let forge = forge.clone();
        im.connect_commit(move |_, text| {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => forge.key(KeyInput { key: Key::Char { c: c.to_string() }, control: false, meta: false, shift: false }),
                _ => forge.text(text.to_string()),
            }
        });
    }
    let key_controller = gtk4::EventControllerKey::new();
    key_controller.set_im_context(Some(&im));
    {
        let forge = forge.clone();
        let win = win.clone();
        key_controller.connect_key_pressed(move |_, keyval, _, mods| {
            // Ctrl+Shift+V pastes the system clipboard, as in terminals.
            if mods.contains(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK) && matches!(keyval.to_unicode(), Some('v' | 'V')) {
                let forge = forge.clone();
                win.clipboard().read_text_async(None::<&gio::Cancellable>, move |r| {
                    if let Ok(Some(t)) = r {
                        forge.text(t.to_string());
                    }
                });
                return glib::Propagation::Stop;
            }
            match keys::translate(keyval, mods) {
                Some(k) => {
                    forge.key(k);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
    }
    area.add_controller(key_controller);
    {
        let focus = gtk4::EventControllerFocus::new();
        let (im_in, im_out) = (im.clone(), im.clone());
        focus.connect_enter(move |_| im_in.focus_in());
        focus.connect_leave(move |_| im_out.focus_out());
        area.add_controller(focus);
        im.set_client_widget(Some(&area));
    }

    let cell_of = {
        let state = state.clone();
        move |x: f64, y: f64| {
            let (cw, ch) = state.borrow().cell;
            ((x / cw).floor().max(0.0) as u16, (y / ch).floor().max(0.0) as u16)
        }
    };
    let click = gtk4::GestureClick::builder().button(0).build();
    {
        let (forge, area, cell_of, state) = (forge.clone(), area.clone(), cell_of.clone(), state.clone());
        click.connect_pressed(move |g, _, x, y| {
            area.grab_focus();
            let (c, r) = cell_of(x, y);
            match g.current_button() {
                1 => {
                    state.borrow_mut().dragging = true;
                    forge.mouse(MouseKind::Down, c, r);
                }
                3 => forge.mouse(MouseKind::RightDown, c, r),
                2 => forge.run_command("mouse-yank-primary"),
                _ => {}
            }
        });
    }
    {
        let (forge, cell_of, state) = (forge.clone(), cell_of.clone(), state.clone());
        click.connect_released(move |g, _, x, y| {
            if g.current_button() == 1 {
                state.borrow_mut().dragging = false;
                let (c, r) = cell_of(x, y);
                forge.mouse(MouseKind::Up, c, r);
            }
        });
    }
    area.add_controller(click);
    let motion = gtk4::EventControllerMotion::new();
    {
        let (forge, cell_of, state) = (forge.clone(), cell_of.clone(), state.clone());
        motion.connect_motion(move |_, x, y| {
            if state.borrow().dragging {
                let (c, r) = cell_of(x, y);
                forge.mouse(MouseKind::Drag, c, r);
            }
        });
    }
    area.add_controller(motion);
    let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::DISCRETE);
    {
        let (forge, state) = (forge.clone(), state.clone());
        scroll.connect_scroll(move |c, _, dy| {
            let _ = state.borrow();
            let kind = if dy < 0.0 { MouseKind::ScrollUp } else { MouseKind::ScrollDown };
            let (col, row) = (c.widget().map_or(0, |w| (w.width() / 2) as u16 / 9), 3);
            for _ in 0..(dy.abs().ceil() as u32).max(1) * 3 {
                forge.mouse(kind, col, row);
            }
            glib::Propagation::Stop
        });
    }
    area.add_controller(scroll);

    // C-g that the editor can't see because it is busy.
    {
        let forge = forge.clone();
        let c = gtk4::EventControllerKey::new();
        c.set_propagation_phase(gtk4::PropagationPhase::Capture);
        c.connect_key_pressed(move |_, keyval, _, mods| {
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && matches!(keyval.to_unicode(), Some('g')) {
                forge.interrupt();
            }
            glib::Propagation::Proceed
        });
        win.add_controller(c);
    }

    win.set_child(Some(&area));
    {
        let forge = forge.clone();
        win.connect_close_request(move |_| {
            forge.save_session();
            glib::Propagation::Proceed
        });
    }
    win.present();
    area.grab_focus();

    // --- the editor's thread draws; this thread asks for grids and shows them
    let (tx, rx) = async_channel::bounded::<Grid>(2);
    {
        let (forge, size, dark) = (forge.clone(), size.clone(), dark.clone());
        // Sizes are read on the main thread and sent over a channel.
        let (size_tx, size_rx) = std::sync::mpsc::channel::<(u16, u16, bool)>();
        glib::timeout_add_local(Duration::from_millis(100), {
            let (size, dark, area) = (size.clone(), dark.clone(), area.clone());
            let state = state.clone();
            move || {
                dark.set(prefers_dark());
                let (cw, ch) = state.borrow().cell;
                let cols = ((f64::from(area.width()) / cw).floor() as u16).max(20);
                let rows = ((f64::from(area.height()) / ch).floor() as u16).max(5);
                size.set((cols, rows));
                let _ = size_tx.send((cols, rows, dark.get()));
                glib::ControlFlow::Continue
            }
        });
        std::thread::spawn(move || {
            let mut current = (100, 30, true);
            loop {
                while let Ok(s) = size_rx.try_recv() {
                    current = s;
                }
                let grid = pollster::block_on(forge.grid(current.0, current.1, current.2));
                if tx.send_blocking(grid).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        });
    }
    {
        let (area, win, state, forge) = (area.clone(), win.clone(), state.clone(), forge.clone());
        glib::spawn_future_local(async move {
            let mut shown = String::new();
            while let Ok(grid) = rx.recv().await {
                let title = format!("{}{} — Forge", if grid.modified { "• " } else { "" }, grid.title);
                if title != shown {
                    win.set_title(Some(&title));
                    shown = title;
                }
                if grid.quit {
                    win.close();
                    return;
                }
                if let Some(d) = grid.dialog.clone() {
                    let fresh = state.borrow().last_dialog != d.id;
                    if fresh {
                        state.borrow_mut().last_dialog = d.id;
                        picker(&win, forge.clone(), d);
                    }
                }
                let same = state.borrow().grid.as_ref() == Some(&grid);
                if !same {
                    state.borrow_mut().grid = Some(grid);
                    area.queue_draw();
                }
            }
        });
    }
    // The kill ring is the system clipboard too.
    {
        let (forge, state, win) = (forge.clone(), state.clone(), win.clone());
        glib::timeout_add_local(Duration::from_millis(400), move || {
            let (forge, state, win) = (forge.clone(), state.clone(), win.clone());
            glib::spawn_future_local(async move {
                let top = forge.kill_top().await;
                if let Some(t) = top
                    && state.borrow().last_clip.as_deref() != Some(&t)
                {
                    win.clipboard().set_text(&t);
                    state.borrow_mut().last_clip = Some(t);
                }
            });
            glib::ControlFlow::Continue
        });
    }
}

/// Show GTK's file picker for what the editor asked.
fn picker(win: &gtk4::ApplicationWindow, forge: Arc<Forge>, d: GridDialog) {
    let dialog = gtk4::FileDialog::builder().title(d.title.as_str()).modal(true).build();
    if !d.start.is_empty() {
        dialog.set_initial_folder(Some(&gio::File::for_path(&d.start)));
    }
    let id = d.id;
    let answer = move |r: Result<gio::File, glib::Error>| {
        let path = r.ok().and_then(|f| f.path()).map(|p| p.display().to_string());
        forge.dialog_result(id, path);
    };
    match d.kind.as_str() {
        "open_folder" => dialog.select_folder(Some(win), None::<&gio::Cancellable>, answer),
        "save_as" => {
            dialog.set_initial_name(Some(&d.name));
            dialog.save(Some(win), None::<&gio::Cancellable>, answer);
        }
        _ => dialog.open(Some(win), None::<&gio::Cancellable>, answer),
    }
}

fn hex(c: u32) -> (f64, f64, f64) {
    (f64::from(c >> 16 & 0xff) / 255.0, f64::from(c >> 8 & 0xff) / 255.0, f64::from(c & 0xff) / 255.0)
}

fn draw(area: &gtk4::DrawingArea, cr: &gtk4::cairo::Context, w: i32, h: i32, st: &mut State) {
    use gtk4::pango;
    let layout = area.create_pango_layout(None);
    let mut base = pango::FontDescription::from_string("Monospace 12");
    base.set_weight(pango::Weight::Normal);
    layout.set_font_description(Some(&base));
    layout.set_text("M");
    let (pw, ph) = layout.pixel_size();
    st.cell = (f64::from(pw.max(1)), f64::from(ph.max(1)));
    let (cw, ch) = st.cell;
    let Some(grid) = st.grid.clone() else {
        cr.set_source_rgb(0.12, 0.12, 0.18);
        let _ = cr.paint();
        return;
    };
    let (r, g, b) = hex(grid.bg);
    cr.set_source_rgb(r, g, b);
    let _ = cr.paint();
    let fonts: Vec<pango::FontDescription> = (0..4)
        .map(|i| {
            let mut f = base.clone();
            if i & 1 != 0 {
                f.set_weight(pango::Weight::Bold);
            }
            if i & 2 != 0 {
                f.set_style(pango::Style::Italic);
            }
            f
        })
        .collect();
    for run in &grid.runs {
        let (x, y) = (f64::from(run.col) * cw, f64::from(run.row) * ch);
        let rw = f64::from(run.cols) * cw;
        if run.bg != grid.bg {
            let (r, g, b) = hex(run.bg);
            cr.set_source_rgb(r, g, b);
            cr.rectangle(x, y, rw, ch);
            let _ = cr.fill();
        }
        let (r, g, b) = hex(run.fg);
        cr.set_source_rgb(r, g, b);
        if !run.text.trim().is_empty() {
            layout.set_font_description(Some(&fonts[usize::from(run.flags & 1) | usize::from(run.flags >> 1 & 1) << 1]));
            layout.set_text(&run.text);
            cr.move_to(x, y);
            pangocairo::functions::show_layout(cr, &layout);
        }
        if run.flags & 4 != 0 {
            cr.rectangle(x, y + ch - 2.0, rw, 1.0);
            let _ = cr.fill();
        }
        if run.flags & 8 != 0 {
            cr.rectangle(x, y + ch / 2.0, rw, 1.0);
            let _ = cr.fill();
        }
    }
    for p in &grid.pictures {
        if scene::is_scene(&p.path) {
            let (x, y) = (f64::from(p.col) * cw, f64::from(p.row) * ch);
            scene::draw(&mut st.scenes, cr, &p.path, x, y, f64::from(p.cols) * cw, f64::from(p.rows) * ch);
            continue;
        }
        let Some(tex) = texture(st, &p.path) else { continue };
        let (x, y) = (f64::from(p.col) * cw, f64::from(p.row) * ch);
        let (bw, bh) = (f64::from(p.cols) * cw, f64::from(p.rows) * ch);
        let (tw, th) = (f64::from(tex.width()), f64::from(tex.height()));
        let scale = (bw / tw).min(bh / th).min(1.0);
        let (dw, dh) = (tw * scale, th * scale);
        let snapshot = gtk4::Snapshot::new();
        snapshot.append_texture(&tex, &gtk4::graphene::Rect::new(x as f32, y as f32, dw as f32, dh as f32));
        if let Some(node) = snapshot.to_node() {
            node.draw(cr);
        }
    }
    // The cursor: a translucent block.
    let (r, g, b) = hex(grid.fg);
    cr.set_source_rgba(r, g, b, 0.55);
    cr.rectangle(f64::from(grid.cursor_col) * cw, f64::from(grid.cursor_row) * ch, cw, ch);
    let _ = cr.fill();
    let _ = (w, h);
}

fn texture(st: &mut State, path: &str) -> Option<gdk::Texture> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    if let Some((t, tex)) = st.textures.get(path)
        && *t == modified
    {
        return tex.clone();
    }
    let tex = gdk::Texture::from_filename(path).ok();
    st.textures.insert(path.to_string(), (modified, tex.clone()));
    tex
}
