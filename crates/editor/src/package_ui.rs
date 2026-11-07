// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared text and decorations for the package manager's special buffers.

use crate::app::App;
use forge_core::Pos;
use forge_core::decoration::{DecoFace, DecoSpec, PackageFace};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const OWNER: &str = "package-ui";

/// The meaning a package status label carries. The label itself is always
/// text; the mark is the redundant color cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warning,
    Error,
    Info,
    Neutral,
}

impl Status {
    pub fn of(text: &str) -> Self {
        let text = text.to_ascii_lowercase();
        if ["failed", "error", "needs newer", "conflicted"]
            .iter()
            .any(|word| text.contains(word))
        {
            Self::Error
        } else if [
            "fetching",
            "checking",
            "checkout",
            "submodule",
            "commit",
            "reviewing",
            "building",
            "queued",
            "awaiting",
            "reading",
            "installing",
        ]
        .iter()
        .any(|word| text.contains(word))
        {
            Self::Info
        } else if ["upgrade", "update", "review", "warning"]
            .iter()
            .any(|word| text.contains(word))
        {
            Self::Warning
        } else if [
            "loaded",
            "installed",
            "up to date",
            "unchanged",
            "ok",
            "no problems",
        ]
        .iter()
        .any(|word| text.contains(word))
        {
            Self::Ok
        } else {
            Self::Neutral
        }
    }

    pub fn face(self) -> PackageFace {
        match self {
            Self::Ok => PackageFace::Ok,
            Self::Warning => PackageFace::Warning,
            Self::Error => PackageFace::Error,
            Self::Info => PackageFace::Info,
            Self::Neutral => PackageFace::Neutral,
        }
    }
}

/// The glyphs a package screen uses. Terminals get ASCII, so a missing
/// font never turns a status into a box.
#[derive(Debug, Clone, Copy)]
pub struct Symbols {
    pub selected: &'static str,
    pub arrow: &'static str,
    pub dot: &'static str,
    pub rule: &'static str,
    pub ellipsis: &'static str,
    pub ok: &'static str,
    pub warning: &'static str,
    pub error: &'static str,
    pub info: &'static str,
    pub neutral: &'static str,
    pub check: &'static str,
    pub cross: &'static str,
    pub progress_full: &'static str,
    pub progress_empty: &'static str,
    pub ascii: bool,
}

impl Symbols {
    pub const UNICODE: Self = Self {
        selected: "▌",
        arrow: "→",
        dot: "·",
        rule: "─",
        ellipsis: "…",
        ok: "●",
        warning: "▲",
        error: "✕",
        info: "◆",
        neutral: "○",
        check: "✓",
        cross: "✕",
        progress_full: "█",
        progress_empty: "░",
        ascii: false,
    };

    pub const ASCII: Self = Self {
        selected: "|",
        arrow: "->",
        dot: "-",
        rule: "-",
        ellipsis: "...",
        ok: "*",
        warning: "!",
        error: "x",
        info: "i",
        neutral: "-",
        check: "+",
        cross: "x",
        progress_full: "#",
        progress_empty: "-",
        ascii: true,
    };

    const SPINNER: [&'static str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    /// A spinner frame. Terminals get a static `...` so a missing font
    /// never turns progress into a box.
    pub fn spinner(self, tick: usize) -> &'static str {
        if self.ascii {
            "..."
        } else {
            Self::SPINNER[tick % Self::SPINNER.len()]
        }
    }

    pub fn spinner_width(self) -> usize {
        if self.ascii { 3 } else { 1 }
    }

    pub fn for_app(app: &App) -> Self {
        if app.graphical {
            Self::UNICODE
        } else {
            Self::ASCII
        }
    }

    pub fn mark(self, status: Status) -> &'static str {
        match status {
            Status::Ok => self.ok,
            Status::Warning => self.warning,
            Status::Error => self.error,
            Status::Info => self.info,
            Status::Neutral => self.neutral,
        }
    }
}

/// A package buffer under construction, with char offsets for its faces.
///
/// A buffer made with a margin indents every line that has text, so a
/// screen can keep its content a few cells in from the window edge while
/// blank lines stay empty.
#[derive(Debug, Default)]
pub struct Text {
    text: String,
    chars: usize,
    lines: usize,
    decos: Vec<DecoSpec>,
    margin: usize,
    line_started: bool,
}

/// Cells kept clear on each side of a package screen.
pub const MARGIN: usize = 2;

/// The margin a window of this width gets. Narrow windows keep the room.
pub fn margin(width: usize) -> usize {
    if width >= 40 { MARGIN } else { 0 }
}

/// The width left for content in a window of this width.
pub fn inner(width: usize) -> usize {
    width.saturating_sub(2 * margin(width)).max(1)
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    /// A buffer for a window of this width, with the screen margin.
    pub fn for_width(width: usize) -> Self {
        Self {
            margin: margin(width),
            ..Self::default()
        }
    }

    fn raw(&mut self, text: &str) {
        self.text.push_str(text);
        self.chars += text.chars().count();
    }

    /// Write the margin if this line has no text yet.
    fn start_line(&mut self) {
        if !self.line_started {
            self.line_started = true;
            if self.margin > 0 {
                let margin = " ".repeat(self.margin);
                self.raw(&margin);
            }
        }
    }

    /// Start the line now, with the margin in `face`. A selected row uses
    /// this so its surface runs from the window edge.
    pub fn begin_line(&mut self, face: Option<PackageFace>) {
        if self.line_started {
            return;
        }
        match face {
            Some(face) if self.margin > 0 => {
                self.line_started = true;
                let start = self.chars;
                let margin = " ".repeat(self.margin);
                self.raw(&margin);
                self.decos
                    .push(DecoSpec::new(start, self.chars, DecoFace::Package(face)));
            }
            _ => self.start_line(),
        }
    }

    /// Fill the right margin in `face`, ending a row that began with
    /// [`Text::begin_line`].
    pub fn end_line_fill(&mut self, face: PackageFace) {
        if self.margin > 0 {
            self.push_face(&" ".repeat(self.margin), face);
        }
    }

    pub fn push(&mut self, text: &str) {
        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                self.start_line();
                self.raw(part);
            }
            if parts.peek().is_some() {
                self.newline();
            }
        }
    }

    pub fn push_face(&mut self, text: &str, face: PackageFace) {
        if text.is_empty() {
            return;
        }
        self.start_line();
        let start = self.chars;
        self.push(text);
        if start < self.chars {
            self.decos
                .push(DecoSpec::new(start, self.chars, DecoFace::Package(face)));
        }
    }

    /// The offset where the next text will go, after the line's margin.
    pub fn mark(&mut self) -> usize {
        self.start_line();
        self.chars
    }

    pub fn chars(&self) -> usize {
        self.chars
    }

    pub fn line_count(&self) -> usize {
        self.lines
    }

    /// Mark an already-written range as a clickable region.
    pub fn push_region(&mut self, start: usize, end: usize, keymap: &str) {
        if start < end {
            self.decos.push(DecoSpec::region(start, end, keymap));
        }
    }

    pub fn push_status(
        &mut self,
        status: Status,
        label: &str,
        width: usize,
        selected: bool,
        symbols: Symbols,
    ) {
        let label_face = if selected {
            PackageFace::Selected
        } else {
            PackageFace::Neutral
        };
        self.push_face(symbols.mark(status), status.face());
        self.push_face(" ", label_face);
        let room = width.saturating_sub(2);
        let label = fit(label, room, symbols.ellipsis);
        self.push_face(&label, label_face);
        let pad = room.saturating_sub(display_width(&label));
        if pad > 0 {
            self.push_face(&" ".repeat(pad), label_face);
        }
    }

    pub fn push_cell(&mut self, text: &str, width: usize, face: PackageFace, ellipsis: &str) {
        self.push_face(&pad(text, width, ellipsis), face);
    }

    pub fn newline(&mut self) {
        self.raw("\n");
        self.lines += 1;
        self.line_started = false;
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn show(self, app: &mut App, name: &str, mode: &str, point: Option<Pos>) {
        app.show_special_buffer(name, mode, &self.text);
        let _ = app.buf().set_option("line-numbers", "off");
        let _ = app.buf().set_option("hl-line", "true");
        app.buf().set_decorations(OWNER, self.decos);
        if let Some(point) = point {
            app.buf().set_point(point);
        }
    }
}

/// A filled title pill with optional right-aligned metadata, then a blank
/// line.
pub fn title(text: &mut Text, title: &str, right: &str, width: usize, symbols: Symbols) {
    let title = format!(" {title} ");
    text.push_face(&title, PackageFace::TitlePill);
    let used = display_width(&title) + display_width(right);
    if !right.is_empty() && used < width {
        text.push(&" ".repeat(width.saturating_sub(used)));
        text.push_face(
            &fit(
                right,
                width.saturating_sub(display_width(&title) + 1),
                symbols.ellipsis,
            ),
            PackageFace::Inactive,
        );
    }
    text.newline();
    text.newline();
}

/// A full-width rule and the line after it.
pub fn rule(text: &mut Text, width: usize, symbols: Symbols) {
    text.push_face(&symbols.rule.repeat(width), PackageFace::Rule);
    text.newline();
    text.newline();
}

/// A section heading. The caller ends the line.
pub fn heading(text: &mut Text, label: &str, _width: usize, _symbols: Symbols) {
    text.push_face(label, PackageFace::Heading);
}

/// A full-width band whose tint and mark follow a status.
pub fn band(text: &mut Text, status: Status, message: &str, width: usize, symbols: Symbols) {
    band_lines(text, status, &[message], width, symbols);
}

/// A band of several lines: the first carries the mark, the rest line up
/// under its message.
pub fn band_lines(text: &mut Text, status: Status, lines: &[&str], width: usize, symbols: Symbols) {
    let face = match status {
        Status::Error => PackageFace::BandError,
        Status::Warning => PackageFace::BandWarning,
        _ => status.face(),
    };
    for (index, line) in lines.iter().enumerate() {
        let label = if index == 0 {
            format!(" {} {line}", symbols.mark(status))
        } else {
            format!("   {line}")
        };
        text.push_face(&pad(&label, width, symbols.ellipsis), face);
        text.newline();
    }
}

/// One capability: a mark, its name, what it allows and a short tag. The
/// description column is at most 42 cells, so tags line up down the screen.
#[allow(clippy::too_many_arguments)]
pub fn capability_line(
    text: &mut Text,
    mark: &str,
    mark_face: PackageFace,
    name: &str,
    description: &str,
    tag: Option<(&str, PackageFace)>,
    name_width: usize,
    width: usize,
    symbols: Symbols,
) {
    text.push_face(mark, mark_face);
    text.push(" ");
    text.push_face(
        &pad(name, name_width, symbols.ellipsis),
        PackageFace::Primary,
    );
    let used = display_width(mark) + 1 + name_width;
    match tag {
        Some((tag, tag_face)) => {
            let room = width.saturating_sub(used + display_width(tag)).clamp(8, 42);
            text.push(&pad(description, room, symbols.ellipsis));
            text.push_face(tag, tag_face);
        }
        None => text.push(&fit(
            description,
            width.saturating_sub(used),
            symbols.ellipsis,
        )),
    }
    text.newline();
}

/// The width of a field label such as `source` or `command`.
pub const FIELD_LABEL: usize = 10;

/// A quiet label and its value, indented, on one line. A long value is cut
/// to fit.
pub fn field(
    text: &mut Text,
    indent: usize,
    label: &str,
    value: &str,
    value_face: Option<PackageFace>,
    width: usize,
    symbols: Symbols,
) {
    text.push(&" ".repeat(indent));
    text.push_face(
        &pad(label, FIELD_LABEL, symbols.ellipsis),
        PackageFace::Inactive,
    );
    let room = width.saturating_sub(indent + FIELD_LABEL).max(8);
    let value = fit(value, room, symbols.ellipsis);
    match value_face {
        Some(face) => text.push_face(&value, face),
        None => text.push(&value),
    }
    text.newline();
}

/// A tinted inline label.
pub fn chip(text: &mut Text, label: &str, face: PackageFace) {
    text.push_face(&format!(" {label} "), face);
}

pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut `s` to at most `width` display cells, ending in `ellipsis` when
/// something was cut.
pub fn fit(s: &str, width: usize, ellipsis: &str) -> String {
    if display_width(s) <= width {
        return s.to_string();
    }
    let ellipsis_width = display_width(ellipsis);
    if width <= ellipsis_width {
        return ellipsis.chars().take(width).collect();
    }
    let room = width - ellipsis_width;
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push_str(ellipsis);
    out
}

/// Pad `s` on the right to `width` display cells, cutting it when needed.
pub fn pad(s: &str, width: usize, ellipsis: &str) -> String {
    let s = fit(s, width, ellipsis);
    let mut out = s.clone();
    out.push_str(&" ".repeat(width.saturating_sub(display_width(&s))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_classified_by_words() {
        assert_eq!(Status::of("loaded"), Status::Ok);
        assert_eq!(Status::of("upgrade from 1.0"), Status::Warning);
        assert_eq!(Status::of("review needed"), Status::Warning);
        assert_eq!(Status::of("reviewing changes"), Status::Info);
        assert_eq!(Status::of("fetching Git"), Status::Info);
        assert_eq!(Status::of("needs newer Forge"), Status::Error);
        assert_eq!(Status::of("available"), Status::Neutral);
    }

    #[test]
    fn fit_uses_display_width() {
        assert_eq!(fit("abcdef", 4, "…"), "abc…");
        assert_eq!(fit("abc", 4, "…"), "abc");
        assert_eq!(fit("abcdef", 4, "..."), "a...");
    }
}
