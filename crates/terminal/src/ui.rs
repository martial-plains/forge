// SPDX-License-Identifier: MIT OR Apache-2.0
//! Paints what `forge_core::display` describes. No layout or scrolling
//! decisions happen here: this file only maps faces to colors.

use crate::app::App;
use crate::caps::TermCaps;
use forge_core::decoration::{
    ANNOTATE_AGES, DecoFace, DiredFace, ModernFace, PackageFace, RAINBOW_DEPTHS, RainbowFace,
    VcFace,
};
use forge_core::display::{self, Face, Span as CoreSpan};
use forge_core::syntax::SyntaxFace;
use forge_editor::ui::{display_options, echo_line, vertical_rows};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

fn style(face: Face, caps: TermCaps, theme: &forge_core::theme::Theme) -> Style {
    let mut out = match face {
        Face::Default => Style::default(),
        Face::Region => Style::default().add_modifier(Modifier::REVERSED),
        Face::Match => Style::default().bg(Color::Yellow).fg(Color::Black),
        Face::Escape => Style::default().fg(Color::Magenta),
        Face::Truncation => Style::default().fg(Color::DarkGray),
        Face::ModeLine => Style::default().add_modifier(Modifier::REVERSED),
        Face::ModeLineInactive => Style::default().fg(Color::Gray).bg(Color::DarkGray),
        Face::Border => Style::default().fg(Color::DarkGray),
        // The thumb; the track is dimmer (see the scroll-bar paint).
        Face::ScrollBar => Style::default().fg(Color::Gray).bg(Color::DarkGray),
        Face::Syntax(s) => syntax_style(s),
        Face::Decoration(d, s) => {
            let base = s.map_or_else(Style::default, syntax_style);
            // Underline in the severity's color. A terminal that can't color
            // an underline by itself (see caps.rs) gets the text colored
            // instead: setting the underline color there would come out as
            // stray attributes, not as a color.
            let line = |c: Color| {
                let underlined = base.add_modifier(Modifier::UNDERLINED);
                if caps.underline_color {
                    underlined.underline_color(c)
                } else {
                    underlined.fg(c)
                }
            };
            match d {
                DecoFace::Error | DecoFace::SpellError => line(Color::Red),
                DecoFace::Warning => line(Color::Yellow),
                DecoFace::Info => line(Color::Blue),
                DecoFace::Highlight => base.bg(Color::Blue).fg(Color::White),
                DecoFace::SymbolHighlight => {
                    base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                }
                // The file name above a file's matches in *grep*.
                DecoFace::GrepHeading => base.add_modifier(Modifier::BOLD),
                DecoFace::Underline => base.add_modifier(Modifier::UNDERLINED),
                // No color of its own: a decoration that is only here for
                // its keymap, or a name a listing has no type for.
                DecoFace::Plain => base,
                DecoFace::Syntax(s) => syntax_style(s),
                DecoFace::Dired(f) => dired_style(f),
                DecoFace::Rainbow(f) => rainbow_style(f),
                DecoFace::Vc(f) => vc_style(f),
                DecoFace::Modern(f) => modern_style(f, base),
                DecoFace::Package(f) => package_style(f),
                DecoFace::Term(t) => term_style(t),
            }
        }
        Face::LineNumber => Style::default().fg(Color::DarkGray),
        Face::LineNumberCurrent => Style::default().add_modifier(Modifier::BOLD),
        Face::ParenMatch => Style::default().bg(Color::Cyan).fg(Color::Black),
        Face::ParenMismatch => Style::default().bg(Color::Magenta).fg(Color::White),
        Face::TrailingWhitespace => Style::default().bg(Color::Red),
        Face::Whitespace => Style::default().fg(Color::DarkGray),
        Face::FillColumn => Style::default().fg(Color::DarkGray),
        Face::Fold => Style::default().fg(Color::Cyan),
        Face::Popup => Style::default().bg(Color::Indexed(237)).fg(Color::White),
        Face::PopupSelected => Style::default().bg(Color::Blue).fg(Color::White),
        // The bar reads as a label, so it's reversed like the mode line;
        // an open title or item is highlighted, and an item whose command
        // is gone is dimmed.
        Face::MenuBar => Style::default().add_modifier(Modifier::REVERSED),
        Face::MenuSelected => Style::default()
            .add_modifier(Modifier::REVERSED)
            .bg(Color::Blue)
            .fg(Color::White),
        Face::MenuDisabled => Style::default()
            .add_modifier(Modifier::REVERSED)
            .fg(Color::DarkGray),
        // The tool bar and its modifier buttons read like the menu bar:
        // reversed, with the button under the pointer highlighted and an
        // unavailable one dimmed.
        Face::ToolBar => Style::default().add_modifier(Modifier::REVERSED),
        Face::ToolBarSelected => Style::default()
            .add_modifier(Modifier::REVERSED)
            .bg(Color::Blue)
            .fg(Color::White),
        Face::ToolBarDisabled => Style::default()
            .add_modifier(Modifier::REVERSED)
            .fg(Color::DarkGray),
        // The tab bar's row: a dim label, with the current tab boxed-out
        // (reversed) the way a GUI tab strip marks it.
        Face::TabBar => Style::default().fg(Color::Gray).bg(Color::Indexed(236)),
        Face::TabBarSelected => Style::default()
            .add_modifier(Modifier::BOLD)
            .bg(Color::Indexed(240)),
        Face::PopupAnnotation => Style::default().bg(Color::Indexed(237)).fg(Color::Gray),
        Face::PopupMargin => Style::default().bg(Color::Indexed(237)).fg(Color::Cyan),
        Face::PopupMatch => Style::default()
            .bg(Color::Indexed(237))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        Face::PopupDeprecated => Style::default()
            .bg(Color::Indexed(237))
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT),
        // The auxiliary popup (corfu-popupinfo): the same panel colour,
        // with the documentation in the usual text colour.
        Face::PopupInfo => Style::default().bg(Color::Indexed(237)).fg(Color::Gray),
    };
    // A theme restyles per field on top of the built-in style, so what
    // it leaves unset keeps working in light and dark terminals.
    if let Some(o) = theme.get(face.name()) {
        let rgb = |c: forge_core::theme::Rgb| Color::Rgb(c.0, c.1, c.2);
        if let Some(c) = o.fg {
            out.fg = Some(rgb(c));
        }
        if let Some(c) = o.bg {
            out.bg = Some(rgb(c));
        }
        if let Some(b) = o.bold {
            out = set_modifier(out, Modifier::BOLD, b);
        }
        if let Some(b) = o.italic {
            out = set_modifier(out, Modifier::ITALIC, b);
        }
        if let Some(u) = o.underline {
            out = set_modifier(out, Modifier::UNDERLINED, u);
            if u && caps.underline_color
                && let Some(c) = o.fg
            {
                out.underline_color = Some(rgb(c));
            }
        }
    }
    out
}

/// The soft surface behind the current line and a selected package row.
/// A dim palette gray, so it sits quietly under text instead of the
/// bright block reverse video makes.
const SELECTION_BG: Color = Color::Indexed(236);

/// Package manager: quiet titles and headings, a selected row on a soft
/// surface, and semantic status marks in the terminal's own palette.
/// Status labels use [`PackageFace::Neutral`], so they stay readable when
/// a terminal's green, yellow or cyan is too light for small text.
fn package_style(f: PackageFace) -> Style {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    match f {
        PackageFace::Title | PackageFace::Heading | PackageFace::Primary => bold,
        PackageFace::Selected => bold.bg(SELECTION_BG),
        PackageFace::SelectedMarker => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        PackageFace::Ok => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        PackageFace::Warning => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        PackageFace::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        PackageFace::Info => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        PackageFace::Neutral => Style::default().add_modifier(Modifier::REVERSED),
        PackageFace::Inactive => Style::default().add_modifier(Modifier::DIM),
        PackageFace::Rule => Style::default().fg(Color::DarkGray),
        PackageFace::TitlePill | PackageFace::TabActive => Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        PackageFace::Accent => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        PackageFace::BadgeWarning => Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        PackageFace::BadgeInactive => Style::default()
            .fg(Color::Black)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
        PackageFace::BadgeInfo => Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        PackageFace::BadgeError => Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD),
        PackageFace::BandWarning | PackageFace::Risk => Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        PackageFace::BandError => Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD),
    }
}

/// org-modern: labels as blocks of background color, in grays and the
/// terminal's own palette so they suit a light or dark theme; symbols
/// keep the text's own color. A theme can restyle each.
fn modern_style(f: ModernFace, base: Style) -> Style {
    let label = |bg: u8, fg: Color| Style::default().bg(Color::Indexed(bg)).fg(fg);
    match f {
        ModernFace::Label => label(238, Color::White),
        ModernFace::Todo => Style::default()
            .bg(Color::Red)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
        ModernFace::Done => label(238, Color::Gray),
        ModernFace::Priority => Style::default()
            .bg(Color::Magenta)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
        ModernFace::Tag => label(238, Color::White),
        ModernFace::DateActive => label(238, Color::White),
        ModernFace::TimeActive => label(250, Color::Black).add_modifier(Modifier::BOLD),
        ModernFace::DateInactive => label(236, Color::Indexed(246)),
        ModernFace::TimeInactive => label(243, Color::Indexed(252)),
        ModernFace::ProgressComplete => label(245, Color::Black),
        ModernFace::ProgressIncomplete => label(236, Color::White),
        ModernFace::BlockName => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        ModernFace::Symbol => base,
        ModernFace::HorizontalRule => Style::default().fg(Color::DarkGray),
        ModernFace::Target => label(238, Color::Cyan),
    }
}

/// Rainbow-delimiters: the terminal's own palette, so the colours suit
/// a light or dark theme and stay clear of the red that means an error.
/// A theme can restyle each (`rainbow-depth-N`).
fn rainbow_style(f: RainbowFace) -> Style {
    let red = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    match f {
        RainbowFace::Depth(n) => Style::default().fg([
            Color::Yellow,
            Color::Magenta,
            Color::Cyan,
            Color::Green,
            Color::Blue,
            Color::LightYellow,
            Color::LightMagenta,
            Color::LightCyan,
            Color::LightGreen,
        ][usize::from(n.clamp(1, RAINBOW_DEPTHS)) - 1]),
        RainbowFace::Unmatched => red,
        RainbowFace::Mismatched => red.add_modifier(Modifier::UNDERLINED),
    }
}

/// Version control, in the terminal's palette: added text green and
/// removed red (refined changes reversed so they stand out within a
/// line), headers bold, the three sides of a conflict in their own colors,
/// and annotations from bright (new) to dim (old).
fn vc_style(f: VcFace) -> Style {
    let fg = |c: Color| Style::default().fg(c);
    let bold = |c: Color| Style::default().fg(c).add_modifier(Modifier::BOLD);
    let reversed = |c: Color| Style::default().bg(c).fg(Color::Black);
    match f {
        VcFace::DiffAdded => fg(Color::Green),
        VcFace::DiffRemoved => fg(Color::Red),
        VcFace::DiffContext => Style::default(),
        VcFace::DiffHeader => fg(Color::Gray).add_modifier(Modifier::BOLD),
        VcFace::DiffFileHeader => Style::default().add_modifier(Modifier::BOLD),
        VcFace::DiffHunkHeader => fg(Color::Cyan),
        VcFace::DiffRefineAdded => reversed(Color::Green),
        VcFace::DiffRefineRemoved => reversed(Color::Red),
        VcFace::DiffNote => fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        VcFace::LogViewMessage => Style::default().add_modifier(Modifier::BOLD),
        VcFace::LogViewRevision => fg(Color::Yellow),
        VcFace::LogViewAuthor => fg(Color::Cyan),
        VcFace::LogViewDate => fg(Color::Blue),
        VcFace::LogViewBody => Style::default(),
        VcFace::LogViewRef => bold(Color::Magenta),
        VcFace::VcDirHeader => fg(Color::Cyan),
        VcFace::VcDirHeaderValue => bold(Color::Yellow),
        VcFace::VcDirDirectory => bold(Color::Blue),
        VcFace::VcDirFile => Style::default(),
        VcFace::VcDirMark => bold(Color::Yellow),
        VcFace::VcDirEdited => fg(Color::Yellow),
        VcFace::VcDirAdded => fg(Color::Green),
        VcFace::VcDirUpToDate => fg(Color::DarkGray),
        VcFace::VcDirWarning => bold(Color::Red),
        VcFace::VcDirIgnored => fg(Color::DarkGray),
        VcFace::VcDirUnregistered => fg(Color::Magenta),
        VcFace::LogEditSummary => bold(Color::Blue),
        VcFace::LogEditOverlong => Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::UNDERLINED),
        VcFace::LogEditHeader => bold(Color::Magenta),
        VcFace::LogEditUnknownHeader => fg(Color::DarkGray),
        VcFace::LogEditFiles => fg(Color::DarkGray),
        VcFace::SmergeUpper => fg(Color::Red),
        VcFace::SmergeLower => fg(Color::Green),
        VcFace::SmergeBase => fg(Color::Yellow),
        VcFace::SmergeMarkers => bold(Color::Magenta),
        VcFace::SmergeRefinedAdded => reversed(Color::Green),
        VcFace::SmergeRefinedRemoved => reversed(Color::Red),
        VcFace::AnnotateAge(n) => {
            const AGES: [Color; ANNOTATE_AGES as usize] = [
                Color::LightRed,
                Color::Red,
                Color::LightYellow,
                Color::Yellow,
                Color::LightGreen,
                Color::Green,
                Color::LightCyan,
                Color::Cyan,
                Color::LightBlue,
                Color::Blue,
            ];
            fg(AGES[usize::from(n.clamp(1, ANNOTATE_AGES)) - 1])
        }
    }
}

/// A text attribute on or off.
fn set_modifier(style: Style, modifier: Modifier, on: bool) -> Style {
    if on {
        style.add_modifier(modifier)
    } else {
        style.remove_modifier(modifier)
    }
}

/// Colors from the terminal's own palette, so light and dark themes
/// both work.
/// Dired follows its default colors: bold blue directories, bold green
/// executables, cyan links, yellow/red/green r/w/x, purple inode numbers,
/// blue dates and underlined column headers. Sizes are shaded by
/// magnitude (green below a megabyte, then yellow, red and purple), and
/// [`DiredFace::Shade`] carries a color the listing computed itself.
fn dired_style(f: DiredFace) -> Style {
    let fg = |c| Style::default().fg(c);
    let bold = |c| Style::default().fg(c).add_modifier(Modifier::BOLD);
    let dim = fg(Color::DarkGray);
    match f {
        DiredFace::Header => bold(Color::Blue),
        DiredFace::ColumnHeader => {
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        }
        DiredFace::Directory | DiredFace::PermDir => bold(Color::Blue),
        DiredFace::Executable => bold(Color::Green),
        DiredFace::Symlink | DiredFace::PermLink => fg(Color::Cyan),
        DiredFace::Marked => bold(Color::Yellow),
        DiredFace::Flagged => bold(Color::Red),
        DiredFace::PermRead => fg(Color::Yellow),
        DiredFace::PermWrite => fg(Color::Red),
        DiredFace::PermExec => fg(Color::Green),
        DiredFace::Dim => dim,
        DiredFace::Size => fg(Color::Green),
        DiredFace::SizeMega => fg(Color::Yellow),
        DiredFace::SizeGiga => fg(Color::Red),
        DiredFace::SizeHuge => fg(Color::Magenta),
        DiredFace::User => fg(Color::Yellow),
        DiredFace::Group => fg(Color::Magenta),
        DiredFace::Date => fg(Color::Blue),
        DiredFace::Inode => fg(Color::Magenta),
        DiredFace::SymlinkTarget => fg(Color::Cyan),
        DiredFace::TreePrefix => fg(Color::DarkGray),
        DiredFace::Image => fg(Color::Magenta),
        DiredFace::Video => bold(Color::Magenta),
        DiredFace::Music => fg(Color::Cyan),
        DiredFace::Lossless => bold(Color::Cyan),
        DiredFace::Crypto => bold(Color::Green),
        DiredFace::Document => fg(Color::Green),
        DiredFace::Compressed => fg(Color::Red),
        DiredFace::Temp => dim,
        DiredFace::Compiled => fg(Color::Yellow),
        DiredFace::Build => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        DiredFace::Source => bold(Color::Yellow),
        DiredFace::GitNew => fg(Color::Green),
        DiredFace::GitModified => fg(Color::Yellow),
        DiredFace::GitDeleted | DiredFace::GitConflicted => bold(Color::Red),
        DiredFace::GitRenamed => fg(Color::Cyan),
        DiredFace::GitTypeChange => fg(Color::Magenta),
        DiredFace::GitIgnored => dim,
        DiredFace::Shade(rgb) => {
            let channel = |shift: u32| ((rgb >> shift) & 0xff) as u8;
            Style::default().fg(Color::Rgb(channel(16), channel(8), channel(0)))
        }
    }
}

/// A `term` cell run: its own colours, resolved by the emulator.
fn term_style(t: forge_core::decoration::TermFace) -> Style {
    use forge_core::decoration::TermFace as T;
    let rgb = |v: u32| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    let mut st = Style::default();
    let (mut fg, mut bg) = (t.fg, t.bg);
    if t.attrs & T::REVERSE != 0 {
        std::mem::swap(&mut fg, &mut bg);
        st = st
            .fg(bg.map_or(Color::Black, rgb))
            .bg(fg.map_or(Color::White, rgb));
        fg = None;
        bg = None;
    }
    if let Some(v) = fg {
        st = st.fg(rgb(v));
    }
    if let Some(v) = bg {
        st = st.bg(rgb(v));
    }
    for (bit, m) in [
        (T::BOLD, Modifier::BOLD),
        (T::DIM, Modifier::DIM),
        (T::ITALIC, Modifier::ITALIC),
        (T::UNDERLINE, Modifier::UNDERLINED),
        (T::STRIKE, Modifier::CROSSED_OUT),
    ] {
        if t.attrs & bit != 0 {
            st = st.add_modifier(m);
        }
    }
    st
}

fn syntax_style(s: SyntaxFace) -> Style {
    let fg = |c: Color| Style::default().fg(c);
    match s {
        SyntaxFace::Keyword => fg(Color::Magenta),
        SyntaxFace::String => fg(Color::Green),
        SyntaxFace::Escape => fg(Color::Cyan).add_modifier(Modifier::BOLD),
        SyntaxFace::Comment => fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        SyntaxFace::Function => fg(Color::Blue),
        SyntaxFace::Type => fg(Color::Yellow),
        SyntaxFace::Constant => fg(Color::Cyan),
        SyntaxFace::Builtin => fg(Color::Red),
        SyntaxFace::Property => fg(Color::LightBlue),
        SyntaxFace::Attribute => fg(Color::LightMagenta),
        // Colored and underlined, like link/org-link.
        SyntaxFace::Link => fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
        SyntaxFace::OrgBold => Style::default().add_modifier(Modifier::BOLD),
        SyntaxFace::OrgItalic => Style::default().add_modifier(Modifier::ITALIC),
        SyntaxFace::OrgUnderline => Style::default().add_modifier(Modifier::UNDERLINED),
        SyntaxFace::OrgStrike => Style::default().add_modifier(Modifier::CROSSED_OUT),
    }
}

fn line(spans: &[CoreSpan], caps: TermCaps, theme: &forge_core::theme::Theme) -> Line<'static> {
    Line::from(
        spans
            .iter()
            .map(|s| Span::styled(s.text.clone(), style(s.face, caps, theme)))
            .collect::<Vec<_>>(),
    )
}

/// A core rectangle as ratatui's (same fields, different type).
fn from_core(r: forge_core::Rect) -> Rect {
    Rect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

fn to_core(r: Rect) -> forge_core::Rect {
    forge_core::Rect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

/// A menu panel over the text: one item per row, `top` rows below the
/// bar's own row (a submenu cascades from its parent item). The rows are
/// already padded to a common width, so the highlighted one is a filled
/// block.
fn render_panel(
    f: &mut Frame,
    area: Rect,
    left: u16,
    y: u16,
    column: u16,
    rows: &[(String, Face)],
    caps: TermCaps,
    theme: &forge_core::theme::Theme,
) {
    let h = (rows.len() as u16).min(area.height.saturating_sub(y));
    if h == 0 {
        return;
    }
    let widest = rows
        .iter()
        .map(|(text, _)| text.width() as u16)
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = rows
        .iter()
        .take(h as usize)
        .map(|(text, face)| {
            line(
                &[CoreSpan {
                    text: text.clone(),
                    face: *face,
                }],
                caps,
                theme,
            )
        })
        .collect();
    // Keep the whole panel on screen: near the right edge it
    // slides left rather than being clipped.
    let x = column.min(area.width.saturating_sub(widest)).max(left);
    let r = Rect {
        x,
        y,
        width: widest,
        height: h,
    }
    .intersection(area);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines), r);
}

pub fn render(f: &mut Frame, app: &mut App) -> Vec<crate::gfx::Slot> {
    render_with(f, app, crate::caps::TermCaps::detect())
}

/// The same for a frontend that knows what it can draw (the GNOME and Windows
/// and Android apps draw this grid with their own text and pictures).
pub fn render_with(
    f: &mut Frame,
    app: &mut App,
    caps: crate::caps::TermCaps,
) -> Vec<crate::gfx::Slot> {
    let area = f.area();
    // A resized window re-wraps the text browser's page first, so what
    // draws below is already at the new width.
    forge_editor::few::maybe_rewrap(app);
    // Vertical completion takes rows between the windows and the prompt;
    // the menu bar takes one at the top. Both frontends ask core where
    // each area is, so a menu bar can't come out a row taller on one.
    let vertical = crate::minibuffer::vertical_view(app);
    let list_h = vertical.as_ref().map_or(0, |v| v.0.len() as u16);
    let areas = forge_core::menu::frame_areas_with(
        to_core(area),
        crate::menu::visible(app),
        forge_editor::toolbar::visible(app),
        forge_editor::toolbar::modifier_visible(app),
        forge_editor::tabs::visible(app),
        list_h,
        forge_editor::toolbar::position(app).is_bottom(),
    );
    let windows_area = from_core(areas.windows);
    let tool_area = from_core(areas.tool_bar);
    let modifier_area = from_core(areas.modifier_bar);
    let tab_area = from_core(areas.tab_bar);
    let list_area = from_core(areas.list);
    let echo_area = from_core(areas.echo);

    let mut opts = display_options(app);
    // A terminal frame has no title bar, so its name goes on the mode line
    // (the macOS app uses the window title instead).
    opts.mode_line_frame = true;
    let views = display::redisplay(&mut app.editor, to_core(windows_area), &opts);

    let mut point_cursor = None;
    let mut pictures: Vec<crate::gfx::Slot> = Vec::new();
    for v in &views {
        let r = Rect {
            x: v.rect.x,
            y: v.rect.y,
            width: v.rect.width,
            height: v.rect.height,
        };
        // The core lays out exactly the text rows it means us to draw
        // (header row included); a horizontal scroll bar has already taken
        // its row off the count.
        let text_h = (v.rows.len() as u16).min(r.height);
        let text = Rect {
            width: v.text_width,
            height: text_h,
            ..r
        };
        f.render_widget(
            Paragraph::new(
                v.rows
                    .iter()
                    .map(|row| line(row, caps, &app.editor.theme))
                    .collect::<Vec<_>>(),
            ),
            text,
        );
        // display-fill-column-indicator-mode: a thin line where rows end
        // before the fill column.
        if let Some(col) = v.fill_column {
            let x = text.x + col;
            if x < text.x + text.width {
                let marker = v.fill_column_character.as_deref().unwrap_or("│");
                let st = style(Face::FillColumn, caps, &app.editor.theme);
                for y in text.y..text.y + text.height {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    if cell.symbol() == " " {
                        cell.set_symbol(marker).set_style(st);
                    }
                }
            }
        }
        // hl-line-mode: point's line gets a background. Cells a face has
        // already tinted keep their colour; unstyled cells carry the
        // terminal's `Reset`, which counts as no background.
        for &row in &v.hl_rows {
            let y = text.y + row;
            if y < text.y + text.height {
                for x in text.x + v.gutter..text.x + text.width {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    if matches!(cell.style().bg, None | Some(Color::Reset)) {
                        cell.set_bg(SELECTION_BG);
                    }
                }
            }
        }
        if v.border {
            let bx = r.x + r.width - 1;
            let bar: Vec<Line> = (0..text_h)
                .map(|_| Line::styled("│", style(Face::Border, caps, &app.editor.theme)))
                .collect();
            f.render_widget(
                Paragraph::new(bar),
                Rect {
                    x: bx,
                    y: r.y,
                    width: 1,
                    height: text_h,
                },
            );
        }
        // scroll-bar-mode: a thin track with a solid thumb in the last
        // column of the text area (the column the core reserved).
        let thumb = style(Face::ScrollBar, caps, &app.editor.theme);
        let track = Style::default().fg(Color::DarkGray);
        if let Some((start, len)) = v.scroll_bar {
            let bx = text.x + text.width.saturating_sub(1);
            for row in 0..text.height {
                let y = text.y + row;
                if y >= area.height || bx >= area.width {
                    break;
                }
                let cell = &mut f.buffer_mut()[(bx, y)];
                if row >= start && row < start.saturating_add(len) {
                    cell.set_symbol("█").set_style(thumb);
                } else {
                    cell.set_symbol("│").set_style(track);
                }
            }
        }
        // horizontal-scroll-bar-mode: a track with a solid thumb on the row
        // just above the mode line, spanning the text region.
        if let Some((start, len)) = v.horizontal_scroll_bar {
            let by = r.y + text_h;
            let width = text.width.saturating_sub(v.gutter);
            for i in 0..width {
                let x = text.x + v.gutter + i;
                if x >= area.width || by >= area.height {
                    break;
                }
                let cell = &mut f.buffer_mut()[(x, by)];
                if i >= start && i < start.saturating_add(len) {
                    cell.set_symbol("█").set_style(thumb);
                } else {
                    cell.set_symbol("─").set_style(track);
                }
            }
        }
        if r.height >= 2 {
            let ml = Rect {
                y: r.y + r.height - 1,
                height: 1,
                ..r
            };
            f.render_widget(
                Paragraph::new(line(&v.mode_line, caps, &app.editor.theme)),
                ml,
            );
        }
        if v.selected {
            point_cursor = Some(v.cursor);
            app.cursor_shape = v.cursor_shape;
        }
        // A completion popup, over the text, and the auxiliary popup
        // beside it (corfu-popupinfo).
        for p in [&v.popup, &v.popup_info].into_iter().flatten() {
            for (i, row) in p.rows.iter().enumerate() {
                let r = Rect {
                    x: p.x,
                    y: p.y + i as u16,
                    width: p.width,
                    height: 1,
                }
                .intersection(area);
                f.render_widget(Clear, r);
                f.render_widget(Paragraph::new(line(row, caps, &app.editor.theme)), r);
            }
        }
        // Pictures for terminals that draw them, over their blank rows.
        // A popup covering one keeps its placeholder: the picture comes
        // back when the popup closes.
        let popup = v
            .popup
            .as_ref()
            .map(|p| (p.x, p.y, p.width, p.rows.len() as u16));
        let popup_info = v
            .popup_info
            .as_ref()
            .map(|p| (p.x, p.y, p.width, p.rows.len() as u16));
        for im in &v.images {
            let r = Rect {
                x: text.x,
                y: text.y.saturating_add(im.row),
                width: text.width,
                height: im.rows,
            }
            .intersection(area);
            if r.width == 0 || r.height == 0 {
                continue;
            }
            let overlaps = |(px, py, pw, ph): (u16, u16, u16, u16)| {
                r.x < px.saturating_add(pw)
                    && px < r.x.saturating_add(r.width)
                    && r.y < py.saturating_add(ph)
                    && py < r.y.saturating_add(r.height)
            };
            if popup.is_some_and(overlaps) || popup_info.is_some_and(overlaps) {
                continue;
            }
            // A terminal that can't draw pictures says what it would show.
            if matches!(caps.graphics, crate::caps::GraphicsProto::Off) {
                let name = std::path::Path::new(&im.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let note = format!(
                    "[picture: {name} — this terminal can't draw pictures (kitty, Ghostty and WezTerm can)]"
                );
                let row = Rect { height: 1, ..r };
                f.render_widget(
                    Paragraph::new(Span::styled(note, Style::default().fg(Color::DarkGray))),
                    row,
                );
            }
            pictures.push(crate::gfx::Slot {
                x: r.x,
                y: r.y,
                w: r.width,
                h: r.height,
                path: im.path.clone(),
                descriptor: im.descriptor.clone(),
            });
        }
    }
    if let Some((rows, _)) = &vertical {
        let lines: Vec<Line> = vertical_rows(rows, list_area.width as usize)
            .iter()
            .map(|r| line(r, caps, &app.editor.theme))
            .collect();
        f.render_widget(Paragraph::new(lines), list_area);
    }

    // ---- prefix hints (which-key style), over the bottom of the windows
    let hints_shown = match app.keypad_hints() {
        Some((name, hints)) => render_hints(f, &format!(" {name} "), &hints, windows_area),
        None => app.show_prefix_hints && render_prefix_hints(f, app, windows_area),
    };

    let menu_area = from_core(areas.menu);
    if menu_area.height > 0 {
        let row: Vec<CoreSpan> = crate::menu::row(app);
        f.render_widget(
            Paragraph::new(line(&row, caps, &app.editor.theme)),
            menu_area,
        );
    }

    // ---- tool bar: the default one or the selected mode's own
    if tool_area.height > 0 {
        let row: Vec<CoreSpan> = forge_editor::toolbar::row(app);
        f.render_widget(
            Paragraph::new(line(&row, caps, &app.editor.theme)),
            tool_area,
        );
    }

    // ---- modifier bar: buttons for the modifiers the next key gets
    if modifier_area.height > 0 {
        let row: Vec<CoreSpan> = forge_editor::toolbar::modifier_row(app);
        f.render_widget(
            Paragraph::new(line(&row, caps, &app.editor.theme)),
            modifier_area,
        );
    }

    // ---- tab bar: a row of tabs, just below the modifier bar
    if tab_area.height > 0 {
        let row: Vec<CoreSpan> = forge_editor::tabs::row(app);
        f.render_widget(
            Paragraph::new(line(&row, caps, &app.editor.theme)),
            tab_area,
        );
    }

    // ---- menu panels (main and submenu) drawn on top of the tab bar
    if menu_area.height > 0 {
        if let Some((column, rows)) = crate::menu::panel(app) {
            render_panel(
                f,
                area,
                menu_area.x,
                menu_area.y + 1,
                column,
                &rows,
                caps,
                &app.editor.theme,
            );
        }
        if let Some((column, top, rows)) = crate::menu::subpanel(app) {
            render_panel(
                f,
                area,
                menu_area.x,
                menu_area.y + 1 + top,
                column,
                &rows,
                caps,
                &app.editor.theme,
            );
        }
    }

    // ---- context menu: panels anchored where it was opened, over the text
    // (frame-relative, so they go where the windows start)
    for (column, row, rows) in crate::menu::context_panels(app) {
        let frame = from_core(areas.windows);
        render_panel(
            f,
            area,
            frame.x,
            frame.y + row,
            frame.x + column,
            &rows,
            caps,
            &app.editor.theme,
        );
    }

    // ---- echo area / minibuffer
    let (mut echo, mut echo_cursor) = echo_line(app);
    if let Some((_, count)) = &vertical {
        echo = format!("{count} {echo}");
        echo_cursor = echo_cursor.map(|c| c + count.width() + 1);
    }
    f.render_widget(Paragraph::new(echo), echo_area);

    // ---- cursor
    if echo_cursor.is_none() && hints_shown {
        echo_cursor = Some(app.message.as_deref().unwrap_or("").width());
    }
    match (echo_cursor, point_cursor) {
        (Some(x), _) => {
            let x = (x as u16).min(echo_area.width.saturating_sub(1));
            f.set_cursor_position(Position::new(echo_area.x + x, echo_area.y));
        }
        (None, Some((x, y))) => f.set_cursor_position(Position::new(x, y)),
        (None, None) => {}
    }
    pictures
}

/// What redisplay needs from the app (every frontend passes this).
/// Draw the keys that can follow the current prefix in columns, filled
/// top to bottom. Returns false if there was nothing to show.
fn render_prefix_hints(f: &mut Frame, app: &App, area: Rect) -> bool {
    if app.current_prefix().is_empty() {
        return false;
    }
    let title = format!(" {}- ", app.current_prefix().join(" "));
    render_hints(f, &title, &app.prefix_hints(), area)
}

/// Draw (key, what) pairs in columns over the bottom of `area`, under a
/// rule with `title`. Returns false if there was nothing to show.
fn render_hints(f: &mut Frame, title: &str, hints: &[(String, String)], area: Rect) -> bool {
    if hints.is_empty() || area.height < 3 {
        return false;
    }
    let key_w = hints.iter().map(|(k, _)| k.width()).max().unwrap_or(1);
    let cmd_w = hints.iter().map(|(_, c)| c.width()).max().unwrap_or(1);
    let width = area.width as usize;
    let col_w = (key_w + 3 + cmd_w + 3).min(width.max(1));
    let cols = (width / col_w).max(1);
    let max_rows = (area.height as usize * 2 / 3).max(1);
    let rows = hints.len().div_ceil(cols).min(max_rows);
    let capacity = rows * cols;
    let truncated = hints.len() > capacity;
    let shown = if truncated { capacity - 1 } else { hints.len() };

    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let prefix_style = Style::default().fg(Color::Magenta);
    let mut lines: Vec<Vec<Span>> = vec![Vec::new(); rows];
    for (i, (key, cmd)) in hints.iter().take(shown).enumerate() {
        let (row, col) = (i % rows, i / rows);
        let line = &mut lines[row];
        let used: usize = line.iter().map(|s| s.content.width()).sum();
        let target = col * col_w;
        if target > used {
            line.push(Span::raw(" ".repeat(target - used)));
        }
        line.push(Span::styled(format!("{key:>key_w$}"), key_style));
        line.push(Span::raw(" → "));
        let style = if cmd == "+prefix" {
            prefix_style
        } else {
            Style::default()
        };
        line.push(Span::styled(cmd.clone(), style));
    }
    if truncated {
        let more = format!("… {} more (C-h b lists all)", hints.len() - shown);
        let row = rows - 1;
        let used: usize = lines[row].iter().map(|s| s.content.width()).sum();
        let target = (cols - 1) * col_w;
        lines[row].push(Span::raw(" ".repeat(target.saturating_sub(used))));
        lines[row].push(Span::styled(
            more,
            Style::default().add_modifier(Modifier::DIM),
        ));
    }

    let height = rows as u16 + 1;
    let popup = Rect::new(area.x, area.y + area.height - height, area.width, height);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).block(
            Block::default()
                .borders(Borders::TOP)
                .title(title.to_string()),
        ),
        popup,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::GraphicsProto;

    fn deco(d: DecoFace, underline_color: bool) -> Style {
        style(
            Face::Decoration(d, None),
            TermCaps {
                underline_color,
                graphics: GraphicsProto::Off,
            },
            &forge_core::theme::Theme::new(),
        )
    }

    #[test]
    fn colors_the_underline_where_the_terminal_can() {
        for (d, color) in [
            (DecoFace::Error, Color::Red),
            (DecoFace::Warning, Color::Yellow),
            (DecoFace::Info, Color::Blue),
        ] {
            let s = deco(d, true);
            assert!(s.add_modifier.contains(Modifier::UNDERLINED));
            assert_eq!(s.underline_color, Some(color));
            assert_eq!(s.fg, None, "the text keeps its own color");
        }
    }

    #[test]
    fn package_faces_mark_status_and_keep_rows_quiet() {
        let package = |face| deco(DecoFace::Package(face), true);
        assert_eq!(package(PackageFace::Ok).fg, Some(Color::Green));
        assert_eq!(package(PackageFace::Warning).fg, Some(Color::Yellow));
        assert_eq!(package(PackageFace::Error).fg, Some(Color::Red));
        assert_eq!(package(PackageFace::Info).fg, Some(Color::Cyan));
        assert!(
            package(PackageFace::Neutral)
                .add_modifier
                .contains(Modifier::REVERSED),
            "status labels are badges without tinting their text"
        );
        let selected = package(PackageFace::Selected);
        assert_eq!(selected.bg, Some(SELECTION_BG));
        assert!(
            selected.add_modifier.contains(Modifier::BOLD),
            "the selected row stays bold on a soft surface"
        );
        assert!(
            !selected.add_modifier.contains(Modifier::REVERSED),
            "the selected row is dim, not reversed"
        );
        assert_eq!(package(PackageFace::SelectedMarker).fg, Some(Color::Blue));
        assert!(
            package(PackageFace::Inactive)
                .add_modifier
                .contains(Modifier::DIM)
        );
        for face in [PackageFace::TitlePill, PackageFace::TabActive] {
            let style = package(face);
            assert_eq!(style.fg, Some(Color::White), "{face:?}");
            assert_eq!(style.bg, Some(Color::Blue), "{face:?}");
            assert!(style.add_modifier.contains(Modifier::BOLD), "{face:?}");
        }
        assert_eq!(package(PackageFace::Accent).fg, Some(Color::Blue));
        for (face, fg, bg) in [
            (PackageFace::BadgeWarning, Color::Black, Color::Yellow),
            (PackageFace::BadgeInactive, Color::Black, Color::DarkGray),
            (PackageFace::BadgeInfo, Color::Black, Color::Cyan),
            (PackageFace::BadgeError, Color::Black, Color::Red),
            (PackageFace::BandWarning, Color::Black, Color::Yellow),
            (PackageFace::BandError, Color::Black, Color::Red),
            (PackageFace::Risk, Color::Black, Color::Yellow),
        ] {
            let style = package(face);
            assert_eq!(style.fg, Some(fg), "{face:?}");
            assert_eq!(style.bg, Some(bg), "{face:?}");
            assert!(style.add_modifier.contains(Modifier::BOLD), "{face:?}");
        }
    }

    #[test]
    fn rainbow_depths_are_distinct() {
        let colors: Vec<_> = (1..=RAINBOW_DEPTHS)
            .map(|n| rainbow_style(RainbowFace::Depth(n)).fg)
            .collect();
        for (i, c) in colors.iter().enumerate() {
            assert!(c.is_some());
            assert!(!colors[..i].contains(c), "depth {} repeats a colour", i + 1);
        }
        assert_eq!(rainbow_style(RainbowFace::Unmatched).fg, Some(Color::Red));
        assert!(
            rainbow_style(RainbowFace::Mismatched)
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn never_sets_an_underline_color_where_it_cannot() {
        for (d, color) in [
            (DecoFace::Error, Color::Red),
            (DecoFace::SpellError, Color::Red),
            (DecoFace::Warning, Color::Yellow),
            (DecoFace::Info, Color::Blue),
        ] {
            let s = deco(d, false);
            assert!(
                s.add_modifier.contains(Modifier::UNDERLINED),
                "still underlined"
            );
            assert_eq!(s.underline_color, None, "no SGR 58 for {d:?}");
            assert_eq!(s.fg, Some(color), "severity shows in the text instead");
        }
    }

    #[test]
    fn faces_without_an_underline_are_the_same_either_way() {
        for face in [
            Face::Default,
            Face::Region,
            Face::Syntax(SyntaxFace::Keyword),
            Face::Decoration(DecoFace::Highlight, None),
        ] {
            assert_eq!(
                style(
                    face,
                    TermCaps {
                        underline_color: true,
                        graphics: GraphicsProto::Off,
                    },
                    &forge_core::theme::Theme::new(),
                ),
                style(
                    face,
                    TermCaps {
                        underline_color: false,
                        graphics: GraphicsProto::Off,
                    },
                    &forge_core::theme::Theme::new(),
                )
            );
        }
    }

    #[test]
    fn a_theme_override_restyles_its_face() {
        use forge_core::theme::{FaceStyle, Rgb, Theme};
        let caps = TermCaps {
            underline_color: true,
            graphics: GraphicsProto::Off,
        };
        let plain = style(Face::Syntax(SyntaxFace::Comment), caps, &Theme::new());
        let mut theme = Theme::new();
        theme
            .set(
                "comment",
                FaceStyle {
                    fg: Some(Rgb(1, 2, 3)),
                    ..Default::default()
                },
            )
            .unwrap();
        let styled = style(Face::Syntax(SyntaxFace::Comment), caps, &theme);
        assert_eq!(styled.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(styled.bg, plain.bg, "unset fields keep working");
        // ...and other faces are untouched.
        assert_eq!(
            style(Face::Syntax(SyntaxFace::Keyword), caps, &theme).fg,
            Some(Color::Magenta)
        );
    }

    #[test]
    fn package_faces_use_palette_colors_and_theme_overrides() {
        use forge_core::theme::{FaceStyle, Rgb, Theme};
        let caps = TermCaps {
            underline_color: true,
            graphics: GraphicsProto::Off,
        };
        for face in PackageFace::ALL {
            let styled = style(
                Face::Decoration(DecoFace::Package(face), None),
                caps,
                &Theme::new(),
            );
            for color in [styled.fg, styled.bg].into_iter().flatten() {
                assert!(
                    !matches!(color, Color::Rgb(..)),
                    "{face:?} should use a terminal palette color, not {color:?}"
                );
            }
        }

        let mut theme = Theme::new();
        theme
            .set(
                "package-error",
                FaceStyle {
                    fg: Some(Rgb(1, 2, 3)),
                    bg: Some(Rgb(4, 5, 6)),
                    ..Default::default()
                },
            )
            .unwrap();
        let styled = style(
            Face::Decoration(DecoFace::Package(PackageFace::Error), None),
            caps,
            &theme,
        );
        assert_eq!(styled.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(styled.bg, Some(Color::Rgb(4, 5, 6)));
        assert_eq!(
            style(
                Face::Decoration(DecoFace::Package(PackageFace::Warning), None),
                caps,
                &theme,
            )
            .fg,
            Some(Color::Yellow),
            "other package faces keep their palette colors"
        );
    }
}
