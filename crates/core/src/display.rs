// SPDX-License-Identifier: MIT OR Apache-2.0
//! Redisplay: what every window should show, as text and faces.
//!
//! The core never draws. [`redisplay`] lays out the windows in an area,
//! scrolls each so its point is visible, and returns rows of styled
//! [`Span`]s already clipped to the window, plus mode lines and cursor
//! positions. A frontend (the terminal UI today, a GUI later) maps
//! [`Face`]s to colors and paints the cells.

use crate::app::Editor;
use crate::buffer::{Buffer, Pos};
use crate::decoration::{DecoFace, FillColumnChar, FillColumnColumn, LineNumbers, Wrap};
use crate::syntax::SyntaxFace;
use crate::width::{self, grapheme_width};
use crate::window::{FrameId, Rect, WindowId};
use std::path::PathBuf;

/// The display properties of an `image` specification. Only the properties Forge can use are kept; the rest are
/// parsed and ignored.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageDescriptor {
    /// The picture file, from `:file`.
    pub path: PathBuf,
    /// Raw picture data, from `:data` (the editor writes it to a temp file
    /// before a frontend draws it).
    pub data: Option<Vec<u8>>,
    /// Pixels to add around the image, as `(x, y)`.
    pub margin: (i64, i64),
    /// Ascent as a percentage of the height; `None` centers on the
    /// baseline.
    pub ascent: Option<f64>,
    /// Shadow width in pixels (negative: pressed).
    pub relief: i64,
    /// Target size in pixels (None: natural size).
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub max_width: Option<f64>,
    pub max_height: Option<f64>,
    /// Scale factor.
    pub scale: Option<f64>,
    /// Rotation in degrees (multiples of 90).
    pub rotation: i32,
    /// Horizontal flip.
    pub flip_h: bool,
    /// Centre the picture in the window (a plugin's banner), not at the
    /// left edge.
    pub center: bool,
    /// Slice `(x, y, w, h)`: pixels, or fractions of the picture.
    pub slice: Option<[SliceValue; 4]>,
}

/// One number of a `(slice x y width height)` specification: an integer is
/// pixels, a float from 0.0 to 1.0 a fraction of the picture's width or height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceValue {
    pub value: f64,
    pub fraction: bool,
}

impl SliceValue {
    pub fn px(value: u32) -> Self {
        Self {
            value: value as f64,
            fraction: false,
        }
    }

    fn resolve(self, whole: u32) -> u32 {
        if self.fraction {
            (self.value * whole as f64).round().max(0.0) as u32
        } else {
            self.value.max(0.0) as u32
        }
    }
}

impl ImageDescriptor {
    /// The slice in pixels for a picture of `natural` size, kept inside it.
    pub fn slice_rect(&self, natural: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
        let [x, y, w, h] = self.slice?;
        let (iw, ih) = natural;
        let x = x.resolve(iw).min(iw.saturating_sub(1));
        let y = y.resolve(ih).min(ih.saturating_sub(1));
        let w = w.resolve(iw).min(iw - x).max(1);
        let h = h.resolve(ih).min(ih - y).max(1);
        Some((x, y, w, h))
    }
}

impl ImageDescriptor {
    /// The target size in pixels, from `:width`, `:height`, `:scale`,
    /// `:max-width` and `:max-height`, given the picture's natural size.
    pub fn target_size(&self, natural: (u32, u32)) -> (u32, u32) {
        let (mut nw, mut nh) = (natural.0 as f64, natural.1 as f64);
        match (self.width, self.height) {
            (Some(w), Some(h)) => {
                nw = w;
                nh = h;
            }
            (Some(w), None) => {
                nh *= w / nw;
                nw = w;
            }
            (None, Some(h)) => {
                nw *= h / nh;
                nh = h;
            }
            (None, None) => {}
        }
        if let Some(s) = self.scale {
            nw *= s;
            nh *= s;
        }
        if let Some(mw) = self.max_width
            && nw > mw
        {
            nh *= mw / nw;
            nw = mw;
        }
        if let Some(mh) = self.max_height
            && nh > mh
        {
            nw *= mh / nh;
            nh = mh;
        }
        ((nw.max(1.0)) as u32, (nh.max(1.0)) as u32)
    }
}

/// What a piece of text is, for the frontend to pick colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Default,
    /// The active region (between mark and point).
    Region,
    /// The current search match.
    Match,
    /// Control characters shown as ^X.
    Escape,
    /// `$` where a long line is cut off.
    Truncation,
    ModeLine,
    ModeLineInactive,
    /// The line between side-by-side windows.
    Border,
    /// Code, by what it is (from the syntax tree).
    Syntax(SyntaxFace),
    /// A decoration (usually drawn underlined), over the text's syntax face.
    Decoration(DecoFace, Option<SyntaxFace>),
    LineNumber,
    /// The number of the line point is on.
    LineNumberCurrent,
    /// show-paren-mode: the bracket at point and its match...
    ParenMatch,
    /// ...or a bracket with no match.
    ParenMismatch,
    /// Blanks at the end of a line (show-trailing-whitespace).
    TrailingWhitespace,
    /// Spaces and tabs drawn by whitespace-mode.
    Whitespace,
    /// The line display-fill-column-indicator-mode draws.
    FillColumn,
    /// The "..." after a line whose following lines are folded away.
    Fold,
    /// A completion popup's rows, the selected one, and annotations.
    Popup,
    PopupSelected,
    PopupAnnotation,
    /// The icon/margin a completion row shows before its text.
    PopupMargin,
    /// The part of a completion row's text that matches what's typed.
    PopupMatch,
    /// A completion row whose candidate is deprecated (dimmed/struck).
    PopupDeprecated,
    /// The auxiliary popup beside the completion popup (corfu-popupinfo).
    PopupInfo,
    /// The menu bar's row, the open title or item, and an item whose
    /// command isn't defined (a plugin unloaded).
    MenuBar,
    MenuSelected,
    MenuDisabled,
    /// The tab bar's row, and the tab that is current.
    TabBar,
    TabBarSelected,
    /// The tool bar's row, its hovered item, and an item that can't be
    /// chosen. The modifier bar uses the same faces.
    ToolBar,
    ToolBarSelected,
    ToolBarDisabled,
    /// The vertical scroll bar's track and thumb (scroll-bar-mode).
    ScrollBar,
}

impl Face {
    /// The name Lua themes and the macOS app use for this face
    /// ("mode-line", "keyword", "error", ...). A decoration answers with
    /// its own name; what it underlines keeps its own.
    pub fn name(&self) -> &'static str {
        match self {
            Face::Default => "default",
            Face::Region => "region",
            Face::Match => "match",
            Face::Escape => "escape",
            Face::Truncation => "truncation",
            Face::ModeLine => "mode-line",
            Face::ModeLineInactive => "mode-line-inactive",
            Face::Border => "border",
            Face::Syntax(f) => f.name(),
            Face::Decoration(d, _) => d.name(),
            Face::LineNumber => "line-number",
            Face::LineNumberCurrent => "line-number-current",
            Face::ParenMatch => "paren-match",
            Face::ParenMismatch => "paren-mismatch",
            Face::TrailingWhitespace => "trailing-whitespace",
            Face::Whitespace => "whitespace",
            Face::FillColumn => "fill-column-indicator",
            Face::Fold => "fold",
            Face::Popup => "popup",
            Face::PopupSelected => "popup-selected",
            Face::PopupAnnotation => "popup-annotation",
            Face::PopupMargin => "popup-margin",
            Face::PopupMatch => "popup-match",
            Face::PopupDeprecated => "popup-deprecated",
            Face::PopupInfo => "popup-info",
            Face::MenuBar => "menu-bar",
            Face::MenuSelected => "menu-selected",
            Face::MenuDisabled => "menu-disabled",
            Face::TabBar => "tab-bar",
            Face::TabBarSelected => "tab-bar-selected",
            Face::ToolBar => "tool-bar",
            Face::ToolBarSelected => "tool-bar-selected",
            Face::ToolBarDisabled => "tool-bar-disabled",
            Face::ScrollBar => "scroll-bar",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub face: Face,
}

/// What a part of the mode line stands for, so a click on it can do what
/// Forge does there (`mode-line-modified`, `mode-line-buffer-identification`,
/// `mode-line-position`, `mode-line-modes`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeLineTarget {
    /// The input method and coding system (`U:`).
    Mule,
    /// The modified / read-only flag (`**`, `%%`).
    Modified,
    /// The frame's name, on a terminal.
    Frame,
    BufferName,
    /// The size indication (`12k`).
    Size,
    /// How far down the buffer the window is (`Top`, `42%`).
    Percent,
    Line,
    Column,
    /// The version control state (`Git-main`).
    Vc,
    /// The major mode's name.
    MajorMode,
    /// A minor mode's lighter; holds the mode's name.
    MinorMode(String),
    /// A word from the mode line's extras (`Def`, `Compiling`, ...).
    Extra(String),
    /// Anything else the mode line shows (a plugin's note).
    Info,
}

/// A clickable stretch of the mode line, in cells from its left end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeLineHit {
    pub from: u16,
    pub to: u16,
    pub target: ModeLineTarget,
}

impl ModeLineHit {
    /// The target at `column`, if any.
    pub fn at(hits: &[ModeLineHit], column: u16) -> Option<&ModeLineTarget> {
        hits.iter()
            .find(|h| column >= h.from && column < h.to)
            .map(|h| &h.target)
    }
}

/// One window, ready to paint.
#[derive(Debug, Clone)]
pub struct WindowView {
    pub id: WindowId,
    /// Whole window: text rows, then the mode line row.
    pub rect: Rect,
    /// The text is on a grid of cells this many percent of the normal size
    /// (the buffer's text scale; 100 is normal). `rows`, `text_width`, the
    /// cursor, highlighted rows, pictures, style runs and the bars are
    /// counted in those cells from the window's top-left corner, so the
    /// window shows `rect.height - 1` rows only when this is 100.
    pub cell_scale_pct: u16,
    /// Text rows (`rect.height - 1` of them), each at most `text_width` cells.
    pub rows: Vec<Vec<Span>>,
    pub text_width: u16,
    /// Draw a vertical border in the last column of the text rows.
    pub border: bool,
    pub mode_line: Vec<Span>,
    /// The clickable parts of `mode_line`.
    pub mode_line_hits: Vec<ModeLineHit>,
    pub selected: bool,
    /// Screen position of this window's point.
    pub cursor: (u16, u16),
    /// Columns of line numbers at the start of each row (included in
    /// `text_width`).
    pub gutter: u16,
    /// Rows at the top that stay put while the text scrolls (Org's header).
    pub header_rows: u16,
    /// hl-line-mode: rows showing point's line, to draw with a background.
    pub hl_rows: Vec<u16>,
    /// display-fill-column-indicator-mode: the column (within the
    /// window, after the gutter) to draw a thin line at, where rows are
    /// shorter than that.
    pub fill_column: Option<u16>,
    /// The character to draw it with, when `fill_column` is set.
    pub fill_column_character: Option<String>,
    /// A completion popup at point (selected window only), drawn over
    /// the text.
    pub popup: Option<PopupView>,
    /// The auxiliary popup beside it (corfu-popupinfo): the selection's
    /// documentation and/or location.
    pub popup_info: Option<PopupView>,
    /// How the cursor looks (the buffer's `cursor` option).
    pub cursor_shape: CursorShape,
    /// Pictures to draw over blank rows of the text (frontends that can).
    pub images: Vec<ImageView>,
    /// A vertical scroll bar for this window: the first text row and how
    /// many rows the thumb covers (the last column of `text_width`). None
    /// when the scroll-bar option is off or there is no room.
    pub scroll_bar: Option<(u16, u16)>,
    /// Fractional vertical pixel offset of the first displayed line: how
    /// many pixels of it are clipped off its top (true pixel scrolling).
    /// Applied by the frontend over `rows[0]`.
    pub scroll_y_px: usize,
    /// A horizontal scroll bar for this window: the first text column and
    /// how many columns the thumb covers, within the text area (after the
    /// gutter). Drawn on the row just above the mode line. None when the
    /// horizontal-scroll-bar option is off or there is no room.
    pub horizontal_scroll_bar: Option<(u16, u16)>,
    /// Stretches drawn at another height or raised (the `display`
    /// property's `height` and `raise`), for frontends that can scale text.
    /// Rows without one are drawn as usual.
    pub style_runs: Vec<StyleRun>,
}

/// A stretch of one text row drawn with its own height and raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleRun {
    /// Window row (header rows included) and the cells it covers, counted
    /// from the window's left edge.
    pub row: u16,
    pub col: u16,
    pub cells: u16,
    /// See [`crate::display_spec::TextStyle`].
    pub height_pct: u16,
    pub raise_pct: i16,
}

/// A picture for a frontend to draw over `rows` blank text rows starting
/// at text row `row` (header row included).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageView {
    pub row: u16,
    pub rows: u16,
    pub path: String,
    /// The image's display properties (scale, rotation, slice, ...).
    pub descriptor: Option<ImageDescriptor>,
}

/// A popup's place on the screen and its rows, each `width` cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupView {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub rows: Vec<Vec<Span>>,
}

/// One row of a completion popup: the candidate text (what insert uses),
/// the margin drawn before it (an icon or kind, or empty), the annotation
/// after it (or empty), whether it is the selected row, and whether the
/// candidate is deprecated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PopupRow {
    pub text: String,
    pub margin: String,
    pub annotation: String,
    pub selected: bool,
    pub deprecated: bool,
}

/// What the frontend should show for a completion popup: under the text
/// starting at `anchor` (a char offset in the selected window's buffer),
/// the rows to show, and the input typed so far (to highlight the matched
/// part of each candidate).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PopupSpec {
    pub anchor: usize,
    pub rows: Vec<PopupRow>,
    /// The text typed into the popup. Each row's leading part that matches
    /// it is drawn with [`Face::PopupMatch`].
    pub input: String,
    /// Preferred width in columns (0: as wide as the rows need). Clamped
    /// to the window by whoever draws it.
    pub width: u32,
}

/// The auxiliary popup beside the completion popup (corfu-popupinfo): the
/// lines to show (already wrapped and scrolled), one per row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoSpec {
    pub lines: Vec<String>,
    /// Preferred width in columns (0: as wide as the lines need).
    pub width: u32,
}

pub use crate::decoration::CursorShape;

/// Things redisplay needs to know that live outside the core.
#[derive(Debug, Clone, Default)]
pub struct DisplayOptions {
    /// Highlight in the selected window (isearch, query-replace match).
    pub highlight: Option<(Pos, Pos)>,
    /// Extra words for the mode line's mode list, e.g. "Def" while a
    /// keyboard macro is being defined.
    pub mode_line_extra: Vec<String>,
    /// Line numbers for buffers that don't set their own (the
    /// line-numbers option). Never in special-mode buffers.
    pub line_numbers: LineNumbers,
    /// Defaults for buffers that don't set their own options.
    pub wrap: Wrap,
    pub hl_line: bool,
    pub fill_column_indicator: bool,
    pub show_trailing_whitespace: bool,
    /// Highlight the bracket at point and its match (selected window).
    pub show_paren: bool,
    /// A completion popup to show at point.
    pub popup: Option<PopupSpec>,
    /// The auxiliary popup beside it (corfu-popupinfo).
    pub popup_info: Option<InfoSpec>,
    /// The menu bar's row and open panel, when menu-bar-mode is on.
    pub menu_bar: Option<crate::menu::MenuBarView>,
    /// The tool bar's row, when tool-bar-mode is on.
    pub tool_bar: Option<crate::toolbar::ToolBarView>,
    /// The modifier bar's row, when modifier-bar-mode is on.
    pub modifier_bar: Option<crate::toolbar::ModifierBarView>,
    /// Show a vertical scroll bar in the rightmost column of each window
    /// (the scroll-bar option).
    pub scroll_bar: bool,
    /// Show a horizontal scroll bar on the bottom row of each window
    /// (the horizontal-scroll-bar option).
    pub horizontal_scroll_bar: bool,
    /// Put the frame's name at the front of the mode line, the way Forge
    /// identifies a text-terminal frame (it shows a space in a GUI).
    pub mode_line_frame: bool,
    /// Leave the line number out of the mode line (`line-number-mode` off).
    pub mode_line_no_line: bool,
    /// Leave the column out (`column-number-mode` off).
    pub mode_line_no_column: bool,
    /// Show the buffer's size (`size-indication-mode`).
    pub mode_line_size: bool,
    /// The display draws text at sizes other than the cell grid's (the
    /// GUI): `height` widens characters and text scaling applies. Off in
    /// a terminal, where every character is one cell tall.
    pub pixel_text: bool,
    /// Blank columns kept at the left and right of every window's text
    /// (the left_margin_width and right_margin_width options).
    pub left_margin: usize,
    pub right_margin: usize,
}

/// Lay out and scroll every window of the selected frame in `area`, and
/// describe its contents.
pub fn redisplay(editor: &mut Editor, area: Rect, opts: &DisplayOptions) -> Vec<WindowView> {
    let frame = editor.windows.selected_frame_id();
    redisplay_frame(editor, frame, area, opts, true)
}

/// The same for any frame. `active` is true only for the focused frame:
/// the other frames still lay out, scroll and cursor their own windows,
/// but their mode line draws inactive and their popup is suppressed (this
/// is what a background frame looks like while another has focus).
pub fn redisplay_frame(
    editor: &mut Editor,
    frame: FrameId,
    area: Rect,
    opts: &DisplayOptions,
    active: bool,
) -> Vec<WindowView> {
    let selected = editor.windows.frame_selected_id(frame);
    let layout = editor.windows.frame_layout(frame, area);
    // Forge puts the frame's name on a text terminal's mode line, standing
    // in for the title bar a GUI frame has; other frontends leave it out.
    let frame_name = if opts.mode_line_frame {
        editor.windows.frame_name(frame).unwrap_or("").to_string()
    } else {
        String::new()
    };
    let mut views = Vec::with_capacity(layout.len());
    for (id, rect) in layout {
        let point = editor.window_point(id);
        let buf_id = editor.windows.get(id).expect("laid out").buffer;
        editor.sync_language(buf_id);
        let special = editor
            .state
            .get_buffer(buf_id)
            .is_some_and(|b| editor.modes.derives_from(b.major_mode(), "special-mode"));
        let w = editor.windows.get(id).expect("laid out");
        let Some(buf) = editor.state.buffers.get_mut(&buf_id) else {
            continue;
        };
        let is_sel = id == selected;
        // Only the focused frame's own selected window draws as active.
        let is_active = is_sel && active;
        let border = rect.x + rect.width < area.x + area.width;
        // The buffer's text scale (text-scale-adjust) draws this window's
        // text on a grid of bigger (or smaller) cells: lay it out in that
        // many fewer (or more) of them, and let the frontend draw the grid
        // scaled. Everything but the mode line is counted in those cells;
        // the window itself (`real_rect`) keeps its place on the screen.
        let cell_pct = crate::display_spec::scale_pct(buf.text_scale, opts.pixel_text);
        let real_rect = rect;
        let rect = if cell_pct == 100 {
            rect
        } else {
            let pct = u32::from(cell_pct);
            Rect {
                x: rect.x,
                y: rect.y,
                width: (u32::from(rect.width) * 100 / pct).clamp(1, u32::from(u16::MAX)) as u16,
                height: ((u32::from(rect.height).saturating_sub(1)) * 100 / pct)
                    .clamp(1, u32::from(u16::MAX - 1)) as u16
                    + 1,
            }
        };
        // Org: the header of a long table, kept on top when it scrolls off.
        let header_info = if buf.org_display.table_header && rect.height >= 4 {
            let pl = buf.offset_to_pos(point).line;
            // Where the window will start once point is in view.
            let rows = rect.height.saturating_sub(1).max(1) as usize;
            let top_est = if pl >= w.scroll_top + rows {
                pl + 1 - rows
            } else {
                w.scroll_top
            };
            buf.org_table_header_line(pl, top_est)
        } else {
            None
        };
        let header_rows = usize::from(header_info.is_some());
        let full_w = rect.width.saturating_sub(u16::from(border)).max(1) as usize;
        let o = buf.options.clone();
        // A picture buffer (a canvas, Image mode) is a picture, not lines
        // of text: numbering its placeholder rows is noise.
        let is_picture = buf.image.is_some();
        let numbers = o.line_numbers.unwrap_or(if special || is_picture {
            LineNumbers::Off
        } else {
            opts.line_numbers
        });
        let wrap = o.wrap.unwrap_or(opts.wrap);
        let hl_line = o.hl_line.unwrap_or(opts.hl_line) && is_sel;
        // The line is off when the mode is off, when its column is any
        // value but t or a number, or when its character is nil.
        let fci = o
            .fill_column_indicator
            .unwrap_or(opts.fill_column_indicator && !special)
            && !matches!(o.fill_column_indicator_column, Some(FillColumnColumn::Off))
            && !matches!(o.fill_column_indicator_character, Some(FillColumnChar::Off));
        let fci_column = match o.fill_column_indicator_column {
            Some(FillColumnColumn::Column(n)) => n,
            _ => buf.fill_column,
        };
        let trailing = o
            .show_trailing_whitespace
            .unwrap_or(opts.show_trailing_whitespace && !special);
        let whitespace = o.whitespace.unwrap_or(false);
        let num_w = match numbers {
            LineNumbers::Off => 0,
            // Digits of the last line number, at least 2, and a space.
            _ => (buf.line_count().to_string().len().max(2) + 1).min(full_w.saturating_sub(1)),
        };
        // The left margin (a `margin` display property), a column of
        // text before the line numbers. A margin takes a cell of space
        // after it, as does.
        let margin_w = match buf.margin_width("left-margin") {
            0 => opts.left_margin,
            n => (n + 1).max(opts.left_margin),
        }
        .min(full_w.saturating_sub(num_w + 1));
        // Fringes (`left-fringe`/`right-fringe` display properties) are one
        // cell wide, drawn with a character standing for the bitmap. The
        // right margin and right fringe sit between the text and the bar.
        let room = full_w.saturating_sub(margin_w + num_w);
        let lf_w = usize::from(room > 8 && buf.has_fringe("left-fringe"));
        let rf_w = usize::from(room > 8 && buf.has_fringe("right-fringe"));
        let rm_w = match buf.margin_width("right-margin") {
            0 => opts.right_margin,
            n => (n + 1).max(opts.right_margin),
        }
        .min(room.saturating_sub(lf_w + rf_w + 4));
        let right_w = rf_w + rm_w;
        let gutter = margin_w + num_w + lf_w;
        let content_w = (full_w - gutter).saturating_sub(right_w).max(1);
        // A vertical scroll bar takes the rightmost column of the text area.
        let bar_w = usize::from(opts.scroll_bar && rect.height >= 3 && content_w >= 4);
        let text_w = (content_w - bar_w).max(1);
        // A horizontal scroll bar takes the row just above the mode line.
        let hbar_h = usize::from(opts.horizontal_scroll_bar && rect.height >= 4 && text_w >= 4);
        let text_h = (rect.height.saturating_sub(1).max(1) as usize)
            .saturating_sub(header_rows)
            .saturating_sub(hbar_h)
            .max(1);

        // Point never stays in folded text: show it.
        if !buf.folds().is_empty() && buf.is_hidden(point) {
            buf.reveal(point);
        }
        let p = buf.offset_to_pos(point);
        let line_text = buf.line(p.line);
        let old_top = w.scroll_top;
        let scroll_y_px = w.scroll_y_px;
        // A partially clipped first line needs one more row below it, or
        // the bottom of the window would show a gap as the text moves up.
        let row_budget = text_h + usize::from(scroll_y_px > 0);
        let (mut top, mut hscroll) = (w.scroll_top, w.hscroll);
        if buf.folds().is_empty() {
            scroll_to_show(&mut top, p.line, text_h);
        } else {
            // Folded lines take no rows: buffer-line arithmetic would
            // scroll for point that's already shown, so count visible
            // lines instead. Lines count as shown, not as in the text.
            top = buf.visible_line(top.min(p.line));
            while buf.visible_lines_between(top, p.line) >= text_h {
                match buf.next_visible_line(top) {
                    Some(t) => top = t,
                    None => break,
                }
            }
        }
        let image_rows_est = crate::org_display::inline_image_rows().min(text_h.saturating_sub(1));
        let next_line = |buf: &Buffer, li: usize| buf.next_visible_line(li).unwrap_or(usize::MAX);
        // Org display-only text for lines other than point's.
        let org_on = buf.org_display.any();
        // Org indent mode: virtual indent per line (point's line too, so
        // the cursor never jumps sideways moving onto it). Org buffers
        // only; the flag is never set elsewhere.
        let indent_on = buf.org_display.indent && buf.major_mode() == crate::org::MODE;
        // Org num mode: section numbers after the stars (point's line too).
        let num_on = buf.org_display.num && buf.major_mode() == crate::org::MODE;
        let per_level = editor.org_indent_per_level;
        // Pictures from any mode (Org previews, a plugin's pins, the text
        // browser's cache).
        let pics_on = org_on
            || !buf.inline_images.is_empty()
            || !buf.plugin_images.is_empty()
            || buf.has_display_image();
        let has_display = buf.has_display();
        // The buffer's text scale (text-scale-adjust), as a percentage.
        // (Drawn by scaling the grid, so no character is widened for it.)
        let base_pct = 100;
        let style_on = buf.has_display_style();
        // Text shown in place of a line's own (not point's): Org's
        // stand-ins, else display replacements from a plugin.
        // Descriptive links stay collapsed to their description wherever
        // point is with `org-link-descriptive`: the text under
        // the cursor never jumps. Point inside the hidden markup is drawn at
        // the next visible character (see `point_col` below).
        let avoid = |_li: usize| -> Option<usize> { None };
        // Taller text takes more cells: widen a line's text (and what it
        // stands for) when a `height` or the buffer's scale makes it so.
        let tall_on = style_on && opts.pixel_text;
        let widen = |buf: &Buffer, li: usize, base: Option<(String, Vec<usize>)>| {
            if tall_on {
                buf.tall_line(li, base.clone(), base_pct)
                    .map(|t| (t.text, t.src))
                    .or(base)
            } else {
                base
            }
        };
        let shown = |buf: &Buffer, li: usize| -> Option<String> {
            if (li != p.line || buf.inline_image(li).is_some())
                && org_on
                && let Some(t) = buf.org_display_line(li)
            {
                return Some(t);
            }
            if org_on && let Some((t, _)) = buf.org_descriptive_line(li, avoid(li)) {
                return Some(t);
            }
            let rep = if has_display {
                buf.display_line(li)
            } else {
                None
            };
            widen(buf, li, rep).map(|(t, _)| t)
        };
        // If point's own line collapses a link before point, point's screen
        // column shifts: map the source column through the shown line.
        // Display replacements (org-modern's glyphs, say) stay on point's
        // line too: point is drawn at the character it reaches.
        let on_point_line = |buf: &Buffer| {
            if has_display {
                buf.display_line(p.line)
            } else {
                None
            }
        };
        // A line that is a picture shows as one (its caption) even with
        // point on it; point is clamped onto the caption.
        let point_stand_in = if org_on && buf.inline_image(p.line).is_some() {
            buf.org_display_line(p.line).map(|t| {
                let n = t.chars().count();
                (t, (0..n).collect::<Vec<usize>>())
            })
        } else {
            None
        };
        let point_rep = if point_stand_in.is_some() {
            point_stand_in
        } else if org_on {
            buf.org_descriptive_line(p.line, avoid(p.line))
                .or_else(|| widen(buf, p.line, on_point_line(buf)))
        } else {
            widen(buf, p.line, on_point_line(buf))
        };
        let point_text = point_rep
            .as_ref()
            .map_or(line_text.as_str(), |(t, _)| t.as_str());
        let point_col = match &point_rep {
            Some((_, src)) => src
                .iter()
                .position(|&c| c == p.col)
                .or_else(|| src.iter().position(|&c| c >= p.col))
                .unwrap_or_else(|| src.len().min(p.col)),
            None => p.col,
        };
        // The screen rows: (line, first column, end column).
        let mut rows_layout: Vec<(usize, usize, usize)> = Vec::with_capacity(row_budget);
        let cursor_x;
        if wrap == Wrap::Off {
            let x = width::display_col(point_text, point_col);
            if x < hscroll {
                hscroll = x.saturating_sub(text_w / 4);
            } else if x >= hscroll + text_w {
                hscroll = x + 1 + text_w / 4 - text_w;
            }
            cursor_x = x - hscroll;
            let mut li = top;
            while rows_layout.len() < row_budget && li < buf.line_count() {
                let len = shown(buf, li).map_or_else(|| buf.line_len(li), |t| t.chars().count());
                rows_layout.push((li, 0, len));
                li = next_line(buf, li);
            }
        } else {
            hscroll = 0;
            let word = wrap == Wrap::Word;
            let starts_of = |li: usize| {
                visual_rows(
                    &shown(buf, li).unwrap_or_else(|| buf.line(li)),
                    text_w,
                    word,
                )
            };
            let point_row =
                |starts: &[usize]| starts.iter().rposition(|&s| s <= point_col).unwrap_or(0);
            // Scroll until point's row fits under `top`.
            loop {
                let rows_before: usize = (top..p.line)
                    .filter(|&l| !buf.line_hidden(l))
                    .map(|l| {
                        let img = match buf.inline_image(l) {
                            Some(path) if pics_on && image_rows_est > 0 && l != p.line => {
                                buf.inline_image_plan(
                                    l,
                                    &path,
                                    text_w,
                                    text_h.saturating_sub(1),
                                    image_rows_est,
                                )
                                .0
                            }
                            _ => 0,
                        };
                        starts_of(l).len() + img
                    })
                    .sum();
                if top >= p.line || rows_before + point_row(&starts_of(p.line)) < text_h {
                    break;
                }
                top = next_line(buf, top).min(p.line);
            }
            let mut li = top;
            while rows_layout.len() < row_budget && li < buf.line_count() {
                let starts = starts_of(li);
                let len = shown(buf, li).map_or_else(|| buf.line_len(li), |t| t.chars().count());
                for (r, &st) in starts.iter().enumerate() {
                    let end = starts.get(r + 1).copied().unwrap_or(len);
                    rows_layout.push((li, st, end));
                }
                li = next_line(buf, li);
            }
            rows_layout.truncate(row_budget);
            let starts = starts_of(p.line);
            let r = point_row(&starts);
            let chunk: String = point_text.chars().skip(starts[r]).collect();
            cursor_x =
                width::display_col(&chunk, point_col - starts[r]).min(text_w.saturating_sub(1));
        }

        // Pictures: blank rows under the link line, for a frontend to draw on.
        // (at, room, path, line)
        let mut image_slots: Vec<(usize, usize, std::path::PathBuf, usize, Option<f64>)> =
            Vec::new();
        // Image mode: the picture fills the window; the raw bytes (the
        // buffer's text) are not shown while it is displayed.
        let image_mode_picture = if buf.major_mode() == crate::buffer::IMAGE_MODE
            || buf.image.as_ref().is_some_and(|s| s.canvas)
        {
            buf.image.as_ref().and_then(|s| match s.display {
                crate::buffer::ImageDisplay::Image => s.rendered.as_ref(),
                _ => None,
            })
        } else {
            None
        };
        if let Some(pic) = image_mode_picture {
            // The picture's own blank rows: the window's whole body. (With
            // no rows left, the placement would be cut to nothing.)
            rows_layout.clear();
            rows_layout.extend(std::iter::repeat_n((0, 0, 0), row_budget.min(text_h)));
            image_slots.push((0, text_h, pic.clone(), 0, None));
        }
        // Tall text (`height` above 1, or a scaled buffer): the rows above
        // a line that has some are left blank, marked by `to == usize::MAX`,
        // for the taller letters to rise into. The line itself keeps one
        // row, so everything that counts rows stays right.
        if style_on && opts.pixel_text && image_mode_picture.is_none() {
            let mut out = Vec::with_capacity(rows_layout.len());
            for &(li, from, to) in &rows_layout {
                if from == 0 {
                    let pct = buf
                        .display_style_runs(li, base_pct)
                        .iter()
                        .map(|r| r.2.height_pct)
                        .max()
                        .unwrap_or(100);
                    let extra = ((usize::from(pct) + 99) / 100)
                        .saturating_sub(1)
                        .min(3)
                        .min(text_h.saturating_sub(2));
                    for _ in 0..extra {
                        out.push((li, 0, usize::MAX));
                    }
                }
                out.push((li, from, to));
            }
            out.truncate(row_budget);
            rows_layout = out;
        }
        let image_rows = crate::org_display::inline_image_rows();
        if pics_on && image_rows > 0 && text_h > 2 {
            let k = image_rows.min(text_h - 1);
            let mut out: Vec<(usize, usize, usize)> = Vec::with_capacity(rows_layout.len());
            let mut i = 0;
            while i < rows_layout.len() {
                let (li, from, to) = rows_layout[i];
                out.push((li, from, to));
                let last_of_line = rows_layout.get(i + 1).is_none_or(|n| n.0 != li);
                if last_of_line && let Some(path) = buf.inline_image(li) {
                    let at = out.len();
                    let (want, width) = buf.inline_image_plan(li, &path, text_w, text_h - 1, k);
                    let room = row_budget.saturating_sub(at).min(want);
                    if room > 0 {
                        for _ in 0..room {
                            out.push((li, to, to));
                        }
                        image_slots.push((at, room, path, li, width));
                    }
                }
                i += 1;
            }
            out.truncate(row_budget);
            rows_layout = out;
        }

        let region = if is_sel { buf.active_region() } else { None };
        let highlight = if is_sel { opts.highlight } else { None };
        // Faces for the visible text: syntax, decorations, then overlays.
        let last_line = rows_layout.last().map_or(top, |r| r.0);
        let vis_start = buf.pos_to_offset(Pos::new(top, 0));
        let vis_end = buf.pos_to_offset(Pos::new(last_line, buf.line_len(last_line)));
        let mut faces: Vec<Face> = buf
            .syntax_faces(vis_start..vis_end)
            .into_iter()
            .map(|f| f.map_or(Face::Default, Face::Syntax))
            .collect();
        for d in buf
            .decorations()
            .iter()
            .filter(|d| d.start < vis_end && d.end > vis_start)
        {
            for i in d.start.max(vis_start)..d.end.min(vis_end) {
                let slot = &mut faces[i - vis_start];
                let syntax = match *slot {
                    Face::Syntax(s) | Face::Decoration(_, Some(s)) => Some(s),
                    _ => None,
                };
                *slot = match d.face {
                    DecoFace::Syntax(s) => Face::Syntax(s),
                    f => Face::Decoration(f, syntax),
                };
            }
        }
        if trailing {
            for li in top..=last_line {
                let text = buf.line(li);
                let len = text.chars().count();
                let from = text.trim_end_matches([' ', '\t']).chars().count();
                let from = if li == p.line && is_sel && p.col >= from {
                    len
                } else {
                    from
                };
                let start = buf.pos_to_offset(Pos::new(li, 0));
                for i in start + from..start + len {
                    if let Some(f) = faces.get_mut(i - vis_start) {
                        *f = Face::TrailingWhitespace;
                    }
                }
            }
        }
        if is_sel && opts.show_paren {
            let mut mark = |at: usize, face: Face| {
                if (vis_start..vis_end).contains(&at) {
                    faces[at - vis_start] = face;
                }
            };
            match buf.matching_bracket(point) {
                Some(Ok((a, b))) => {
                    mark(a, Face::ParenMatch);
                    mark(b, Face::ParenMatch);
                }
                Some(Err(a)) => mark(a, Face::ParenMismatch),
                None => {}
            }
        }

        let style = RowStyle {
            region,
            hl: highlight,
            whitespace,
            truncate: wrap == Wrap::Off,
        };
        let mut rows = Vec::with_capacity(row_budget);
        let mut style_runs: Vec<StyleRun> = Vec::new();
        let mut hl_rows = Vec::new();
        let mut cursor_row = 0;
        // Org indent mode: one forward pass maps visible lines to their
        // virtual indent. A backward scan per line would be quadratic on
        // files with sparse headlines, and this runs every frame. Org num
        // mode walks the same pass to number headlines.
        let (indent_map, num_map) = if indent_on || num_on {
            let mut visible = std::collections::HashSet::new();
            let mut max_li = 0usize;
            for (li, _, _) in &rows_layout {
                visible.insert(*li);
                max_li = max_li.max(*li);
            }
            let mut imap = std::collections::HashMap::new();
            let mut nmap = std::collections::HashMap::new();
            let mut cur: Option<(usize, usize)> = None;
            let mut numbering: Vec<usize> = Vec::new();
            for li in 0..=max_li {
                let level = buf.org_level(li);
                if let Some(n) = level {
                    cur = Some((n, crate::org_display::headline_indent(n, per_level)));
                }
                if indent_on && visible.contains(&li) {
                    let w = match (level, cur) {
                        (Some(_), Some((_, hi))) => hi,
                        (None, Some((hn, hi))) => hi + hn + 1,
                        _ => 0,
                    };
                    imap.insert(li, w);
                }
                if let Some(n) = level {
                    let s = crate::org_display::org_num_step(&mut numbering, n);
                    if num_on && visible.contains(&li) {
                        nmap.insert(li, s);
                    }
                }
            }
            (imap, nmap)
        } else {
            (
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
            )
        };
        for (i, &(li, from, to)) in rows_layout.iter().enumerate() {
            if to == usize::MAX {
                // Room for the taller text below.
                rows.push(vec![Span {
                    text: " ".repeat(gutter),
                    face: Face::Default,
                }]);
                continue;
            }
            let first = from == 0;
            let mut row = if margin_w > 0 {
                let text = if first {
                    buf.margin_text(li, "left-margin")
                } else {
                    None
                };
                vec![margin_cells(text.as_deref().unwrap_or(""), margin_w)]
            } else {
                Vec::new()
            };
            if num_w > 0 {
                if first {
                    row.extend(gutter_cells(li, p.line, buf.line_count(), num_w, numbers));
                } else {
                    row.push(Span {
                        text: " ".repeat(num_w),
                        face: Face::LineNumber,
                    });
                }
            }
            if lf_w > 0 {
                let c = if first {
                    buf.fringe_char(li, "left-fringe")
                } else {
                    None
                };
                row.push(Span {
                    text: c.unwrap_or(' ').to_string(),
                    face: Face::LineNumber,
                });
            }
            let stand_in = if org_on && (li != p.line || buf.inline_image(li).is_some()) {
                buf.org_display_line(li)
            } else {
                None
            };
            // Descriptive links collapse even on point's line (except the
            // link point is inside); a plugin's display replacement only
            // applies to lines point isn't on.
            let replaced = if stand_in.is_some() {
                None
            } else if org_on {
                if let Some(r) = buf.org_descriptive_line(li, avoid(li)) {
                    Some(r)
                } else {
                    let rep = if has_display {
                        buf.display_line(li)
                    } else {
                        None
                    };
                    widen(buf, li, rep)
                }
            } else {
                let rep = if has_display {
                    buf.display_line(li)
                } else {
                    None
                };
                widen(buf, li, rep)
            };
            let mut text = stand_in
                .clone()
                .or_else(|| replaced.as_ref().map(|r| r.0.clone()))
                .unwrap_or_else(|| buf.line(li));
            // Org indent mode: virtual indent, and all but one star
            // masked. Display-only: the text keeps every column.
            let start = buf.pos_to_offset(Pos::new(li, 0)) - vis_start;
            let indent_w = indent_map.get(&li).copied().unwrap_or(0);
            if indent_w > 0 && first {
                row.push(Span {
                    text: " ".repeat(indent_w),
                    face: Face::Default,
                });
            }
            let mut masked: Vec<Face> = Vec::new();
            if indent_on
                && stand_in.is_none()
                && replaced.is_none()
                && let Some(n) = buf.org_level(li)
            {
                let chars: Vec<char> = text.chars().collect();
                if chars.len() >= n && n > 1 {
                    text = chars
                        .iter()
                        .enumerate()
                        .map(|(i, c)| if (1..n).contains(&i) { ' ' } else { *c })
                        .collect();
                    masked = faces
                        [start.min(faces.len())..(start + buf.line_len(li)).min(faces.len())]
                        .to_vec();
                    for f in masked.iter_mut().skip(1).take(n - 1) {
                        *f = Face::Default;
                    }
                }
            }
            let shown_faces: Vec<Face>;
            let mut base: &[Face] = if stand_in.is_some() {
                &[]
            } else if let Some((_, src)) = &replaced {
                // Each shown char takes the face of the char it stands for.
                let line_faces =
                    &faces[start.min(faces.len())..(start + buf.line_len(li)).min(faces.len())];
                shown_faces = src
                    .iter()
                    .map(|&c| line_faces.get(c).copied().unwrap_or(Face::Default))
                    .collect();
                &shown_faces
            } else if !masked.is_empty() {
                &masked
            } else {
                &faces[start.min(faces.len())..(start + text.chars().count()).min(faces.len())]
            };
            // Org num mode: put the section number after the stars, as a
            // display-only insertion (only on unwrapped lines, so the row's
            // end column stays simple).
            let mut to = to;
            let mut num_owned: Vec<Face>;
            if num_on
                && first
                && wrap == Wrap::Off
                && stand_in.is_none()
                && replaced.is_none()
                && let Some(n) = buf.org_level(li)
                && let Some(numstr) = num_map.get(&li)
            {
                let chars: Vec<char> = text.chars().collect();
                let at = (n + 1).min(chars.len());
                let mut nt: String = chars[..at].iter().collect();
                nt.push_str(numstr);
                nt.extend(chars[at..].iter());
                num_owned = base.to_vec();
                let ins = at.min(num_owned.len());
                for _ in 0..numstr.chars().count() {
                    num_owned.insert(ins, Face::Default);
                }
                text = nt;
                base = &num_owned;
                to = text.chars().count();
            }
            row.extend(render_line(
                &text,
                li,
                from..to,
                hscroll,
                text_w,
                &style,
                base,
            ));
            if style_on {
                let off_h = if wrap == Wrap::Off { hscroll } else { 0 };
                let shown_col = |source: usize| match &replaced {
                    Some((_, src)) => src.iter().position(|&c| c >= source).unwrap_or(src.len()),
                    None => source,
                };
                let first_col = width::display_col(&text, from);
                for (a, b, st) in buf.display_style_runs(li, base_pct) {
                    let (lo, hi) = (shown_col(a).max(from), shown_col(b).min(to));
                    if lo >= hi {
                        continue;
                    }
                    let base = (gutter + if first { indent_w } else { 0 }) as isize
                        - first_col as isize
                        - off_h as isize;
                    let c0 = (base + width::display_col(&text, lo) as isize).max(gutter as isize);
                    let c1 = (base + width::display_col(&text, hi) as isize)
                        .min((gutter + text_w) as isize);
                    if c1 > c0 {
                        style_runs.push(StyleRun {
                            row: rows.len() as u16,
                            col: c0 as u16,
                            cells: (c1 - c0) as u16,
                            height_pct: st.height_pct,
                            raise_pct: st.raise_pct,
                        });
                    }
                }
            }
            if to == text.chars().count() && buf.line_folded(li) {
                let used: usize = row.iter().map(|s| width::line_width(&s.text)).sum();
                let room = (gutter + text_w).saturating_sub(used).min(3);
                if room > 0 {
                    row.push(Span {
                        text: ".".repeat(room),
                        face: Face::Fold,
                    });
                }
            }
            if right_w > 0 {
                let used: usize = row.iter().map(|sp| width::line_width(&sp.text)).sum();
                row.push(Span {
                    text: " ".repeat((gutter + text_w).saturating_sub(used)),
                    face: Face::Default,
                });
                if rf_w > 0 {
                    let c = if first {
                        buf.fringe_char(li, "right-fringe")
                    } else {
                        None
                    };
                    row.push(Span {
                        text: c.unwrap_or(' ').to_string(),
                        face: Face::LineNumber,
                    });
                }
                if rm_w > 0 {
                    let text = if first {
                        buf.margin_text(li, "right-margin")
                    } else {
                        None
                    };
                    row.push(margin_cells(text.as_deref().unwrap_or(""), rm_w));
                }
            }
            if li == p.line {
                if hl_line {
                    hl_rows.push(i as u16);
                }
                if p.col >= from && (p.col < to || to == buf.line_len(li)) {
                    cursor_row = i;
                }
            }
            rows.push(row);
        }
        // Rows past the end of the buffer.
        while rows.len() < row_budget {
            let li = buf.line_count() + rows.len();
            let mut blank = if margin_w > 0 {
                vec![margin_cells("", margin_w)]
            } else {
                Vec::new()
            };
            if num_w > 0 {
                blank.extend(gutter_cells(li, p.line, buf.line_count(), num_w, numbers));
            }
            rows.push(blank);
        }
        // In wrapped lines the cursor is on the last row that starts at or
        // before it (the loop above picked the first).
        if wrap != Wrap::Off {
            cursor_row = rows_layout
                .iter()
                .rposition(|&(li, from, _)| li == p.line && from <= p.col)
                .unwrap_or(0);
        }
        if let Some((_, htext)) = &header_info {
            let t = width::truncate(htext, full_w);
            let used = width::line_width(&t);
            rows.insert(
                0,
                vec![Span {
                    text: format!("{t}{}", " ".repeat(full_w.saturating_sub(used))),
                    face: Face::ModeLine,
                }],
            );
            // Anything placed by text row moves down one.
            for h in hl_rows.iter_mut() {
                *h += 1;
            }
            cursor_row += 1;
        }
        let fill_column = (fci && fci_column >= hscroll && fci_column - hscroll < text_w)
            .then(|| (gutter + fci_column - hscroll) as u16);
        let fill_column_character = fill_column.map(|_| match o.fill_column_indicator_character {
            Some(FillColumnChar::Char(c)) => c.to_string(),
            _ => "│".to_string(),
        });

        let mut extra: Vec<String> = Vec::new();
        if wrap == Wrap::Word {
            extra.push("Wrap".into());
        }
        if whitespace {
            extra.push("ws".into());
        }
        if is_sel {
            extra.extend(opts.mode_line_extra.iter().cloned());
        }
        let column = width::display_col(point_text, point_col);
        let (mode_line, mode_line_hits) = mode_line(
            buf,
            real_rect.width as usize,
            top,
            text_h,
            p,
            column,
            &extra,
            is_active,
            &frame_name,
            &editor.modes,
            (
                opts.mode_line_no_line,
                opts.mode_line_no_column,
                opts.mode_line_size,
            ),
        );
        // Org indent mode shifts point's line right by its virtual indent;
        // Org num mode shifts it by the inserted number when point is past
        // the stars.
        let num_shift = if num_on && wrap == Wrap::Off {
            match (buf.org_level(p.line), num_map.get(&p.line)) {
                (Some(n), Some(s)) if p.col > (n + 1).min(buf.line_len(p.line)) => {
                    s.chars().count()
                }
                _ => 0,
            }
        } else {
            0
        };
        let cursor_x = cursor_x + indent_map.get(&p.line).copied().unwrap_or(0) + num_shift;
        let cursor = (
            rect.x + gutter as u16 + cursor_x.min(text_w.saturating_sub(1)) as u16,
            rect.y + cursor_row as u16,
        );
        let popup = match &opts.popup {
            Some(spec) if is_active && !spec.rows.is_empty() => {
                let a = buf.offset_to_pos(spec.anchor.min(buf.char_count()));
                let row = rows_layout
                    .iter()
                    .rposition(|&(li, from, _)| li == a.line && from <= a.col);
                row.map(|r| {
                    let (li, from, _) = rows_layout[r];
                    let chunk: String = buf.line(li).chars().skip(from).collect();
                    let x = width::display_col(&chunk, a.col - from)
                        .saturating_sub(if wrap == Wrap::Off { hscroll } else { 0 })
                        + indent_map.get(&a.line).copied().unwrap_or(0);
                    popup_view(
                        spec,
                        rect,
                        gutter,
                        full_w,
                        text_h + header_rows,
                        r + header_rows,
                        x,
                    )
                })
            }
            _ => None,
        };
        // The auxiliary popup (corfu-popupinfo) sits beside the popup, at
        // its row, every row it shows (`rect`'s text area is the limit).
        let popup_info = match (&opts.popup_info, &popup) {
            (Some(spec), Some(main)) if is_active && !spec.lines.is_empty() => Some(info_view(
                spec,
                main,
                rect,
                gutter,
                text_w,
                text_h + header_rows,
            )),
            _ => None,
        };
        let cursor_shape = o.cursor.unwrap_or_default();
        // The vertical scroll bar's thumb: where it starts and how tall it
        // is, within the text rows (the header row included).
        let scroll_bar = (bar_w > 0).then(|| {
            let lines = buf.line_count().max(1);
            let rows = text_h + header_rows;
            let thumb = ((rows as f64) * (rows as f64) / (lines as f64)).round() as usize;
            let thumb = thumb.clamp(1, rows);
            let max_start = rows - thumb;
            let start = if lines <= rows {
                0
            } else {
                ((top as f64) * (rows as f64) / (lines as f64)).round() as usize
            }
            .min(max_start);
            (start as u16, thumb as u16)
        });
        // The horizontal scroll bar's thumb: (start column, columns) within
        // the text area (after the gutter). Point's line sets the range; a
        // line that fits fills the whole track.
        let horizontal_scroll_bar = (hbar_h > 0).then(|| {
            let line_w = crate::width::line_width(&line_text);
            if line_w <= text_w {
                (0, text_w as u16)
            } else {
                let thumb = ((text_w * text_w) / line_w).clamp(1, text_w);
                let room = text_w - thumb;
                let max_h = line_w - text_w;
                let start = (hscroll.min(max_h) * room) / max_h;
                (start as u16, thumb as u16)
            }
        });

        // Point can make redisplay choose a different top; the fractional
        // offset belongs to the line it was measured from, so drop it then.
        let scroll_y_px = if top == old_top { scroll_y_px } else { 0 };
        let w = editor.windows.get_mut(id).expect("laid out");
        w.scroll_top = top;
        w.hscroll = hscroll;
        w.scroll_y_px = scroll_y_px;
        w.height = text_h;
        w.header_rows = header_rows;
        w.popup_area = popup
            .as_ref()
            .map(|p| (p.x, p.y, p.width, p.rows.len() as u16));
        w.scroll_bar = scroll_bar;
        w.horizontal_scroll_bar = horizontal_scroll_bar;
        w.width = text_w;
        w.gutter = gutter;
        w.mode_line_hits = mode_line_hits.clone();
        w.rows = rows_layout
            .iter()
            .map(|&(li, from, _)| (li, from))
            .collect();
        views.push(WindowView {
            id,
            rect: real_rect,
            cell_scale_pct: cell_pct,
            rows,
            text_width: (gutter + text_w + bar_w) as u16,
            border,
            mode_line,
            mode_line_hits,
            selected: is_sel,
            cursor,
            gutter: gutter as u16,
            header_rows: header_rows as u16,
            hl_rows,
            fill_column,
            fill_column_character,
            popup,
            popup_info,
            cursor_shape,
            scroll_bar,
            scroll_y_px,
            horizontal_scroll_bar,
            style_runs: style_runs
                .into_iter()
                .map(|mut r| {
                    r.row += header_rows as u16;
                    r
                })
                .collect(),
            images: image_slots
                .iter()
                .map(|(at, n, path, line, width)| ImageView {
                    row: (*at + header_rows) as u16,
                    rows: (*n).min(rows_layout.len().saturating_sub(*at)) as u16,
                    path: path.to_string_lossy().into_owned(),
                    descriptor: {
                        let mut d = buf.display_image_descriptor(*line);
                        if buf.plugin_image_centered(*line) {
                            d.get_or_insert_with(Default::default).center = true;
                        }
                        // The size an Org image link asked for (or its
                        // natural size, capped), unless the picture's own
                        // display properties say how big to be.
                        if let Some(w) = width {
                            let d = d.get_or_insert_with(Default::default);
                            if d.width.is_none() && d.height.is_none() && d.scale.is_none() {
                                d.width = Some(*w);
                            }
                        }
                        d
                    },
                })
                .filter(|im| im.rows > 0)
                .collect(),
        });
    }
    views
}

/// Lay out a popup under screen row `row` of a window (above it if it
/// doesn't fit below), starting at text column `x`, moved left if it
/// would stick out.
fn popup_view(
    spec: &PopupSpec,
    rect: Rect,
    gutter: usize,
    full_w: usize,
    text_h: usize,
    row: usize,
    x: usize,
) -> PopupView {
    let text_w = full_w.saturating_sub(gutter).max(1);
    let widest_margin = spec
        .rows
        .iter()
        .map(|r| width::line_width(&r.margin))
        .max()
        .unwrap_or(0);
    let widest_text = spec
        .rows
        .iter()
        .map(|r| width::line_width(&r.text))
        .max()
        .unwrap_or(0);
    let widest_note = spec
        .rows
        .iter()
        .map(|r| width::line_width(&r.annotation))
        .max()
        .unwrap_or(0);
    let mut w = 1
        + widest_margin
        + widest_text
        + 1
        + if widest_note > 0 {
            1 + widest_note + 1
        } else {
            0
        };
    // The plugin's preferred width (corfu's min/max), never past the
    // window. 0 means "as wide as the rows need".
    if spec.width > 0 {
        w = spec.width as usize;
    }
    w = w.min(text_w).max(4);
    // The candidates line up with the text (the row starts with a blank).
    let x = x.saturating_sub(1).min(text_w - w);
    let n = spec.rows.len();
    let below = row + 1 + n <= text_h;
    let top = if below {
        row + 1
    } else {
        row.saturating_sub(n)
    };
    let rows = spec
        .rows
        .iter()
        .take(text_h)
        .map(|r| {
            let face = if r.selected {
                Face::PopupSelected
            } else if r.deprecated {
                Face::PopupDeprecated
            } else {
                Face::Popup
            };
            let note_face = if r.selected {
                Face::PopupSelected
            } else if r.deprecated {
                Face::PopupDeprecated
            } else {
                Face::PopupAnnotation
            };
            // The leading blank, the (space-padded) margin if any, then the
            // text; a trailing blank before the annotation.
            let prefix_w = 1 + if widest_margin > 0 {
                widest_margin + 1
            } else {
                0
            };
            let pad = (prefix_w + widest_text + 1).min(w);
            let margin = format!(
                "{}{}",
                r.margin,
                " ".repeat(widest_margin.saturating_sub(width::line_width(&r.margin)))
            );
            let mut plain = String::from(" ");
            if widest_margin > 0 {
                plain.push_str(&margin);
                plain.push(' ');
            }
            plain.push_str(&r.text);
            let tw = width::line_width(&plain);
            let mut spans = Vec::new();
            if tw > pad {
                spans.push(Span {
                    text: width::truncate(&plain, pad),
                    face,
                });
            } else {
                spans.push(Span {
                    text: " ".into(),
                    face,
                });
                if widest_margin > 0 {
                    spans.push(Span {
                        text: r.margin.clone(),
                        face: if r.selected {
                            Face::PopupSelected
                        } else {
                            Face::PopupMargin
                        },
                    });
                    if widest_margin > width::line_width(&r.margin) {
                        spans.push(Span {
                            text: " ".repeat(widest_margin - width::line_width(&r.margin)),
                            face,
                        });
                    }
                    spans.push(Span {
                        text: " ".into(),
                        face,
                    });
                }
                // Highlight the part of the text the typed input matches.
                let mw = matched_width(&r.text, &spec.input);
                if mw > 0 {
                    let (hit, rest) = split_width(&r.text, mw);
                    spans.push(Span {
                        text: hit,
                        face: Face::PopupMatch,
                    });
                    if !rest.is_empty() {
                        spans.push(Span { text: rest, face });
                    }
                } else {
                    spans.push(Span {
                        text: r.text.clone(),
                        face,
                    });
                }
                if tw < pad {
                    spans.push(Span {
                        text: " ".repeat(pad - tw),
                        face,
                    });
                }
            }
            if w > pad {
                let mut n = format!("{} ", r.annotation);
                let room = w - pad;
                let nw = width::line_width(&n);
                if nw > room {
                    n = width::truncate(&n, room);
                } else {
                    n.push_str(&" ".repeat(room - nw));
                }
                spans.push(Span {
                    text: n,
                    face: note_face,
                });
            }
            spans
        })
        .collect();
    PopupView {
        x: rect.x + (gutter + x) as u16,
        y: rect.y + top as u16,
        width: w as u16,
        rows,
    }
}

/// Place the auxiliary popup (corfu-popupinfo) beside the completion
/// popup: to its right when there is room, else its left, else clamped to
/// the window's text area. Its top lines up with the popup's.
fn info_view(
    spec: &InfoSpec,
    main: &PopupView,
    rect: Rect,
    gutter: usize,
    text_w: usize,
    text_h: usize,
) -> PopupView {
    let widest = spec
        .lines
        .iter()
        .map(|l| width::line_width(l))
        .max()
        .unwrap_or(0);
    let mut w = if spec.width > 0 {
        spec.width as usize
    } else {
        widest + 2
    };
    w = w.min(text_w).max(4);
    let left = rect.x as usize + gutter;
    let right = left + text_w;
    let main_left = main.x as usize;
    let main_right = main_left + main.width as usize;
    let x = if right.saturating_sub(main_right + 1) >= w {
        main_right + 1
    } else if main_left > left + w {
        main_left - w - 1
    } else {
        right.saturating_sub(w).max(left)
    };
    let n = spec.lines.len().min(text_h);
    // Its top lines up with the popup's, pulled up if it would spill past
    // the text area.
    let y = if main.y as usize + n <= rect.y as usize + text_h {
        main.y as usize
    } else {
        (rect.y as usize + text_h).saturating_sub(n)
    };
    let rows = spec
        .lines
        .iter()
        .take(n)
        .map(|line| {
            let text = width::truncate(line, w);
            let pad = w.saturating_sub(width::line_width(&text));
            vec![Span {
                text: format!("{text}{}", " ".repeat(pad)),
                face: Face::PopupInfo,
            }]
        })
        .collect();
    PopupView {
        x: x as u16,
        y: y as u16,
        width: w as u16,
        rows,
    }
}

/// How many display columns of `text` match the popup `input` (a
/// case-insensitive prefix, which is what the popup filters on). 0 when
/// nothing does.
fn matched_width(text: &str, input: &str) -> usize {
    if input.is_empty() || text.is_empty() {
        return 0;
    }
    if text.to_lowercase().starts_with(&input.to_lowercase()) {
        width::line_width(input).min(width::line_width(text))
    } else {
        0
    }
}

/// Split `s` after `cells` display columns (a whole grapheme). The head
/// is a byte prefix of `s`, so the rest slices cleanly.
fn split_width(s: &str, cells: usize) -> (String, String) {
    let head = width::truncate(s, cells);
    let rest = s[head.len()..].to_string();
    (head, rest)
}

/// Where a line breaks into screen rows `width` cells wide: the first
/// column of each row (always starting with 0). `word`: break after a
/// space when there is one in the row (visual-line-mode), else anywhere.
pub fn visual_rows(line: &str, width: usize, word: bool) -> Vec<usize> {
    let width = width.max(1);
    let mut starts = vec![0];
    let mut x = 0;
    // Column just after the last blank in this row, if any.
    let mut after_blank: Option<usize> = None;
    let graphemes: Vec<(usize, &str)> = width::graphemes(line).collect();
    let mut i = 0;
    while i < graphemes.len() {
        let (col, g) = graphemes[i];
        let w = grapheme_width(g, x).max(
            if g.chars().next().is_some_and(char::is_control) && g != "\t" {
                2
            } else {
                0
            },
        );
        if x + w > width && x > 0 {
            let at = match after_blank {
                // A blank that doesn't fit hangs off the end of the row.
                _ if word && (g == " " || g == "\t") => col + 1,
                Some(b) if word && b > *starts.last().expect("never empty") => b,
                _ => col,
            };
            starts.push(at);
            // Measure again from the new row's start.
            x = 0;
            after_blank = None;
            i = graphemes.iter().position(|&(c, _)| c == at).unwrap_or(i);
            continue;
        }
        x += w;
        if g == " " || g == "\t" {
            after_blank = Some(col + 1);
        }
        i += 1;
    }
    starts
}

/// Move `n` screen rows down (up if negative) from `offset` in a buffer
/// shown `width` cells wide, keeping the display column (visual-line-mode's
/// C-n / C-p). Returns the new offset.
pub fn move_visual_rows(buf: &Buffer, offset: usize, n: isize, width: usize, word: bool) -> usize {
    let p = buf.offset_to_pos(offset);
    let starts = visual_rows(&buf.line(p.line), width, word);
    let mut row = starts.iter().rposition(|&s| s <= p.col).unwrap_or(0);
    let chunk: String = buf.line(p.line).chars().skip(starts[row]).collect();
    let x = width::display_col(&chunk, p.col - starts[row]);
    let (mut line, mut starts) = (p.line, starts);
    let mut left = n;
    while left != 0 {
        if left > 0 {
            if row + 1 < starts.len() {
                row += 1;
            } else if let Some(next) = buf.next_visible_line(line) {
                line = next;
                starts = visual_rows(&buf.line(line), width, word);
                row = 0;
            } else {
                break;
            }
            left -= 1;
        } else {
            if row > 0 {
                row -= 1;
            } else if let Some(prev) = buf.prev_visible_line(line) {
                line = prev;
                starts = visual_rows(&buf.line(line), width, word);
                row = starts.len() - 1;
            } else {
                break;
            }
            left += 1;
        }
    }
    let text = buf.line(line);
    let from = starts[row];
    let to = starts.get(row + 1).copied().unwrap_or(text.chars().count());
    let chunk: String = text.chars().skip(from).take(to - from).collect();
    let mut col = from + width::col_at_display(&chunk, x);
    // Not past the end of a wrapped row (that's the next row's start).
    if row + 1 < starts.len() && col >= to {
        col = to.saturating_sub(1).max(from);
    }
    buf.pos_to_offset(Pos::new(line, col))
}

/// Scroll so `line` is visible: a one-line move scrolls by a line, a
/// jump recenters.
pub fn scroll_to_show(top: &mut usize, line: usize, height: usize) {
    let h = height.max(1);
    if line < *top {
        *top = if *top - line == 1 {
            line
        } else {
            line.saturating_sub(h / 2)
        };
    } else if line >= *top + h {
        *top = if line == *top + h {
            *top + 1
        } else {
            line.saturating_sub(h / 2)
        };
    }
}

/// The line number cells at the start of row `li`.
/// The left margin of one row: `text` cut or padded to `width` cells.
fn margin_cells(text: &str, width: usize) -> Span {
    let shown = width::truncate(text, width.saturating_sub(1));
    let pad = width.saturating_sub(width::line_width(&shown));
    Span {
        text: format!("{shown}{}", " ".repeat(pad)),
        face: Face::LineNumber,
    }
}

fn gutter_cells(
    li: usize,
    point_line: usize,
    lines: usize,
    width: usize,
    numbers: LineNumbers,
) -> Vec<Span> {
    if li >= lines {
        return vec![Span {
            text: " ".repeat(width),
            face: Face::LineNumber,
        }];
    }
    let n = match numbers {
        LineNumbers::Relative if li != point_line => li.abs_diff(point_line),
        _ => li + 1,
    };
    let face = if li == point_line {
        Face::LineNumberCurrent
    } else {
        Face::LineNumber
    };
    let digits = width.saturating_sub(1);
    let text = format!("{:>digits$} ", n);
    // Too narrow a window: keep the right end of the number.
    let text: String = text
        .chars()
        .skip(text.chars().count().saturating_sub(width))
        .collect();
    vec![Span { text, face }]
}

fn in_range(range: Option<(Pos, Pos)>, p: Pos) -> bool {
    range.is_some_and(|(a, b)| a <= p && p < b)
}

/// How a row is drawn beyond its text and faces.
struct RowStyle {
    region: Option<(Pos, Pos)>,
    hl: Option<(Pos, Pos)>,
    /// Show spaces and tabs (whitespace-mode).
    whitespace: bool,
    /// Long lines are cut off with `$` (not wrapped).
    truncate: bool,
}

/// One screen row: columns `cols` of the line, from display column
/// `hscroll`, `width` wide. When truncating, `$` marks text cut off on
/// either side. `base` has a face per column of the whole line.
fn render_line(
    text: &str,
    line: usize,
    cols: std::ops::Range<usize>,
    hscroll: usize,
    width: usize,
    style: &RowStyle,
    base: &[Face],
) -> Vec<Span> {
    // One entry per screen cell; "" continues the wide cell before it.
    let mut cells: Vec<(String, Face)> = Vec::with_capacity(width);
    let mut x = 0;
    let mut cut_right = false;
    for (col, g) in width::graphemes(text) {
        if col < cols.start {
            continue;
        }
        if col >= cols.end {
            break;
        }
        let p = Pos::new(line, col);
        let face = if in_range(style.hl, p) {
            Face::Match
        } else if in_range(style.region, p) {
            Face::Region
        } else {
            base.get(col).copied().unwrap_or(Face::Default)
        };
        let plain = matches!(face, Face::Default | Face::Syntax(_));
        let w = grapheme_width(g, x);
        let shown: Vec<(String, Face)> = match g {
            "\t" if style.whitespace => {
                let f = if plain { Face::Whitespace } else { face };
                let mut v = vec![("»".to_string(), f)];
                v.extend(std::iter::repeat_n(
                    (" ".to_string(), face),
                    w.saturating_sub(1),
                ));
                v
            }
            "\t" => vec![(" ".to_string(), face); w],
            " " if style.whitespace => {
                vec![("·".to_string(), if plain { Face::Whitespace } else { face })]
            }
            _ if g.chars().next().is_some_and(char::is_control) => {
                let c = g.chars().next().unwrap_or('?');
                let caret = char::from_u32((c as u32 + 64) & 0x7f).unwrap_or('?');
                let face = if face == Face::Default {
                    Face::Escape
                } else {
                    face
                };
                vec![("^".to_string(), face), (caret.to_string(), face)]
            }
            _ if w == 0 => {
                // Zero-width (a stray combining mark): attach to the previous cell.
                if x > hscroll
                    && let Some(last) = cells.last_mut()
                {
                    last.0.push_str(g);
                }
                continue;
            }
            _ => {
                let mut v = vec![(g.to_string(), face)];
                v.extend(std::iter::repeat_n((String::new(), face), w - 1));
                v
            }
        };
        for (i, cell) in shown.into_iter().enumerate() {
            let cx = x + i;
            if cx < hscroll {
                continue;
            }
            if cx >= hscroll + width {
                cut_right = true;
                break;
            }
            // A wide character cut by the left edge shows as spaces.
            let cell = if cell.0.is_empty() && cells.is_empty() {
                (" ".to_string(), cell.1)
            } else {
                cell
            };
            cells.push(cell);
        }
        x += w;
        if cut_right {
            break;
        }
    }
    if style.truncate {
        // A wide character that doesn't fit at the right edge.
        if let Some(last) = cells.last()
            && cells.len() == width
            && !last.0.is_empty()
            && grapheme_width(&last.0, 0) > 1
        {
            cells.pop();
            cells.push((" ".to_string(), Face::Default));
            cut_right = true;
        }
        if cut_right && !cells.is_empty() {
            let n = cells.len();
            if cells[n - 1].0.is_empty() {
                cells[n - 2] = (" ".to_string(), Face::Default);
            }
            cells[n - 1] = ("$".to_string(), Face::Truncation);
        }
        if hscroll > 0 && !cells.is_empty() {
            if cells.len() > 1 && cells[1].0.is_empty() {
                cells[1] = (" ".to_string(), Face::Default);
            }
            cells[0] = ("$".to_string(), Face::Truncation);
        }
    }

    let mut spans: Vec<Span> = Vec::new();
    for (text, face) in cells {
        match spans.last_mut() {
            Some(s) if s.face == face => s.text.push_str(&text),
            _ => spans.push(Span { text, face }),
        }
    }
    spans
}

/// "fundamental-mode" -> "Fundamental", "Buffer-menu-mode" -> "Buffer Menu".
pub fn mode_display_name(mode: &str) -> String {
    let base = mode.strip_suffix("-mode").unwrap_or(mode);
    base.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[allow(clippy::too_many_arguments)]
fn mode_line(
    buf: &Buffer,
    width: usize,
    top: usize,
    height: usize,
    point: Pos,
    column: usize,
    extra: &[String],
    selected: bool,
    frame: &str,
    registry: &crate::modes::ModeRegistry,
    (no_line, no_column, size): (bool, bool, bool),
) -> (Vec<Span>, Vec<ModeLineHit>) {
    let total = buf.line_count();
    let position = if total <= height {
        "All".to_string()
    } else if top == 0 {
        "Top".to_string()
    } else if top + height >= total {
        "Bot".to_string()
    } else {
        format!("{}%", top * 100 / (total - height))
    };
    let flag = match (buf.is_modified(), buf.changed_on_disk(), buf.read_only()) {
        (_, true, _) => "!!",
        (true, _, true) => "%*",
        (false, _, true) => "%%",
        (true, _, false) => "**",
        _ => "--",
    };
    let mut text = String::new();
    let mut hits: Vec<ModeLineHit> = Vec::new();
    // Append `s`, making it clickable as `target` when there is one.
    let mut put = |text: &mut String, s: &str, target: Option<ModeLineTarget>| {
        let from = width::line_width(text) as u16;
        text.push_str(s);
        if let Some(target) = target {
            let to = width::line_width(text) as u16;
            if to > from {
                hits.push(ModeLineHit { from, to, target });
            }
        }
    };
    let im = buf.input_method_title.as_deref().unwrap_or("");
    put(&mut text, "-", None);
    put(&mut text, &format!("{im}U:"), Some(ModeLineTarget::Mule));
    put(&mut text, flag, Some(ModeLineTarget::Modified));
    put(&mut text, "-", None);
    put(&mut text, frame, Some(ModeLineTarget::Frame));
    put(&mut text, "  ", None);
    put(
        &mut text,
        &format!("{:<20}", buf.name()),
        Some(ModeLineTarget::BufferName),
    );
    put(&mut text, " ", None);
    put(
        &mut text,
        &format!("{position:<4}"),
        Some(ModeLineTarget::Percent),
    );
    if size {
        put(
            &mut text,
            &format!(" {}", size_indication(buf.char_count())),
            Some(ModeLineTarget::Size),
        );
    }
    if !no_line {
        put(
            &mut text,
            &format!(" L{}", point.line + 1),
            Some(ModeLineTarget::Line),
        );
    }
    if !no_column {
        put(
            &mut text,
            &format!("  C{column}"),
            Some(ModeLineTarget::Column),
        );
    }
    put(&mut text, "   ", None);
    // Version control's part, as vc-mode: " Git-main".
    if let Some(v) = buf.vc_mode.as_deref() {
        put(&mut text, v, Some(ModeLineTarget::Vc));
        put(&mut text, "  ", None);
    }
    for info in buf.mode_line_info.values().filter(|i| !i.is_empty()) {
        put(&mut text, info, Some(ModeLineTarget::Info));
        put(&mut text, "  ", None);
    }
    put(&mut text, "(", None);
    put(
        &mut text,
        &mode_display_name(buf.major_mode()),
        Some(ModeLineTarget::MajorMode),
    );
    for m in buf.minor_modes() {
        if let Some(l) = registry.lighter(m) {
            put(&mut text, " ", None);
            put(
                &mut text,
                &l,
                Some(ModeLineTarget::MinorMode(m.to_string())),
            );
        }
    }
    for m in extra {
        put(&mut text, " ", None);
        put(
            &mut text,
            &mode_display_name(m),
            Some(ModeLineTarget::Extra(m.clone())),
        );
    }
    put(&mut text, ")  ", None);
    let used = width::line_width(&text);
    let text = if used < width {
        format!("{text}{}", "-".repeat(width - used))
    } else {
        text
    };
    let face = if selected {
        Face::ModeLine
    } else {
        Face::ModeLineInactive
    };
    (vec![Span { text, face }], hits)
}

/// size indication: characters as `123`, `4.5k` or `1.2M`.
pub fn size_indication(chars: usize) -> String {
    if chars < 1000 {
        format!("{chars}")
    } else if chars < 1_000_000 {
        format!("{:.1}k", chars as f64 / 1000.0)
    } else {
        format!("{:.1}M", chars as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::Direction;

    /// `inline_image_rows` is a global; tests that set it take this lock.
    static IMAGE_ROWS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const TRUNC: RowStyle = RowStyle {
        region: None,
        hl: None,
        whitespace: false,
        truncate: true,
    };

    fn row_text(row: &[Span]) -> String {
        row.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn tabs_controls_and_wide_chars() {
        let row = render_line("a\tb\u{1}漢", 0, 0..usize::MAX, 0, 20, &TRUNC, &[]);
        assert_eq!(row_text(&row), "a       b^A漢");
        assert_eq!(
            row[1],
            Span {
                text: "^A".into(),
                face: Face::Escape
            }
        );
    }

    #[test]
    fn truncation_markers() {
        let row = render_line("abcdefghij", 0, 0..usize::MAX, 0, 5, &TRUNC, &[]);
        assert_eq!(row_text(&row), "abcd$");
        let row = render_line("abcdefghij", 0, 0..usize::MAX, 3, 5, &TRUNC, &[]);
        assert_eq!(row_text(&row), "$efg$");
        // A wide char that would straddle the right edge becomes a space.
        let row = render_line("abcd漢x", 0, 0..usize::MAX, 0, 5, &TRUNC, &[]);
        assert_eq!(row_text(&row), "abcd$");
        let row = render_line("abc漢x", 0, 0..usize::MAX, 0, 5, &TRUNC, &[]);
        assert_eq!(row_text(&row), "abc $");
    }

    #[test]
    fn region_and_match_faces() {
        let region = Some((Pos::new(0, 1), Pos::new(0, 4)));
        let hl = Some((Pos::new(0, 2), Pos::new(0, 3)));
        let row = render_line(
            "abcdef",
            0,
            0..6,
            0,
            10,
            &RowStyle {
                region,
                hl,
                whitespace: false,
                truncate: true,
            },
            &[],
        );
        let faces: Vec<(&str, Face)> = row.iter().map(|s| (s.text.as_str(), s.face)).collect();
        assert_eq!(
            faces,
            vec![
                ("a", Face::Default),
                ("b", Face::Region),
                ("c", Face::Match),
                ("d", Face::Region),
                ("ef", Face::Default)
            ]
        );
    }

    #[test]
    fn two_windows_on_one_buffer_keep_their_own_points() {
        let mut ed = Editor::new();
        ed.create_buffer("*scratch*");
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        ed.state.active_buffer_mut().unwrap().set_contents(&text);
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        };
        let other = ed.split_window(Direction::Rows).unwrap();
        ed.state.active_buffer_mut().unwrap().goto_line(80);
        let views = redisplay(&mut ed, area, &DisplayOptions::default());
        assert_eq!(views.len(), 2);
        assert!(
            row_text(&views[0].rows[0]).starts_with("line 7"),
            "top window scrolled to line 80"
        );
        assert_eq!(row_text(&views[1].rows[0]), "line 1");
        assert!(row_text(&views[0].mode_line).contains("L80"));
        assert!(row_text(&views[1].mode_line).contains("L1 "));

        // Typing at the top of the buffer in the lower window moves the
        // upper window's point along with its line.
        ed.select_window(other);
        ed.state.active_buffer_mut().unwrap().insert_str("new\n");
        let top = ed.windows.ids()[0];
        let p = ed
            .state
            .active_buffer()
            .unwrap()
            .offset_to_pos(ed.window_point(top));
        assert_eq!(p, Pos::new(80, 0));
        ed.select_window(top);
        assert_eq!(ed.state.active_buffer().unwrap().point(), Pos::new(80, 0));
    }

    #[test]
    fn line_numbers_syntax_and_decorations() {
        let mut ed = Editor::new();
        let id = ed.create_buffer("x.rs");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_contents("fn a() {}\nlet x = 1;\n");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_major_mode("rust-mode");
        ed.show_buffer(id);
        ed.state.active_buffer_mut().unwrap().goto_line(2);
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 6,
        };
        let opts = DisplayOptions {
            line_numbers: LineNumbers::Absolute,
            ..Default::default()
        };
        let v = &redisplay(&mut ed, area, &opts)[0];
        assert_eq!(v.gutter, 3);
        assert_eq!(
            v.rows[0][0],
            Span {
                text: " 1 ".into(),
                face: Face::LineNumber
            }
        );
        assert_eq!(
            v.rows[1][0],
            Span {
                text: " 2 ".into(),
                face: Face::LineNumberCurrent
            }
        );
        assert_eq!(
            v.rows[0][1],
            Span {
                text: "fn".into(),
                face: Face::Syntax(SyntaxFace::Keyword)
            }
        );
        assert_eq!(v.cursor, (3, 1), "the cursor is right of the gutter");
        assert_eq!(v.text_width, 30);

        // The buffer's own option wins; relative numbers count from point.
        let b = ed.state.active_buffer_mut().unwrap();
        b.set_option("line-numbers", "relative").unwrap();
        b.set_decorations("lint", vec![(14, 15, DecoFace::Warning, None)]);
        let v = &redisplay(&mut ed, area, &opts)[0];
        assert_eq!(row_text(&v.rows[0]), " 1 fn a() {}");
        assert_eq!(row_text(&v.rows[1]), " 2 let x = 1;");
        let x = v.rows[1].iter().find(|s| s.text == "x").unwrap();
        assert_eq!(x.face, Face::Decoration(DecoFace::Warning, None));
        ed.state.active_buffer_mut().unwrap().goto_line(1);
        let v = &redisplay(&mut ed, area, &opts)[0];
        assert_eq!(v.rows[1][0].text, " 1 ", "distance from point");

        // Special buffers never get the default.
        ed.state
            .active_buffer_mut()
            .unwrap()
            .set_option("line-numbers", "")
            .unwrap();
        ed.state
            .active_buffer_mut()
            .unwrap()
            .set_major_mode("help-mode");
        let v = &redisplay(&mut ed, area, &opts)[0];
        assert_eq!(v.gutter, 0);
        assert!(row_text(&v.rows[0]).starts_with("fn a()"));
        assert!(
            v.rows[0].iter().all(|s| s.face == Face::Default),
            "and no grammar"
        );
    }

    fn one_window(
        text: &str,
        width: u16,
        height: u16,
        setup: impl FnOnce(&mut Buffer),
        opts: &DisplayOptions,
    ) -> WindowView {
        let mut ed = Editor::new();
        ed.create_buffer("t");
        let b = ed.state.active_buffer_mut().unwrap();
        b.set_contents(text);
        setup(b);
        redisplay(
            &mut ed,
            Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            opts,
        )
        .remove(0)
    }

    #[test]
    fn a_pixel_offset_keeps_its_anchor_and_asks_for_an_extra_row() {
        let mut ed = Editor::new();
        ed.create_buffer("t");
        let text: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        ed.state.active_buffer_mut().unwrap().set_contents(&text);
        ed.state.active_buffer_mut().unwrap().goto_line(4);
        {
            let w = ed.windows.selected_mut();
            w.scroll_top = 3;
            w.scroll_y_px = 4;
        }
        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 6,
        };
        let v = &redisplay(&mut ed, area, &DisplayOptions::default())[0];
        assert_eq!(v.scroll_y_px, 4, "the offset survives redisplay");
        assert_eq!(row_text(&v.rows[0]), "line 4", "anchored on its line");
        assert_eq!(v.rows.len(), 6, "five rows plus one to cover the gap");

        // Moving the anchor to another line invalidates the offset.
        ed.windows.selected_mut().scroll_top = 4;
        let v = &redisplay(&mut ed, area, &DisplayOptions::default())[0];
        assert_eq!(v.scroll_y_px, 0);
    }

    #[test]
    fn scroll_bar_reserves_a_column_and_tracks_position() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let opts = DisplayOptions {
            scroll_bar: true,
            ..Default::default()
        };
        // Point at the top: the thumb is at the start of the track.
        let v = one_window(&text, 20, 11, |b| b.set_point(Pos::new(0, 0)), &opts);
        assert_eq!(
            v.text_width, 20,
            "text area keeps its width; a column is taken from the text, not the window"
        );
        let (start, len) = v.scroll_bar.expect("a scroll bar");
        assert_eq!(start, 0, "at the top");
        assert!(
            (1..10).contains(&len),
            "the thumb is a fraction of 10 rows: {len}"
        );

        // Point far down: the thumb moves down, same size.
        let v = one_window(&text, 20, 11, |b| b.set_point(Pos::new(90, 0)), &opts);
        let (start2, len2) = v.scroll_bar.expect("a scroll bar");
        assert!(
            start2 > start,
            "thumb followed the scroll: {start2} > {start}"
        );
        assert_eq!(len2, len, "the thumb keeps its size");

        // Off (the default): no scroll bar column at all.
        let v = one_window(
            &text,
            20,
            11,
            |b| b.set_point(Pos::new(0, 0)),
            &DisplayOptions::default(),
        );
        assert!(v.scroll_bar.is_none());
    }

    #[test]
    fn horizontal_scroll_bar_takes_a_row_and_sizes_to_the_line() {
        let text: String = (0..50).map(|_| format!("{}\n", "x".repeat(200))).collect();
        let opts = DisplayOptions {
            horizontal_scroll_bar: true,
            ..Default::default()
        };
        let v = one_window(&text, 40, 11, |b| b.set_point(Pos::new(0, 0)), &opts);
        let (start, len) = v.horizontal_scroll_bar.expect("a horizontal scroll bar");
        assert_eq!(
            v.rows.len(),
            9,
            "a row for the mode line and one for the bar"
        );
        assert_eq!(start, 0, "point is at the left, so the thumb is too");
        assert!(
            (1..39).contains(&len),
            "the thumb is a fraction of the track: {len}"
        );

        // Off (the default): no bar, and the full height is text.
        let v = one_window(
            &text,
            40,
            11,
            |b| b.set_point(Pos::new(0, 0)),
            &DisplayOptions::default(),
        );
        assert!(v.horizontal_scroll_bar.is_none());
        assert_eq!(v.rows.len(), 10);
    }

    #[test]
    fn folds_do_not_scroll_shown_point() {
        // Hidden lines 1..3, point on visible line 14: its buffer number
        // is past the window height, but only eleven visible lines are
        // above it. Buffer-line arithmetic would scroll; counting shown
        // lines keeps the viewport.
        let text: String = (0..16).map(|i| format!("line {i}\n")).collect();
        let v = one_window(
            &text,
            40,
            13,
            |b| {
                b.fold_lines(1, 3);
                b.set_point(Pos::new(14, 0));
            },
            &DisplayOptions::default(),
        );
        assert!(row_text(&v.rows[0]).starts_with("line 0"));
        assert!(row_text(&v.rows[11]).starts_with("line 14"));
    }

    #[test]
    fn indent_mode_prefixes_and_masks_stars() {
        let v = one_window(
            "* A\nbody\n** B\ndeep\n",
            40,
            12,
            |b| {
                b.set_major_mode(crate::org::MODE);
                b.org_display.indent = true;
                b.set_point(Pos::new(0, 0));
            },
            &DisplayOptions::default(),
        );
        // Body aligns under headline text; L2 headlines stay at column 0.
        assert_eq!(row_text(&v.rows[0]), "* A");
        assert_eq!(row_text(&v.rows[1]), "  body");
        // Only one star stays visible; the rest are blank.
        assert_eq!(row_text(&v.rows[2]), "*  B");
        assert_eq!(row_text(&v.rows[3]), "   deep");
        // Masked stars lose their headline face.
        let cells: Vec<(char, Face)> = v.rows[2]
            .iter()
            .flat_map(|s| s.text.chars().map(|c| (c, s.face)).collect::<Vec<_>>())
            .collect();
        assert!(matches!(cells[0], ('*', Face::Syntax(_))), "{cells:?}");
        assert!(matches!(cells[1], (' ', Face::Default)), "{cells:?}");
    }

    #[test]
    fn num_mode_numbers_headlines_after_the_stars() {
        let v = one_window(
            "* A\n** B\nbody\n",
            40,
            12,
            |b| {
                b.set_major_mode(crate::org::MODE);
                b.org_display.num = true;
                b.set_point(Pos::new(2, 0));
            },
            &DisplayOptions::default(),
        );
        assert_eq!(row_text(&v.rows[0]), "* 1 A");
        assert_eq!(row_text(&v.rows[1]), "** 1.1 B");
        assert_eq!(row_text(&v.rows[2]), "body");
    }

    #[test]
    fn num_mode_shifts_the_cursor_past_the_number() {
        let v = one_window(
            "* A\nbody\n",
            40,
            12,
            |b| {
                b.set_major_mode(crate::org::MODE);
                b.org_display.num = true;
                b.set_point(Pos::new(0, 3));
            },
            &DisplayOptions::default(),
        );
        assert_eq!(row_text(&v.rows[0]), "* 1 A");
        // "* 1 A": the end of the line is column 5.
        assert_eq!(v.cursor.0, 5);
    }

    #[test]
    fn descriptive_links_stay_collapsed_with_point_inside() {
        let v = one_window(
            "see [[https://x.org][the site]] and [[https://y.org][other]] here\n",
            60,
            12,
            |b| {
                b.set_major_mode(crate::org::MODE);
                b.set_point(Pos::new(0, 0));
            },
            &DisplayOptions::default(),
        );
        assert_eq!(
            row_text(&v.rows[0]),
            "see the site and other here",
            "point outside every link collapses them all"
        );
        // The description keeps its Link face; the brackets are gone.
        let cells: Vec<(char, Face)> = v.rows[0]
            .iter()
            .flat_map(|s| s.text.chars().map(|c| (c, s.face)).collect::<Vec<_>>())
            .collect();
        assert!(matches!(cells[4], ('t', Face::Syntax(_))), "{cells:?}");
        // Put point inside the first link's hidden markup:,
        // nothing expands, and the cursor sits on the next visible char.
        let v = one_window(
            "see [[https://x.org][the site]] and [[https://y.org][other]] here\n",
            60,
            12,
            |b| {
                b.set_major_mode(crate::org::MODE);
                b.set_point(Pos::new(0, 10));
            },
            &DisplayOptions::default(),
        );
        assert_eq!(row_text(&v.rows[0]), "see the site and other here");
        assert_eq!(v.cursor.0, 4);
    }

    #[test]
    fn image_mode_places_its_picture_over_the_window() {
        let v = one_window(
            "not text",
            40,
            12,
            |b| {
                b.set_major_mode(crate::buffer::IMAGE_MODE);
                b.image = Some(crate::buffer::ImageState {
                    rendered: Some(std::path::PathBuf::from("/tmp/pic.png")),
                    ..Default::default()
                });
            },
            &DisplayOptions::default(),
        );
        assert_eq!(v.images.len(), 1, "the picture is placed");
        assert_eq!(v.images[0].row, 0);
        assert!(
            v.images[0].rows >= 8,
            "it covers the body: {}",
            v.images[0].rows
        );
        assert_eq!(v.images[0].path, "/tmp/pic.png");
        assert!(
            row_text(&v.rows[0]).trim().is_empty(),
            "the raw bytes are hidden"
        );
    }

    #[test]
    fn picture_links_get_blank_rows_and_a_placement() {
        let _guard = IMAGE_ROWS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("forge-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.png"), b"x").unwrap();
        crate::org_display::set_inline_image_rows(3);
        let v = one_window(
            "one\n[[file:p.png]]\ntail\npoint\n",
            40,
            12,
            |b| {
                b.set_visited_file(dir.join("t.org"));
                b.org_display.link_preview = true;
                b.set_point(Pos::new(3, 0));
            },
            &DisplayOptions::default(),
        );
        crate::org_display::set_inline_image_rows(0);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(v.images.len(), 1);
        assert_eq!((v.images[0].row, v.images[0].rows), (2, 3));
        assert!(
            row_text(&v.rows[5]).starts_with("tail"),
            "{}",
            row_text(&v.rows[5])
        );
        assert_eq!(v.cursor.1, 6);
    }

    /// A PNG header saying the picture is `w` x `h` (enough for its size).
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v
    }

    #[test]
    fn attr_width_sets_the_picture_width_and_its_rows() {
        let _guard = IMAGE_ROWS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("forge-attrw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.png"), png_header(342, 513)).unwrap();
        crate::org_display::set_inline_image_rows(8);
        crate::org_display::set_cell_size(8.0, 16.0);
        let show = |text: &str| {
            one_window(
                text,
                100,
                60,
                |b| {
                    b.set_visited_file(dir.join("t.org"));
                    b.org_display.link_preview = true;
                    b.set_point(Pos::new(4, 0));
                },
                &DisplayOptions::default(),
            )
        };
        // 250 wide, 342x513: 375 tall, 24 rows of 16.
        let v = show("#+attr_org: :width 250\n[[file:p.png]]\ntail\n\nhere\n");
        let im = &v.images[0];
        assert_eq!(im.rows, 24);
        assert_eq!(im.descriptor.as_ref().unwrap().width, Some(250.0));
        // Smaller asks for fewer rows, bigger for more.
        let v = show("#+attr_org: :width 100\n[[file:p.png]]\ntail\n\nhere\n");
        assert_eq!(v.images[0].rows, 10);
        let v = show("#+ATTR_HTML: :width 50%\n[[file:p.png]]\ntail\n\nhere\n");
        assert_eq!(v.images[0].descriptor.as_ref().unwrap().width, Some(400.0));
        // ATTR_ORG beats ATTR_HTML.
        let v =
            show("#+attr_html: :width 300\n#+attr_org: :width 120\n[[file:p.png]]\ntail\n\nhere\n");
        assert_eq!(v.images[0].descriptor.as_ref().unwrap().width, Some(120.0));
        // No attribute: the picture's own size (342 < fill-column's reach).
        let v = show("[[file:p.png]]\ntail\n\n\nhere\n");
        assert_eq!(v.images[0].descriptor.as_ref().unwrap().width, Some(342.0));
        // Taller than the window: the rows stop at the window.
        let v = one_window(
            "#+attr_org: :width 900\n[[file:p.png]]\ntail\n\nhere\n",
            120,
            12,
            |b| {
                b.set_visited_file(dir.join("t.org"));
                b.org_display.link_preview = true;
                b.set_point(Pos::new(4, 0));
            },
            &DisplayOptions::default(),
        );
        assert!(v.images[0].rows <= 11);
        crate::org_display::set_cell_size(0.0, 0.0);
        crate::org_display::set_inline_image_rows(0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_display_property_image_gets_a_placement() {
        let _guard = IMAGE_ROWS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("forge-disp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("p.png");
        std::fs::write(&pic, b"x").unwrap();
        crate::org_display::set_inline_image_rows(3);
        let v = one_window(
            "one\n[image here]\ntail\npoint\n",
            40,
            12,
            |b| {
                b.add_display_prop(
                    8,
                    13,
                    crate::buffer::DisplaySpec {
                        name: "image".to_string(),
                        args: vec![
                            crate::buffer::DisplayValue::Symbol("file".to_string()),
                            crate::buffer::DisplayValue::Str(pic.display().to_string()),
                        ],
                    },
                );
                b.set_point(Pos::new(3, 0));
            },
            &DisplayOptions::default(),
        );
        crate::org_display::set_inline_image_rows(0);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(v.images.len(), 1);
        assert_eq!((v.images[0].row, v.images[0].rows), (2, 3));
    }

    #[test]
    fn a_plugin_pin_gets_blank_rows_without_org_link_preview() {
        let _guard = IMAGE_ROWS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("forge-pin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("p.png");
        std::fs::write(&pic, b"x").unwrap();
        crate::org_display::set_inline_image_rows(3);
        let v = one_window(
            "one\n[[https://e.com/p.png]]\ntail\npoint\n",
            40,
            12,
            |b| {
                b.set_major_mode(crate::org::MODE);
                b.set_point(Pos::new(3, 0));
                b.set_plugin_images("org-remoteimg", vec![(1, pic.clone())]);
            },
            &DisplayOptions::default(),
        );
        crate::org_display::set_inline_image_rows(0);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(v.images.len(), 1, "a plugin pin makes a placement");
        assert_eq!(v.images[0].path, pic.to_string_lossy());
    }

    #[test]
    fn display_replacements_show_instead_of_text_even_at_point() {
        use crate::decoration::{DecoFace, DecoSpec, Display, ModernFace};
        let text = "#+title: Notes\n* TODO Task\nlast\n";
        let v = one_window(
            text,
            40,
            8,
            |b| {
                let todo = DecoFace::Modern(ModernFace::Todo);
                b.set_display(
                    "om",
                    vec![
                        DecoSpec::new(0, 2, todo).display(Display::Hide),
                        DecoSpec::new(15, 17, DecoFace::Modern(ModernFace::Symbol))
                            .display(Display::Text("◉ ".into())),
                        DecoSpec::new(17, 21, todo).display(Display::Text(" TODO ".into())),
                    ],
                );
                b.set_point(Pos::new(2, 0));
            },
            &DisplayOptions::default(),
        );
        assert_eq!(row_text(&v.rows[0]), "title: Notes");
        assert_eq!(row_text(&v.rows[1]), "◉  TODO  Task");
        let v = one_window(
            text,
            40,
            8,
            |b| {
                b.set_display(
                    "om",
                    vec![
                        DecoSpec::new(0, 2, DecoFace::Modern(ModernFace::Todo))
                            .display(Display::Hide),
                    ],
                );
                b.set_point(Pos::new(0, 3));
            },
            &DisplayOptions::default(),
        );
        // The replacement stays on point's line, as org-modern's do in
        // Forge, and the cursor is drawn at the character point is on.
        assert_eq!(
            row_text(&v.rows[0]),
            "title: Notes",
            "point's line keeps it"
        );
        assert_eq!(v.cursor, (1, 0));
    }

    #[test]
    fn modern_face_names_are_known() {
        for n in [
            "label",
            "todo",
            "done",
            "priority",
            "tag",
            "date-active",
            "time-active",
            "date-inactive",
            "time-inactive",
            "progress-complete",
            "progress-incomplete",
            "block-name",
            "symbol",
            "horizontal-rule",
            "target",
        ] {
            assert!(
                crate::decoration::DecoFace::from_name(&format!("org-modern-{n}")).is_ok(),
                "{n}"
            );
        }
    }

    #[test]
    fn shrunk_table_lines_show_cut_text_except_at_point() {
        let text = "| <4> | |\n|-----+--|\n| abcdefgh | x |\n| ijklmnop | y |\n";
        let v = one_window(
            text,
            40,
            8,
            |b| {
                b.org_display.table_shrink = true;
                b.set_point(Pos::new(3, 2));
            },
            &DisplayOptions::default(),
        );
        assert!(
            row_text(&v.rows[2]).contains("abc…"),
            "{}",
            row_text(&v.rows[2])
        );
        assert!(
            row_text(&v.rows[3]).contains("ijklmnop"),
            "point's line is real: {}",
            row_text(&v.rows[3])
        );
        let v = one_window(
            text,
            40,
            8,
            |b| b.set_point(Pos::new(3, 2)),
            &DisplayOptions::default(),
        );
        assert!(row_text(&v.rows[2]).contains("abcdefgh"));
    }

    #[test]
    fn table_header_line_sits_on_top_when_the_header_scrolls_off() {
        let mut text = String::from("| head | x |\n|------+---|\n");
        for i in 0..30 {
            text.push_str(&format!("| row{i} | y |\n"));
        }
        let v = one_window(
            &text,
            40,
            6,
            |b| {
                b.org_display.table_header = true;
                b.set_point(Pos::new(30, 2));
            },
            &DisplayOptions::default(),
        );
        // Point is far down: the first screen row is the header, and the
        // cursor is still on a text row below it.
        let w = v.rows.len();
        assert_eq!(w, 5);
        assert!(
            row_text(&v.rows[0]).starts_with("| head | x |"),
            "{}",
            row_text(&v.rows[0])
        );
        assert_eq!(v.rows[0][0].face, Face::ModeLine);
        assert!(v.cursor.1 >= 1);
        assert!(
            row_text(&v.rows[v.cursor.1 as usize]).contains("row28")
                || row_text(&v.rows[v.cursor.1 as usize]).contains("row"),
            "{}",
            row_text(&v.rows[v.cursor.1 as usize])
        );
        // Near the top the header is just the table's first line.
        let v = one_window(
            &text,
            40,
            6,
            |b| {
                b.org_display.table_header = true;
                b.set_point(Pos::new(2, 2));
            },
            &DisplayOptions::default(),
        );
        assert!(row_text(&v.rows[0]).starts_with("| head"));
        assert_eq!(v.cursor.1, 2);
    }

    #[test]
    fn wrapping_by_characters_and_words() {
        assert_eq!(
            visual_rows("aaaa bbbb cc", 6, false),
            [0, 6, 12]
                .iter()
                .copied()
                .filter(|&c| c < 12)
                .collect::<Vec<_>>()
        );
        assert_eq!(visual_rows("aaaa bbbb cc", 6, true), [0, 5, 10]);
        assert_eq!(
            visual_rows("abcdefghij", 4, true),
            [0, 4, 8],
            "a word longer than the row breaks anywhere"
        );
        assert_eq!(visual_rows("", 4, true), [0]);
        assert_eq!(
            visual_rows("漢字漢字", 5, false),
            [0, 2],
            "wide characters don't split"
        );

        let opts = DisplayOptions {
            wrap: Wrap::Word,
            ..Default::default()
        };
        let v = one_window(
            "one two three four\nshort",
            10,
            6,
            |b| b.set_point(Pos::new(0, 15)),
            &opts,
        );
        let rows: Vec<String> = v.rows.iter().map(|r| row_text(r)).collect();
        assert_eq!(rows, ["one two ", "three four", "short", "", ""]);
        assert_eq!(v.cursor, (7, 1), "point (the o in four) on the second row");
        assert!(row_text(&v.mode_line).contains("Wrap"));
    }

    #[test]
    fn wrapped_windows_scroll_to_point() {
        let text: String = (0..10)
            .map(|i| format!("line{i} xxxx yyyy zzzz\n"))
            .collect();
        let opts = DisplayOptions {
            wrap: Wrap::Word,
            ..Default::default()
        };
        let v = one_window(&text, 10, 5, |b| b.set_point(Pos::new(5, 0)), &opts);
        // Each line is 2 rows ("line5 xxxx", "yyyy zzzz"); 4 text rows,
        // recentered: the line before point's, then point's.
        assert_eq!(row_text(&v.rows[0]), "line4 xxxx");
        assert_eq!(row_text(&v.rows[2]), "line5 xxxx");
        assert_eq!(v.cursor.1, 2);
    }

    #[test]
    fn visual_line_motion() {
        let mut b = Buffer::new("t");
        b.set_contents("aaaa bbbb cccc\nxy");
        // Rows of 5: "aaaa " "bbbb " "cccc".
        let o = b.pos_to_offset(Pos::new(0, 1));
        let down = move_visual_rows(&b, o, 1, 5, true);
        assert_eq!(b.offset_to_pos(down), Pos::new(0, 6));
        let down3 = move_visual_rows(&b, o, 3, 5, true);
        assert_eq!(b.offset_to_pos(down3), Pos::new(1, 1));
        assert_eq!(
            b.offset_to_pos(move_visual_rows(&b, down3, -1, 5, true)),
            Pos::new(0, 11)
        );
        assert_eq!(move_visual_rows(&b, 0, -1, 5, true), 0);
    }

    #[test]
    fn hl_line_fill_column_parens_and_whitespace() {
        let opts = DisplayOptions {
            hl_line: true,
            fill_column_indicator: true,
            show_trailing_whitespace: true,
            show_paren: true,
            ..Default::default()
        };
        let v = one_window(
            "(a b)  \n\tx\n",
            30,
            5,
            |b| {
                b.set_point(Pos::new(0, 0));
                b.fill_column = 20;
            },
            &opts,
        );
        assert_eq!(v.hl_rows, [0]);
        assert_eq!(v.fill_column, Some(20));
        let faces: Vec<(String, Face)> =
            v.rows[0].iter().map(|s| (s.text.clone(), s.face)).collect();
        assert_eq!(faces[0], ("(".into(), Face::ParenMatch));
        assert!(faces.contains(&(")".into(), Face::ParenMatch)));
        assert_eq!(
            faces.last().unwrap(),
            &("  ".into(), Face::TrailingWhitespace)
        );

        let v = one_window(
            "a b\n\tx\n",
            60,
            5,
            |b| b.set_option("whitespace", "true").unwrap(),
            &DisplayOptions::default(),
        );
        assert_eq!(row_text(&v.rows[0]), "a·b");
        assert_eq!(row_text(&v.rows[1]), "»       x");
        assert!(
            row_text(&v.mode_line).contains("Ws"),
            "{}",
            row_text(&v.mode_line)
        );
    }

    #[test]
    fn fill_column_indicator_takes_a_column_and_a_character() {
        let opts = DisplayOptions {
            fill_column_indicator: true,
            ..Default::default()
        };
        let v = one_window(
            "ab\ncd\n",
            20,
            4,
            |b| {
                b.fill_column = 5;
                b.set_option("display-fill-column-indicator-column", "8")
                    .unwrap();
                b.set_option("display-fill-column-indicator-character", "|")
                    .unwrap();
            },
            &opts,
        );
        assert_eq!(v.fill_column, Some(8));
        assert_eq!(v.fill_column_character.as_deref(), Some("|"));

        // Unset, they follow fill-column with the box-drawing default.
        let v = one_window("ab\ncd\n", 20, 4, |b| b.fill_column = 5, &opts);
        assert_eq!(v.fill_column, Some(5));
        assert_eq!(v.fill_column_character.as_deref(), Some("│"));
    }

    #[test]
    fn fill_column_indicator_column_or_character_nil_turns_it_off() {
        let opts = DisplayOptions {
            fill_column_indicator: true,
            ..Default::default()
        };
        for (name, value) in [
            ("display-fill-column-indicator-column", "nil"),
            ("display-fill-column-indicator-character", "nil"),
        ] {
            let v = one_window(
                "ab\ncd\n",
                20,
                4,
                |b| b.set_option(name, value).unwrap(),
                &opts,
            );
            assert_eq!(v.fill_column, None, "{name} nil turns the line off");
        }
    }

    #[test]
    fn popup_rows_show_margin_match_and_deprecation() {
        let spec = PopupSpec {
            anchor: 0,
            input: "al".into(),
            rows: vec![
                PopupRow {
                    text: "alpha".into(),
                    margin: "f".into(),
                    annotation: String::new(),
                    selected: false,
                    deprecated: false,
                },
                PopupRow {
                    text: "alpine".into(),
                    margin: "v".into(),
                    annotation: String::new(),
                    selected: false,
                    deprecated: true,
                },
            ],
            width: 0,
        };
        let v = popup_view(
            &spec,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            0,
            40,
            10,
            0,
            0,
        );
        let r0 = &v.rows[0];
        let hit: Vec<&Span> = r0.iter().filter(|s| s.face == Face::PopupMatch).collect();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].text, "al");
        assert!(
            r0.iter()
                .any(|s| s.face == Face::PopupMargin && s.text == "f"),
            "{r0:?}"
        );
        // A deprecated row is drawn dimmed, but the matched prefix still
        // highlights.
        let r1 = &v.rows[1];
        assert!(
            r1.iter()
                .any(|s| s.face == Face::PopupDeprecated && s.text.contains("pine")),
            "{r1:?}"
        );
        assert_eq!(r1.iter().filter(|s| s.face == Face::PopupMatch).count(), 1);
    }

    #[test]
    fn side_by_side_windows_have_a_border() {
        let mut ed = Editor::new();
        ed.create_buffer("*scratch*");
        ed.split_window(Direction::Columns).unwrap();
        let views = redisplay(
            &mut ed,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &DisplayOptions::default(),
        );
        assert!(views[0].border && !views[1].border);
        assert_eq!(views[0].text_width, 19);
        assert_eq!(views[0].mode_line[0].face, Face::ModeLine);
        assert_eq!(views[1].mode_line[0].face, Face::ModeLineInactive);
    }

    #[test]
    fn display_properties_change_what_a_row_shows() {
        use crate::buffer::{DisplaySpec, DisplayValue};
        let spec = |name: &str, args: Vec<DisplayValue>| DisplaySpec {
            name: name.into(),
            args,
        };
        let v = one_window(
            "abc def\nsecond\nthird\n",
            40,
            8,
            |b| {
                // "abc" is padded to 6 cells; the point stays on line 2.
                b.add_display_prop(0, 3, spec("min-width", vec![DisplayValue::Int(6)]));
                b.set_point(Pos::new(2, 0));
            },
            &DisplayOptions::default(),
        );
        let row: String = v.rows[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(row.trim_end(), "abc    def");
    }

    #[test]
    fn height_and_raise_come_out_as_style_runs() {
        use crate::buffer::{DisplaySpec, DisplayValue};
        let v = one_window(
            "x2 and more\nnext\n",
            40,
            8,
            |b| {
                b.add_display_prop(
                    1,
                    2,
                    DisplaySpec {
                        name: "height".into(),
                        args: vec![DisplayValue::Real(0.7)],
                    },
                );
                b.add_display_prop(
                    1,
                    2,
                    DisplaySpec {
                        name: "raise".into(),
                        args: vec![DisplayValue::Real(0.4)],
                    },
                );
                b.set_point(Pos::new(1, 0));
            },
            &DisplayOptions::default(),
        );
        let runs: Vec<_> = v.style_runs.iter().filter(|r| r.row == 0).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (
                runs[0].col,
                runs[0].cells,
                runs[0].height_pct,
                runs[0].raise_pct
            ),
            (1 + v.gutter, 1, 70, 40)
        );
    }

    #[test]
    fn a_margin_property_puts_text_before_the_line() {
        use crate::buffer::{DisplaySpec, DisplayValue};
        let v = one_window(
            "alpha\nbeta\ngamma\n",
            30,
            8,
            |b| {
                b.add_display_prop(
                    6,
                    10,
                    DisplaySpec {
                        name: "margin".into(),
                        args: vec![
                            DisplayValue::Str("left-margin".into()),
                            DisplayValue::Str("»".into()),
                        ],
                    },
                );
                b.set_point(Pos::new(0, 0));
            },
            &DisplayOptions::default(),
        );
        let row = |i: usize| -> String { v.rows[i].iter().map(|s| s.text.as_str()).collect() };
        assert_eq!(v.gutter, 2, "one cell of text and one of space");
        assert_eq!(row(0), "  alpha");
        assert_eq!(row(1), "» beta");
        assert_eq!(v.cursor, (2, 0), "the cursor is right of the margin");
    }

    #[test]
    fn right_margins_and_fringes_sit_beside_the_text() {
        use crate::buffer::{DisplaySpec, DisplayValue};
        let spec = |name: &str, args: Vec<DisplayValue>| DisplaySpec {
            name: name.into(),
            args,
        };
        let v = one_window(
            "alpha\nbeta\n",
            30,
            6,
            |b| {
                b.add_display_prop(
                    6,
                    10,
                    spec(
                        "margin",
                        vec![
                            DisplayValue::Str("right-margin".into()),
                            DisplayValue::Str("end".into()),
                        ],
                    ),
                );
                b.add_display_prop(
                    6,
                    10,
                    spec(
                        "right-fringe",
                        vec![DisplayValue::Symbol("right-arrow".into())],
                    ),
                );
                b.add_display_prop(
                    0,
                    5,
                    spec(
                        "left-fringe",
                        vec![DisplayValue::Symbol("left-triangle".into())],
                    ),
                );
                b.set_point(Pos::new(0, 0));
            },
            &DisplayOptions::default(),
        );
        let row = |i: usize| -> String { v.rows[i].iter().map(|s| s.text.as_str()).collect() };
        assert_eq!(v.gutter, 1, "the left fringe is one cell");
        assert!(row(0).starts_with("◀alpha"), "{:?}", row(0));
        let b = row(1);
        assert!(b.starts_with(" beta"), "{b:?}");
        assert!(b.contains("→end"), "fringe, then margin: {b:?}");
        assert_eq!(v.cursor, (1, 0));
    }

    #[test]
    fn tall_text_reserves_rows_and_widens_characters() {
        use crate::buffer::{DisplaySpec, DisplayValue};
        let opts = DisplayOptions {
            pixel_text: true,
            ..DisplayOptions::default()
        };
        let v = one_window(
            "ab\ncd\n",
            30,
            8,
            |b| {
                b.add_display_prop(
                    0,
                    1,
                    DisplaySpec {
                        name: "height".into(),
                        args: vec![DisplayValue::Real(2.0)],
                    },
                );
                b.set_point(Pos::new(1, 0));
            },
            &opts,
        );
        let row = |i: usize| -> String { v.rows[i].iter().map(|s| s.text.as_str()).collect() };
        assert_eq!(row(0).trim(), "", "a blank row above the big line");
        assert!(
            row(1).starts_with("a b"),
            "the big letter takes two cells: {:?}",
            row(1)
        );
        assert!(row(2).starts_with("cd"));
        assert_eq!(v.cursor.1, 2, "point is on the third row");
        let run = v.style_runs.iter().find(|r| r.row == 1).expect("a run");
        assert_eq!((run.cells, run.height_pct), (2, 200));
        // Without pixel text (a terminal) nothing changes.
        let t = one_window(
            "ab\ncd\n",
            30,
            8,
            |b| {
                b.add_display_prop(
                    0,
                    1,
                    DisplaySpec {
                        name: "height".into(),
                        args: vec![DisplayValue::Real(2.0)],
                    },
                );
            },
            &DisplayOptions::default(),
        );
        let first: String = t.rows[0].iter().map(|s| s.text.as_str()).collect();
        assert!(first.starts_with("ab"));
    }

    #[test]
    fn text_scale_draws_the_window_on_a_bigger_grid() {
        let opts = DisplayOptions {
            pixel_text: true,
            line_numbers: LineNumbers::Absolute,
            ..DisplayOptions::default()
        };
        let text = "ab\ncd\nef\n";
        let normal = one_window(text, 40, 11, |_| {}, &opts);
        let v = one_window(
            text,
            40,
            11,
            |b| {
                b.text_scale = 4; // 1.2^4 = 2.07 times
                b.set_point(Pos::new(1, 1));
            },
            &opts,
        );
        assert_eq!(normal.cell_scale_pct, 100);
        assert_eq!(v.cell_scale_pct, 207);
        // The window keeps its place; its text is counted in the bigger
        // cells, so fewer columns and rows fit (10 text rows / 2.07 = 4).
        assert_eq!((v.rect.width, v.rect.height), (40, 11));
        assert!(v.rows.len() <= 5, "{} rows", v.rows.len());
        assert!(
            usize::from(v.text_width) <= 40 * 100 / 207 + 1,
            "{}",
            v.text_width
        );
        // The line numbers are part of the grid, so they scale with it, and
        // nothing is widened or padded to fake the size.
        assert!(
            row_text(&v.rows[0]).contains("ab"),
            "{:?}",
            row_text(&v.rows[0])
        );
        assert!(v.style_runs.is_empty(), "{:?}", v.style_runs);
        assert_eq!(v.cursor, (4, 1), "point is in grid cells too");
        // The mode line is not scaled: it spans the whole window.
        assert!(width::line_width(&row_text(&v.mode_line)) >= 40);
        let off = one_window(
            "ab\n",
            30,
            4,
            |b| b.text_scale = 4,
            &DisplayOptions::default(),
        );
        assert_eq!(off.cell_scale_pct, 100, "a terminal ignores the scale");
    }

    #[test]
    fn a_slice_resolves_pixels_and_fractions() {
        use super::SliceValue;
        let d = ImageDescriptor {
            slice: Some([
                SliceValue {
                    value: 0.5,
                    fraction: true,
                },
                SliceValue::px(10),
                SliceValue {
                    value: 0.25,
                    fraction: true,
                },
                SliceValue::px(500),
            ]),
            ..Default::default()
        };
        assert_eq!(d.slice_rect((200, 100)), Some((100, 10, 50, 90)));
        assert_eq!(ImageDescriptor::default().slice_rect((200, 100)), None);
    }

    #[test]
    fn the_mode_line_knows_what_each_part_is() {
        let mut ed = Editor::default();
        let id = ed.create_buffer("notes.txt");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_contents("hello\nworld\n");
        ed.state
            .get_buffer_mut(id)
            .unwrap()
            .set_major_mode("text-mode");
        ed.show_buffer(id);
        let area = Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 4,
        };
        let opts = DisplayOptions {
            mode_line_size: true,
            ..Default::default()
        };
        let v = &redisplay(&mut ed, area, &opts)[0];
        let text = row_text(&v.mode_line);
        let find = |t: &ModeLineTarget| v.mode_line_hits.iter().find(|h| &h.target == t).unwrap();
        let name = find(&ModeLineTarget::BufferName);
        assert_eq!(
            &text[name.from as usize..name.from as usize + 9],
            "notes.txt"
        );
        assert_eq!(
            ModeLineHit::at(&v.mode_line_hits, name.from),
            Some(&ModeLineTarget::BufferName)
        );
        let major = find(&ModeLineTarget::MajorMode);
        assert_eq!(&text[major.from as usize..major.to as usize], "Text");
        assert!(text.contains("12 "), "{text}");
        // Leaving the numbers out drops their parts.
        let opts = DisplayOptions {
            mode_line_no_line: true,
            mode_line_no_column: true,
            ..Default::default()
        };
        let v = &redisplay(&mut ed, area, &opts)[0];
        assert!(!row_text(&v.mode_line).contains(" L1"));
        assert!(
            v.mode_line_hits
                .iter()
                .all(|h| h.target != ModeLineTarget::Line)
        );
        assert_eq!(size_indication(999), "999");
        assert_eq!(size_indication(2500), "2.5k");
    }
}
