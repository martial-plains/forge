// SPDX-License-Identifier: MIT OR Apache-2.0
//! The screen as a grid of styled runs, for frontends that draw Forge with
//! their own text and pictures but want the terminal's whole interface
//! (windows, mode lines, the menu bar, tool bar, tabs, popups, scroll
//! bars): the GNOME and Windows apps.
//!
//! The terminal UI paints into a cell buffer; this resolves every cell's
//! colour to RGB (so a frontend needs no palette), merges neighbours with
//! the same style into runs and reports where pictures go. The macOS app
//! uses the richer `Frame` instead.

use forge_editor::app::App;
use forge_terminal::caps::{GraphicsProto, TermCaps};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const UNDERLINE: u8 = 4;
pub const STRIKE: u8 = 8;

/// A stretch of cells on one row that share a style.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GridRun {
    pub row: u16,
    pub col: u16,
    /// Columns the run covers (a wide character covers two).
    pub cols: u16,
    pub text: String,
    /// `0xRRGGBB`.
    pub fg: u32,
    pub bg: u32,
    /// [`BOLD`] | [`ITALIC`] | [`UNDERLINE`] | [`STRIKE`].
    pub flags: u8,
}

/// A picture to draw over a rectangle of cells (an image in a buffer, an
/// Org preview, a canvas).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GridPicture {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
    pub path: String,
}

/// The system file picker the editor wants shown (`open_file`,
/// `open_folder` or `save_as`); answer with `Forge::dialog_result`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GridDialog {
    pub id: u32,
    pub kind: String,
    pub title: String,
    /// The folder to start in.
    pub start: String,
    /// For `save_as`, the name to offer.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub runs: Vec<GridRun>,
    pub pictures: Vec<GridPicture>,
    pub cursor_col: u16,
    pub cursor_row: u16,
    /// The colours of an unstyled cell, for the window's own background.
    pub fg: u32,
    pub bg: u32,
    /// The current buffer's name, for the window title.
    pub title: String,
    pub modified: bool,
    /// A plugin is running long; the picture is the last one.
    pub busy: Option<String>,
    /// The editor is done: close the window.
    pub quit: bool,
    pub crashed: Option<String>,
    /// A file picker to show.
    pub dialog: Option<GridDialog>,
}

struct Palette {
    fg: u32,
    bg: u32,
    ansi: [u32; 16],
}

const DARK: Palette = Palette {
    fg: 0xcdd6f4,
    bg: 0x1e1e2e,
    ansi: [
        0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, // 0-7
        0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5,
        0xa6adc8, // 8-15
    ],
};

const LIGHT: Palette = Palette {
    fg: 0x4c4f69,
    bg: 0xeff1f5,
    ansi: [
        0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, //
        0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
    ],
};

fn indexed(n: u8, p: &Palette) -> u32 {
    match n {
        0..=15 => p.ansi[n as usize],
        16..=231 => {
            let n = u32::from(n) - 16;
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36) << 16) | (level((n / 6) % 6) << 8) | level(n % 6)
        }
        232..=255 => {
            let v = 8 + (u32::from(n) - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}

/// `None` is the default colour.
fn rgb(c: Color, p: &Palette) -> Option<u32> {
    Some(match c {
        Color::Reset => return None,
        Color::Black => p.ansi[0],
        Color::Red => p.ansi[1],
        Color::Green => p.ansi[2],
        Color::Yellow => p.ansi[3],
        Color::Blue => p.ansi[4],
        Color::Magenta => p.ansi[5],
        Color::Cyan => p.ansi[6],
        Color::Gray => p.ansi[7],
        Color::DarkGray => p.ansi[8],
        Color::LightRed => p.ansi[9],
        Color::LightGreen => p.ansi[10],
        Color::LightYellow => p.ansi[11],
        Color::LightBlue => p.ansi[12],
        Color::LightMagenta => p.ansi[13],
        Color::LightCyan => p.ansi[14],
        Color::White => 0xffffff,
        Color::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
        Color::Indexed(n) => indexed(n, p),
    })
}

/// Blend `c` towards `towards` (for dim text).
fn dim(c: u32, towards: u32) -> u32 {
    let mix = |s: u32| ((c >> s & 0xff) + (towards >> s & 0xff)) / 2;
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// What a frontend asks of the terminal UI: kitty-style pictures, and colour
/// underlines it can draw.
pub fn caps() -> TermCaps {
    TermCaps {
        underline_color: true,
        graphics: GraphicsProto::Kitty,
    }
}

pub fn build(app: &mut App, cols: u16, rows: u16, dark: bool) -> Grid {
    let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(3, 500));
    let p = if dark { &DARK } else { &LIGHT };
    let mut term = Terminal::new(TestBackend::new(cols, rows)).expect("a test terminal");
    let mut slots = Vec::new();
    term.draw(|f| slots = forge_terminal::ui::render_with(f, app, caps()))
        .expect("drawing");
    let cursor = term
        .get_cursor_position()
        .map(|p| (p.x, p.y))
        .unwrap_or((0, 0));
    let buf = term.backend().buffer();
    let mut runs: Vec<GridRun> = Vec::new();
    for y in 0..rows {
        let mut x = 0;
        let mut current: Option<GridRun> = None;
        while x < cols {
            let cell = &buf[(x, y)];
            let sym = cell.symbol();
            let width = (sym.width() as u16).max(1);
            let m = cell.modifier;
            let (mut fg, mut bg) = (
                rgb(cell.fg, p).unwrap_or(p.fg),
                rgb(cell.bg, p).unwrap_or(p.bg),
            );
            if m.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if m.contains(Modifier::DIM) {
                fg = dim(fg, bg);
            }
            let mut flags = 0;
            if m.contains(Modifier::BOLD) {
                flags |= BOLD;
            }
            if m.contains(Modifier::ITALIC) {
                flags |= ITALIC;
            }
            if m.contains(Modifier::UNDERLINED) {
                flags |= UNDERLINE;
            }
            if m.contains(Modifier::CROSSED_OUT) {
                flags |= STRIKE;
            }
            let wide = width > 1;
            match &mut current {
                // Wide characters stand alone so the columns line up.
                Some(run)
                    if !wide
                        && run.fg == fg
                        && run.bg == bg
                        && run.flags == flags
                        && run.cols == run.text.chars().count() as u16 =>
                {
                    run.text.push_str(sym);
                    run.cols += 1;
                }
                _ => {
                    if let Some(run) = current.take() {
                        runs.push(run);
                    }
                    current = Some(GridRun {
                        row: y,
                        col: x,
                        cols: width,
                        text: sym.to_string(),
                        fg,
                        bg,
                        flags,
                    });
                    if wide {
                        runs.push(current.take().expect("just set"));
                    }
                }
            }
            x += width;
        }
        if let Some(run) = current.take() {
            runs.push(run);
        }
    }
    // Rows of blanks in the default style need no drawing at all.
    runs.retain(|r| {
        !(r.bg == p.bg && r.text.chars().all(|c| c == ' ') && r.flags & (UNDERLINE | STRIKE) == 0)
    });
    let (title, modified) = {
        let b = app.buf_ref();
        (b.name().to_string(), b.is_modified())
    };
    Grid {
        cols,
        rows,
        runs,
        pictures: slots
            .into_iter()
            .map(|s| GridPicture {
                col: s.x,
                row: s.y,
                cols: s.w,
                rows: s.h,
                path: s.path,
            })
            .collect(),
        cursor_col: cursor.0,
        cursor_row: cursor.1,
        fg: p.fg,
        bg: p.bg,
        title,
        modified,
        busy: None,
        quit: false,
        crashed: None,
        dialog: forge_editor::dialog::pending().map(|r| GridDialog {
            id: r.id,
            kind: match r.kind {
                forge_editor::dialog::Kind::OpenFile => "open_file",
                forge_editor::dialog::Kind::OpenFolder => "open_folder",
                forge_editor::dialog::Kind::SaveAs => "save_as",
            }
            .into(),
            title: r.title,
            start: r.start,
            name: r.name,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_resolve_to_rgb() {
        assert_eq!(rgb(Color::Rgb(1, 2, 3), &DARK), Some(0x010203));
        assert_eq!(rgb(Color::Reset, &DARK), None);
        assert_eq!(rgb(Color::Red, &DARK), Some(DARK.ansi[1]));
        assert_eq!(indexed(16, &DARK), 0x000000);
        assert_eq!(indexed(231, &DARK), 0xffffff);
        assert_eq!(indexed(232, &DARK), 0x080808);
        assert_eq!(indexed(255, &DARK), 0xeeeeee);
        assert_eq!(indexed(196, &DARK), 0xff0000);
    }

    #[test]
    fn dimming_moves_towards_the_background() {
        assert_eq!(dim(0xffffff, 0x000000), 0x7f7f7f);
    }
}
