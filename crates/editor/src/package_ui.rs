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
        error: "■",
        info: "◆",
        neutral: "○",
        check: "✓",
        cross: "✗",
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

    const SPINNER: [&'static str; 4] = ["◐", "◓", "◑", "◒"];

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
#[derive(Debug, Default)]
pub struct Text {
    text: String,
    chars: usize,
    lines: usize,
    decos: Vec<DecoSpec>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.chars += text.chars().count();
    }

    pub fn push_face(&mut self, text: &str, face: PackageFace) {
        let start = self.chars;
        self.push(text);
        if start < self.chars {
            self.decos
                .push(DecoSpec::new(start, self.chars, DecoFace::Package(face)));
        }
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
        self.push("\n");
        self.lines += 1;
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn show(self, app: &mut App, name: &str, mode: &str, point: Option<Pos>) {
        app.show_special_buffer(name, mode, &self.text);
        app.buf().set_decorations(OWNER, self.decos);
        if let Some(point) = point {
            app.buf().set_point(point);
        }
    }
}

/// A filled title pill with optional right-aligned metadata.
pub fn title(text: &mut Text, title: &str, right: &str, width: usize, symbols: Symbols) {
    let title = format!(" {title} ");
    text.push_face(&title, PackageFace::TitlePill);
    let used = display_width(&title) + display_width(right);
    if !right.is_empty() && used < width {
        text.push(&" ".repeat(width.saturating_sub(used + 1)));
        text.push_face(
            &fit(
                right,
                width.saturating_sub(display_width(&title) + 1),
                symbols.ellipsis,
            ),
            PackageFace::Heading,
        );
    }
    text.newline();
}

/// A section heading followed by a rule, with one space between.
pub fn heading(text: &mut Text, label: &str, width: usize, symbols: Symbols) {
    text.push_face(label, PackageFace::Heading);
    let used = display_width(label);
    if used < width {
        text.push(" ");
        text.push_face(
            &symbols.rule.repeat(width.saturating_sub(used + 1)),
            PackageFace::Rule,
        );
    }
}

/// A full-width band whose tint and mark follow a status.
pub fn band(text: &mut Text, status: Status, message: &str, width: usize, symbols: Symbols) {
    let face = match status {
        Status::Error => PackageFace::BandError,
        Status::Warning => PackageFace::BandWarning,
        _ => status.face(),
    };
    let label = format!(" {} {} ", symbols.mark(status), message);
    text.push_face(&pad(&label, width, symbols.ellipsis), face);
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
