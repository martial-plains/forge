// SPDX-License-Identifier: MIT OR Apache-2.0
//! Buffer features built on the syntax tree and decorations: highlighting,
//! indentation, decorations and options. (Split from buffer.rs, which is
//! about text, undo and files.)

use crate::buffer::{Buffer, SyntaxSlot};
use crate::decoration::{self, DecoSpec, Decoration, Display, LineNumbers, Wrap};
use crate::syntax::{self, IndentStyle, LanguageDef, Syntax, SyntaxFace};
use std::ops::Range;

impl Buffer {
    // ------------------------------------------------------------- syntax

    /// The grammar in use, if any.
    pub fn language(&self) -> Option<&'static LanguageDef> {
        self.syntax.0.as_ref().map(|s| s.language())
    }

    /// Use `def`'s grammar (None: no syntax tree). The editor calls this
    /// when the major mode changes.
    pub fn set_language(&mut self, def: Option<&'static LanguageDef>) {
        let same = match (self.language(), def) {
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.syntax = SyntaxSlot(def.and_then(Syntax::new).map(Box::new));
        }
    }

    /// Syntax faces for chars `range`, one per char (all None without a
    /// grammar).
    pub fn syntax_faces(&mut self, range: Range<usize>) -> Vec<Option<SyntaxFace>> {
        let end = range.end.min(self.char_count());
        let range = range.start.min(end)..end;
        if self.syntax.0.is_none() && self.major_mode() == crate::org::MODE {
            return self.org_faces(range);
        }
        if self.syntax.0.is_none() && self.major_mode() == crate::help_faces::MODE {
            return self.help_faces(range);
        }
        if self.syntax.0.is_none() && self.major_mode() == crate::agenda_faces::MODE {
            return self.line_faces_with(range, crate::agenda_faces::line_faces);
        }
        match self.syntax.0.as_mut() {
            Some(s) => s.faces(&self.text, range),
            None => vec![None; range.len()],
        }
    }

    /// Help highlighting (no grammar: a line at a time).
    fn help_faces(&self, range: Range<usize>) -> Vec<Option<SyntaxFace>> {
        self.line_faces_with(range, crate::help_faces::line_faces)
    }

    /// Faces for `range` from a function that colours one line.
    fn line_faces_with(
        &self,
        range: Range<usize>,
        faces_of: fn(&str) -> Vec<Option<SyntaxFace>>,
    ) -> Vec<Option<SyntaxFace>> {
        let mut out = Vec::with_capacity(range.len());
        let first = self.offset_to_pos(range.start).line;
        let last = self.offset_to_pos(range.end).line;
        for l in first..=last {
            let start = self.pos_to_offset(crate::Pos::new(l, 0));
            let mut f = faces_of(&self.line(l));
            f.push(None);
            for (i, face) in f.into_iter().enumerate() {
                if (range.start..range.end).contains(&(start + i)) {
                    out.push(face);
                }
            }
        }
        out.resize(range.len(), None);
        out
    }

    /// Org highlighting (no grammar: blocks and lines).
    fn org_faces(&self, range: Range<usize>) -> Vec<Option<SyntaxFace>> {
        self.org_faces_blocks(range)
    }

    /// The syntax tree as an S-expression (for debugging and tests).
    #[cfg(feature = "tree-sitter")]
    pub fn syntax_tree_sexp(&mut self) -> Option<String> {
        let s = self.syntax.0.as_mut()?;
        s.tree(&self.text).map(|t| t.root_node().to_sexp())
    }

    #[cfg(not(feature = "tree-sitter"))]
    pub fn syntax_tree_sexp(&mut self) -> Option<String> {
        None
    }

    // -------------------------------------------------------- indentation

    /// How this buffer indents: the language's style, changed by the
    /// indent-width / indent-tabs options. None without a language.
    pub fn indent_style(&self) -> Option<IndentStyle> {
        let base = self.language().map(|l| l.indent);
        if base.is_none() && self.options.indent_width.is_none() {
            return None;
        }
        let base = base.unwrap_or(IndentStyle {
            width: 4,
            tabs: false,
        });
        Some(IndentStyle {
            width: self.options.indent_width.unwrap_or(base.width),
            tabs: self.options.indent_tabs.unwrap_or(base.tabs),
        })
    }

    /// Columns of leading whitespace on `line`.
    pub fn line_indentation(&self, line: usize) -> usize {
        syntax::indentation_width(&self.line(line))
    }

    /// The columns `line` should be indented to, by the major mode's
    /// rules. None if the mode has none (no grammar).
    pub fn indentation_for(&mut self, line: usize) -> Option<usize> {
        let style = self.indent_style()?;
        let s = self.syntax.0.as_mut()?;
        let def = s.language();
        let levels = |cols: usize| cols / def.indent.width.max(1);
        let cols = match s.indent(&self.text, line) {
            Some(c) => c,
            None => syntax::heuristic_indent(&self.text, line, def),
        };
        // The rules count in the language's unit; the buffer may use another.
        if style.width == def.indent.width {
            return Some(cols);
        }
        Some(levels(cols) * style.width + cols % def.indent.width.max(1))
    }

    /// Re-indent `line` by the major mode's rules. Point stays on the
    /// same text, or moves to the indentation if it was in it. False if
    /// the mode has no indentation rules.
    pub fn indent_line(&mut self, line: usize) -> bool {
        let (Some(style), Some(cols)) = (self.indent_style(), self.indentation_for(line)) else {
            return false;
        };
        self.indent_line_to(line, cols, style);
        true
    }

    /// Set `line`'s leading whitespace to `cols` columns.
    pub fn indent_line_to(&mut self, line: usize, cols: usize, style: IndentStyle) {
        let text = self.line(line);
        let ws_chars = text.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let want = syntax::indent_string(cols, style);
        let start = self.pos_to_offset(crate::Pos::new(line, 0));
        let point = self.point_offset();
        if text.chars().take(ws_chars).collect::<String>() != want {
            self.replace_offsets(start, start + ws_chars, &want);
        }
        let new_ws = want.chars().count();
        let p = if point < start {
            point
        } else if point <= start + ws_chars {
            start + new_ws
        } else {
            point - ws_chars + new_ws
        };
        self.set_offset(p);
    }

    // -------------------------------------------------------- decorations

    /// Replace `owner`'s decorations with `items` (char offsets, face,
    /// message, and optionally the keymap the text brings with it).
    /// Empty or backwards ranges are skipped.
    pub fn set_decorations(&mut self, owner: &str, items: Vec<impl Into<DecoSpec>>) {
        // Display replacements are a set of their own (`set_display`).
        self.decorations
            .retain(|d| d.owner != owner || d.display.is_some());
        self.add_decorations(owner, items);
    }

    /// Replace `owner`'s display replacements (text hidden or shown as
    /// something else) with `items`; its faces and keymaps stay.
    pub fn set_display(&mut self, owner: &str, items: Vec<impl Into<DecoSpec>>) {
        self.decorations
            .retain(|d| d.owner != owner || d.display.is_none());
        self.add_decorations(owner, items);
    }

    fn add_decorations(&mut self, owner: &str, items: Vec<impl Into<DecoSpec>>) {
        let len = self.char_count();
        for spec in items {
            let spec = spec.into();
            let (start, end) = (spec.start.min(len), spec.end.min(len));
            if start < end {
                self.decorations.push(Decoration {
                    owner: owner.to_string(),
                    start,
                    end,
                    face: spec.face,
                    message: spec.message,
                    keymap: spec.keymap,
                    display: spec.display,
                });
            }
        }
        self.decorations.sort_by_key(|d| (d.start, d.end));
    }

    /// Whether any decoration replaces what it covers on screen.
    pub fn has_display(&self) -> bool {
        self.decorations.iter().any(|d| d.display.is_some()) || self.has_display_text()
    }

    /// Is there a `display` property (text or overlay) that changes the
    /// text it covers: `string`, `space`, `min-width`, `space-width`?
    pub fn has_display_text(&self) -> bool {
        self.display
            .iter()
            .any(|r| crate::display_spec::changes_text(&r.props))
            || self
                .overlay
                .iter()
                .any(|o| crate::display_spec::changes_text(&o.display.0))
    }

    /// The text a `((margin left-margin) STRING)` display property puts in
    /// a margin of `line` (`side` is "left-margin" or "right-margin"), if
    /// any: written `["margin", "left-margin", "text"]` in a script.
    pub fn margin_text(&self, line: usize, side: &str) -> Option<String> {
        let start = self.pos_to_offset(crate::Pos::new(line, 0));
        let end = start + self.line(line).chars().count();
        let in_line = |a: usize, b: usize| a <= end && b > start;
        self.overlay
            .iter()
            .filter(|o| in_line(o.start, o.end))
            .find_map(|o| margin_of(&o.display.0, side).map(str::to_string))
            .or_else(|| {
                self.display
                    .iter()
                    .filter(|r| in_line(r.start, r.end))
                    .find_map(|r| margin_of(&r.props, side).map(str::to_string))
            })
    }

    /// How wide a margin is: the widest text any line puts there. Zero
    /// when no display property uses it.
    pub fn margin_width(&self, side: &str) -> usize {
        let widest = |props: &[crate::buffer::DisplaySpec]| {
            margin_of(props, side).map_or(0, crate::width::line_width)
        };
        let text = self.display.iter().map(|r| widest(&r.props)).max();
        let overlays = self.overlay.iter().map(|o| widest(&o.display.0)).max();
        text.max(overlays).unwrap_or(0).min(40)
    }

    /// The character a `(left-fringe BITMAP)` or `(right-fringe BITMAP)`
    /// display property shows in the fringe of `line` (`side` is
    /// "left-fringe" or "right-fringe").
    pub fn fringe_char(&self, line: usize, side: &str) -> Option<char> {
        let start = self.pos_to_offset(crate::Pos::new(line, 0));
        let end = start + self.line(line).chars().count();
        let in_line = |a: usize, b: usize| a <= end && b > start;
        self.overlay
            .iter()
            .filter(|o| in_line(o.start, o.end))
            .find_map(|o| fringe_of(&o.display.0, side))
            .or_else(|| {
                self.display
                    .iter()
                    .filter(|r| in_line(r.start, r.end))
                    .find_map(|r| fringe_of(&r.props, side))
            })
    }

    /// Does any display property use this fringe?
    pub fn has_fringe(&self, side: &str) -> bool {
        self.display
            .iter()
            .any(|r| fringe_of(&r.props, side).is_some())
            || self
                .overlay
                .iter()
                .any(|o| fringe_of(&o.display.0, side).is_some())
    }

    /// `line` laid out for text that is taller than the cell grid: each
    /// character of a `height` run takes as many cells as the text is
    /// wide (a character at 200% is followed by a blank cell), so the
    /// letters of a big run do not overlap. `base` is the text and source
    /// columns to start from, when something has already replaced them;
    /// `base_pct` is the whole buffer's text scale. None when nothing on
    /// the line is taller than normal.
    pub fn tall_line(
        &self,
        line: usize,
        base: Option<(String, Vec<usize>)>,
        base_pct: u16,
    ) -> Option<TallLine> {
        let runs = self.display_style_runs(line, base_pct);
        if !runs.iter().any(|r| r.2.height_pct > 100) {
            return None;
        }
        let (text, src) = base.unwrap_or_else(|| {
            let t = self.line(line);
            let n = t.chars().count();
            (t, (0..n).collect())
        });
        let pct_at = |c: usize| {
            runs.iter()
                .find(|r| r.0 <= c && c < r.1)
                .map_or(100, |r| r.2.height_pct)
        };
        let mut out = String::new();
        let mut out_src = Vec::new();
        let mut shown_runs: Vec<(usize, usize, u16)> = Vec::new();
        let mut col = 0;
        for (ch, &s) in text.chars().zip(src.iter()) {
            let pct = pct_at(s);
            let w = crate::width::line_width(&ch.to_string());
            let advance = if pct > 100 {
                ((pct as f64 / 100.0 * w as f64).round() as usize).max(w)
            } else {
                w
            };
            out.push(ch);
            out_src.push(s);
            for _ in w..advance {
                out.push(' ');
                out_src.push(s);
            }
            if pct != 100 {
                match shown_runs.last_mut() {
                    Some(last) if last.1 == col && last.2 == pct => last.1 = col + advance,
                    _ => shown_runs.push((col, col + advance, pct)),
                }
            }
            col += advance;
        }
        Some(TallLine {
            text: out,
            src: out_src,
            runs: shown_runs,
        })
    }

    /// Is there a `display` property that changes how text is drawn
    /// (`height`, `raise`)?
    pub fn has_display_style(&self) -> bool {
        self.display
            .iter()
            .any(|r| crate::display_spec::changes_style(&r.props))
            || self
                .overlay
                .iter()
                .any(|o| crate::display_spec::changes_style(&o.display.0))
    }

    /// The stretches of `line` drawn with a style of their own: source
    /// column range and the style, from `height` and `raise`. `base_pct`
    /// is the whole buffer's text scale (100 for none): it multiplies each
    /// run's height and is itself the last run, covering the line.
    pub fn display_style_runs(
        &self,
        line: usize,
        base_pct: u16,
    ) -> Vec<(usize, usize, crate::display_spec::TextStyle)> {
        let start = self.pos_to_offset(crate::Pos::new(line, 0));
        let len = self.line(line).chars().count();
        let end = start + len;
        let mut out = Vec::new();
        let mut ranges: Vec<(usize, usize, Vec<crate::buffer::DisplaySpec>)> = Vec::new();
        for r in &self.display {
            ranges.push((r.start, r.end, r.props.clone()));
        }
        for o in &self.overlay {
            if !o.display.0.is_empty() {
                ranges.push((o.start, o.end, o.display.0.clone()));
            }
        }
        for (a, b, props) in ranges {
            if b <= start || a >= end {
                continue;
            }
            let mut style = crate::display_spec::style_of(&props);
            if base_pct != 100 {
                style.height_pct = (u32::from(style.height_pct) * u32::from(base_pct) / 100) as u16;
            }
            if !style.is_plain() {
                out.push((a.max(start) - start, b.min(end) - start, style));
            }
        }
        out.sort_by_key(|r| (r.0, r.1));
        if base_pct != 100 && len > 0 {
            out.push((
                0,
                len,
                crate::display_spec::TextStyle {
                    height_pct: base_pct,
                    raise_pct: 0,
                },
            ));
        }
        out
    }

    /// The `display` text-property and overlay stretches that change the
    /// text of the line from char offset `start` to `end`: (from, to, specs)
    /// in line columns, `from` below zero when one began on an earlier line.
    fn display_text_stretches(
        &self,
        start: usize,
        end: usize,
    ) -> Vec<(isize, usize, Vec<crate::buffer::DisplaySpec>)> {
        let mut found: Vec<(usize, usize, Vec<crate::buffer::DisplaySpec>, bool)> = Vec::new();
        for r in &self.display {
            if crate::display_spec::changes_text(&r.props) {
                found.push((r.start, r.end, r.props.clone(), false));
            }
        }
        for o in &self.overlay {
            if crate::display_spec::changes_text(&o.display.0) {
                found.push((o.start, o.end, o.display.0.clone(), true));
            }
        }
        // An overlay's specifications replace the text property's where they meet.
        found.sort_by_key(|f| (f.0, !f.3, f.1));
        let mut out = Vec::new();
        let mut taken_to = 0usize;
        for (a, b, props, _) in found {
            if b <= start || a >= end || a < taken_to {
                continue;
            }
            taken_to = taken_to.max(b);
            let from = a as isize - start as isize;
            out.push((from, b.min(end) - start, props));
        }
        out
    }

    /// `line` as it is shown with display replacements applied: the text,
    /// and for each shown char the column in the line it comes from (to
    /// look its face up). None if nothing on the line is replaced.
    pub fn display_line(&self, line: usize) -> Option<(String, Vec<usize>)> {
        let start = self.pos_to_offset(crate::Pos::new(line, 0));
        let text = self.line(line);
        let chars: Vec<char> = text.chars().collect();
        let end = start + chars.len();
        let mut spans: Vec<&Decoration> = self
            .decorations
            .iter()
            .filter(|d| d.display.is_some() && d.start >= start && d.end <= end)
            .collect();
        let stretches = self.display_text_stretches(start, end);
        if spans.is_empty() && stretches.is_empty() {
            return None;
        }
        spans.sort_by_key(|d| (d.start, d.end));
        // Every replacement as (from, to, what it shows), in line order.
        // `None` for the text keeps the run's own (a min-width pad).
        enum Show<'a> {
            Text(&'a str),
            Specs(&'a [crate::buffer::DisplaySpec], bool),
        }
        let mut edits: Vec<(usize, usize, Show)> = Vec::new();
        for d in &spans {
            // Any other kind of display hides the text it covers.
            let shown = match &d.display {
                Some(Display::Text(t)) => t.as_str(),
                _ => "",
            };
            edits.push((d.start - start, d.end - start, Show::Text(shown)));
        }
        for (from, to, props) in &stretches {
            // A stretch that began on an earlier line shows nothing here;
            // its text is shown where it began.
            edits.push(((*from).max(0) as usize, *to, Show::Specs(props, *from >= 0)));
        }
        edits.sort_by_key(|e| (e.0, e.1));
        let mut out = String::new();
        let mut src = Vec::new();
        let mut col = 0;
        for (a, b, show) in edits {
            if a < col {
                continue; // overlaps one already placed
            }
            for (i, c) in chars.iter().enumerate().take(a).skip(col) {
                out.push(*c);
                src.push(i);
            }
            let shown: String = match show {
                Show::Text(t) => t.to_string(),
                Show::Specs(props, begins_here) => {
                    if begins_here {
                        let own: String = chars[a..b.min(chars.len())].iter().collect();
                        let at = crate::width::line_width(&out);
                        crate::display_spec::replacement(props, &own, at).unwrap_or(own)
                    } else {
                        String::new()
                    }
                }
            };
            for c in shown.chars() {
                out.push(c);
                src.push(a);
            }
            col = b;
        }
        for (i, c) in chars.iter().enumerate().skip(col) {
            out.push(*c);
            src.push(i);
        }
        Some((out, src))
    }

    /// Take every keymap `owner` has attached off its decorations, leaving
    /// the faces. The other half of `set_decoration_keymap`: a caller that
    /// sets regions as a whole (a plugin's `decorations.set-regions`)
    /// clears the old ones first.
    pub fn clear_decoration_keymaps(&mut self, owner: &str) {
        for d in self.decorations.iter_mut().filter(|d| d.owner == owner) {
            d.keymap = None;
        }
    }

    /// Set a keymap on the stretch `start..end` of `owner`'s decorations,
    /// for text a caller already coloured. A range with no decoration on
    /// it gets a plain one, since the keymap is the point.
    pub fn set_decoration_keymap(&mut self, owner: &str, start: usize, end: usize, keymap: &str) {
        if !self
            .decorations
            .iter()
            .any(|d| d.owner == owner && d.start == start && d.end == end)
        {
            self.set_decorations(owner, vec![DecoSpec::region(start, end, keymap)]);
            return;
        }
        for d in self
            .decorations
            .iter_mut()
            .filter(|d| d.owner == owner && d.start == start && d.end == end)
        {
            d.keymap = Some(keymap.to_string());
        }
    }

    pub fn clear_decorations(&mut self, owner: &str) {
        self.decorations.retain(|d| d.owner != owner);
    }

    pub fn decorations(&self) -> &[Decoration] {
        &self.decorations
    }

    /// The decorations under `offset`, narrowest first: an inner one
    /// (a name inside a listing) before the one containing it.
    pub fn decorations_at(&self, offset: usize) -> Vec<&Decoration> {
        let mut out: Vec<&Decoration> = self
            .decorations
            .iter()
            .filter(|d| d.covers(offset))
            .collect();
        out.sort_by_key(|d| d.end - d.start);
        out
    }

    /// The message of a decoration under `offset`, if any.
    pub fn decoration_message_at(&self, offset: usize) -> Option<&str> {
        self.decorations_at(offset)
            .into_iter()
            .find_map(|d| d.message.as_deref())
    }

    /// The keymaps of the decorations under `offset`, widest span first,
    /// so a key bound in an inner region is searched before the map of the
    /// region around it.
    pub fn decoration_keymaps_at(&self, offset: usize) -> Vec<&str> {
        let mut spans: Vec<(usize, &str)> = self
            .decorations
            .iter()
            .filter(|d| d.covers(offset))
            .filter_map(|d| d.keymap.as_deref().map(|k| (d.end - d.start, k)))
            .collect();
        spans.sort_by_key(|(width, k)| (std::cmp::Reverse(*width), *k));
        let mut seen = std::collections::HashSet::new();
        spans.retain(|(_, k)| seen.insert(*k));
        spans.into_iter().map(|(_, k)| k).collect()
    }

    // ------------------------------------------------------------ options

    /// Set a buffer option by name (see [`decoration::OPTIONS`]).
    /// An empty value goes back to the default.
    pub fn set_option(&mut self, name: &str, value: &str) -> Result<(), String> {
        let unset = value.is_empty();
        match name {
            "line-numbers" => {
                self.options.line_numbers = if unset {
                    None
                } else {
                    Some(LineNumbers::from_name(value)?)
                }
            }
            "indent-width" => {
                self.options.indent_width = if unset {
                    None
                } else {
                    Some(decoration::parse_number(name, value, 1..=16)?)
                }
            }
            "indent-tabs" => {
                self.options.indent_tabs = if unset {
                    None
                } else {
                    Some(decoration::parse_bool(name, value)?)
                }
            }
            "fill-column" => {
                self.fill_column = if unset {
                    crate::buffer::DEFAULT_FILL_COLUMN
                } else {
                    decoration::parse_number(name, value, 1..=10_000)?
                }
            }
            "wrap" => {
                self.options.wrap = if unset {
                    None
                } else {
                    Some(Wrap::from_name(value)?)
                }
            }
            "cursor" => {
                self.options.cursor = if unset {
                    None
                } else {
                    Some(decoration::CursorShape::from_name(value)?)
                }
            }
            "read-only" => {
                let yes = !unset && decoration::parse_bool(name, value)?;
                self.set_locked(yes);
            }
            "hl-line"
            | "fill-column-indicator"
            | "display-fill-column-indicator"
            | "show-trailing-whitespace"
            | "whitespace"
            | "trim-trailing-whitespace"
            | "insert-final-newline"
            | "electric-quote" => {
                let v = if unset {
                    None
                } else {
                    Some(decoration::parse_bool(name, value)?)
                };
                *self.bool_option(name).expect("listed") = v;
            }
            "display-fill-column-indicator-column" => {
                self.options.fill_column_indicator_column = if unset {
                    None
                } else {
                    Some(decoration::FillColumnColumn::from_value(value)?)
                }
            }
            "display-fill-column-indicator-character" => {
                self.options.fill_column_indicator_character = if unset {
                    None
                } else {
                    Some(decoration::FillColumnChar::from_value(value)?)
                }
            }
            _ => {
                return Err(format!(
                    "unknown buffer option {name:?} (known: {})",
                    decoration::OPTIONS.join(", ")
                ));
            }
        }
        Ok(())
    }

    fn bool_option(&mut self, name: &str) -> Option<&mut Option<bool>> {
        let o = &mut self.options;
        Some(match name {
            "hl-line" => &mut o.hl_line,
            "fill-column-indicator" | "display-fill-column-indicator" => {
                &mut o.fill_column_indicator
            }
            "show-trailing-whitespace" => &mut o.show_trailing_whitespace,
            "whitespace" => &mut o.whitespace,
            "trim-trailing-whitespace" => &mut o.trim_trailing_whitespace,
            "insert-final-newline" => &mut o.insert_final_newline,
            "electric-quote" => &mut o.electric_quote,
            _ => return None,
        })
    }

    /// A buffer option's value (None: unknown, or not set).
    pub fn option(&self, name: &str) -> Option<String> {
        match name {
            "line-numbers" => self.options.line_numbers.map(|l| l.name().to_string()),
            "indent-width" => self.indent_style().map(|s| s.width.to_string()),
            "indent-tabs" => self.indent_style().map(|s| s.tabs.to_string()),
            "fill-column" => Some(self.fill_column.to_string()),
            "wrap" => self.options.wrap.map(|w| w.name().to_string()),
            "cursor" => self.options.cursor.map(|c| c.name().to_string()),
            "hl-line" => self.options.hl_line.map(|v| v.to_string()),
            "fill-column-indicator" | "display-fill-column-indicator" => {
                self.options.fill_column_indicator.map(|v| v.to_string())
            }
            "display-fill-column-indicator-column" => self
                .options
                .fill_column_indicator_column
                .map(decoration::FillColumnColumn::value),
            "display-fill-column-indicator-character" => self
                .options
                .fill_column_indicator_character
                .map(decoration::FillColumnChar::value),
            "show-trailing-whitespace" => {
                self.options.show_trailing_whitespace.map(|v| v.to_string())
            }
            "electric-quote" => self.options.electric_quote.map(|v| v.to_string()),
            "whitespace" => self.options.whitespace.map(|v| v.to_string()),
            "trim-trailing-whitespace" => {
                self.options.trim_trailing_whitespace.map(|v| v.to_string())
            }
            "insert-final-newline" => self.options.insert_final_newline.map(|v| v.to_string()),
            "read-only" => self.locked().then(|| "true".to_string()),
            _ => None,
        }
    }

    // ------------------------------------------- the display text property

    /// The `display` text property's specification named `name` at char
    /// offset `pos`, if any. An overlay
    /// wins over the buffer's text property.
    pub fn display_prop(&self, pos: usize, name: &str) -> Option<&crate::buffer::DisplaySpec> {
        // An overlay's display property replaces the buffer's at the same
        // position, so if one is there it is the only value.
        if let Some(o) = self
            .overlay
            .iter()
            .find(|o| pos >= o.start && pos < o.end && !o.display.0.is_empty())
        {
            return o.display.0.iter().find(|s| s.name == name);
        }
        self.display
            .iter()
            .find(|r| pos >= r.start && pos < r.end)
            .and_then(|r| r.props.iter().find(|s| s.name == name))
    }

    /// Does the buffer have a `display` property (text or overlay) that
    /// inserts an image?
    pub fn has_display_image(&self) -> bool {
        self.display
            .iter()
            .any(|r| r.props.iter().any(|s| s.name == "image"))
            || self
                .overlay
                .iter()
                .any(|o| o.display.0.iter().any(|s| s.name == "image"))
    }

    /// The whole `display` property at char offset `pos` (overlay first).
    /// An overlay with no `display` specification of its own lets the
    /// buffer's text property show through, as in [`Buffer::display_prop`].
    pub fn display_props(&self, pos: usize) -> crate::buffer::DisplayProps {
        if let Some(o) = self
            .overlay
            .iter()
            .find(|o| pos >= o.start && pos < o.end && !o.display.0.is_empty())
        {
            return o.display.clone();
        }
        let mut out = Vec::new();
        for r in &self.display {
            if pos >= r.start && pos < r.end {
                out.extend(r.props.iter().cloned());
            }
        }
        crate::buffer::DisplayProps(out)
    }

    /// Add a display specification to `start..end`, splitting the ranges so
    /// every character in it carries the spec, and keeping any other
    /// specifications already there.
    pub fn add_display_prop(&mut self, start: usize, end: usize, spec: crate::buffer::DisplaySpec) {
        if start >= end {
            return;
        }
        split_display_at(&mut self.display, start);
        split_display_at(&mut self.display, end);
        // Fill gaps so the spec reaches every character in the range.
        let mut at = start;
        let mut i = 0;
        while at < end {
            if i < self.display.len() && self.display[i].start <= at {
                if self.display[i].end > at {
                    at = self.display[i].end;
                }
                i += 1;
            } else {
                let gap_end = self.display.get(i).map_or(end, |r| r.start.min(end));
                self.display.insert(
                    i,
                    crate::buffer::DisplayRange {
                        start: at,
                        end: gap_end,
                        props: Vec::new(),
                    },
                );
                at = gap_end;
                i += 1;
            }
        }
        for r in self.display.iter_mut() {
            if r.start >= start && r.end <= end {
                if let Some(existing) = r.props.iter_mut().find(|s| s.name == spec.name) {
                    *existing = spec.clone();
                } else {
                    r.props.push(spec.clone());
                }
            }
        }
        normalize_display(&mut self.display);
    }

    /// Remove the display specification `name` from `start..end`, keeping
    /// the others.
    pub fn remove_display_prop(&mut self, start: usize, end: usize, name: &str) {
        if start >= end {
            return;
        }
        split_display_at(&mut self.display, start);
        split_display_at(&mut self.display, end);
        for r in self.display.iter_mut() {
            if r.end <= start || r.start >= end {
                continue;
            }
            r.props.retain(|s| s.name != name);
        }
        normalize_display(&mut self.display);
    }

    /// Add an overlay with a `display` property over `start..end`, and
    /// return its id.
    pub fn add_overlay(
        &mut self,
        start: usize,
        end: usize,
        display: crate::buffer::DisplayProps,
    ) -> usize {
        if start >= end {
            return 0;
        }
        let id = self.next_overlay;
        self.next_overlay += 1;
        self.overlay.push(crate::buffer::Overlay {
            id,
            start,
            end,
            display,
        });
        id
    }

    /// Remove the overlay with `id`.
    pub fn remove_overlay(&mut self, id: usize) {
        self.overlay.retain(|o| o.id != id);
    }
}

/// Split every display range containing `at` into two.
fn split_display_at(display: &mut Vec<crate::buffer::DisplayRange>, at: usize) {
    let mut out = Vec::with_capacity(display.len() + 1);
    for r in std::mem::take(display) {
        if at > r.start && at < r.end {
            out.push(crate::buffer::DisplayRange {
                start: r.start,
                end: at,
                props: r.props.clone(),
            });
            out.push(crate::buffer::DisplayRange {
                start: at,
                end: r.end,
                props: r.props,
            });
        } else {
            out.push(r);
        }
    }
    *display = out;
}

/// Merge adjacent ranges with equal properties and drop empty ones.
fn normalize_display(display: &mut Vec<crate::buffer::DisplayRange>) {
    display.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<crate::buffer::DisplayRange> = Vec::new();
    for r in std::mem::take(display) {
        if let Some(last) = out.last_mut()
            && last.end == r.start
            && last.props == r.props
        {
            last.end = r.end;
            continue;
        }
        if !r.props.is_empty() {
            out.push(r);
        }
    }
    *display = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;
    use crate::decoration::DecoFace;

    fn rust(text: &str) -> Buffer {
        let mut b = Buffer::new("x.rs");
        b.set_contents(text);
        b.set_language(syntax::language("rust"));
        b
    }

    #[test]
    fn a_display_property_reaches_a_range_after_an_earlier_one() {
        let mut b = Buffer::new("x");
        b.set_contents("0123456789");
        let height = crate::buffer::DisplaySpec {
            name: "height".to_string(),
            args: vec![crate::buffer::DisplayValue::Real(2.0)],
        };
        let raise = crate::buffer::DisplaySpec {
            name: "raise".to_string(),
            args: vec![crate::buffer::DisplayValue::Real(0.5)],
        };
        b.add_display_prop(0, 5, height.clone());
        b.add_display_prop(8, 10, raise.clone());
        assert_eq!(b.display_prop(9, "raise"), Some(&raise));
        assert_eq!(b.display_prop(9, "height"), None);
        assert_eq!(b.display_prop(3, "height"), Some(&height));
    }

    #[test]
    fn display_property_specs_merge_per_character() {
        let mut b = Buffer::new("x");
        b.set_contents("0123456789");
        let height = crate::buffer::DisplaySpec {
            name: "height".to_string(),
            args: vec![crate::buffer::DisplayValue::Real(2.0)],
        };
        let raise = crate::buffer::DisplaySpec {
            name: "raise".to_string(),
            args: vec![crate::buffer::DisplayValue::Real(0.5)],
        };
        b.add_display_prop(4, 8, height.clone());
        b.add_display_prop(2, 10, raise.clone());
        // [2,4) raise, [4,8) raise+height, [8,10) raise.
        assert_eq!(b.display_prop(3, "raise"), Some(&raise));
        assert_eq!(b.display_prop(3, "height"), None);
        assert_eq!(b.display_prop(5, "raise"), Some(&raise));
        assert_eq!(b.display_prop(5, "height"), Some(&height));
        assert_eq!(b.display_prop(9, "raise"), Some(&raise));
        assert_eq!(b.display_prop(9, "height"), None);

        // Remove raise from [2,6): [2,4) has nothing, [4,6) keeps height.
        b.remove_display_prop(2, 6, "raise");
        assert_eq!(b.display_prop(3, "raise"), None);
        assert_eq!(b.display_prop(5, "raise"), None);
        assert_eq!(b.display_prop(5, "height"), Some(&height));
        assert_eq!(b.display_prop(9, "raise"), Some(&raise));
    }

    #[test]
    fn an_overlay_wins_over_the_text_property() {
        let mut b = Buffer::new("x");
        b.set_contents("hello world");
        b.add_display_prop(
            0,
            5,
            crate::buffer::DisplaySpec {
                name: "height".to_string(),
                args: vec![crate::buffer::DisplayValue::Real(2.0)],
            },
        );
        let id = b.add_overlay(
            0,
            5,
            crate::buffer::DisplayProps(vec![crate::buffer::DisplaySpec {
                name: "raise".to_string(),
                args: vec![crate::buffer::DisplayValue::Real(0.5)],
            }]),
        );
        // The overlay's display property replaces the text property.
        assert_eq!(
            b.display_prop(0, "raise").map(|s| s.name.as_str()),
            Some("raise")
        );
        assert_eq!(b.display_prop(0, "height"), None);
        // Remove it and the text property is back.
        b.remove_overlay(id);
        assert_eq!(
            b.display_prop(0, "height").map(|s| s.name.as_str()),
            Some("height")
        );
    }

    #[test]
    fn an_image_descriptor_carries_its_properties() {
        let mut b = Buffer::new("x");
        b.set_contents("a\n[img]\nb");
        let spec = crate::buffer::DisplaySpec {
            name: "image".to_string(),
            args: vec![
                crate::buffer::DisplayValue::Symbol("file".to_string()),
                crate::buffer::DisplayValue::Str("/tmp/a.png".to_string()),
                crate::buffer::DisplayValue::Symbol("width".to_string()),
                crate::buffer::DisplayValue::Int(40),
                crate::buffer::DisplayValue::Symbol("scale".to_string()),
                crate::buffer::DisplayValue::Real(1.5),
                crate::buffer::DisplayValue::Symbol("rotation".to_string()),
                crate::buffer::DisplayValue::Int(90),
                crate::buffer::DisplayValue::Symbol("flip".to_string()),
                crate::buffer::DisplayValue::Bool(true),
            ],
        };
        b.add_display_prop(2, 6, spec);
        let d = b.display_image_descriptor(1).unwrap();
        assert_eq!(d.path, std::path::PathBuf::from("/tmp/a.png"));
        assert_eq!(d.width, Some(40.0));
        assert_eq!(d.scale, Some(1.5));
        assert_eq!(d.rotation, 90);
        assert!(d.flip_h);
        // :width 40 on a 100x50 picture keeps the aspect (40x20), then
        // :scale 1.5 doubles it to 60x30.
        assert_eq!(d.target_size((100, 50)), (60, 30));
    }

    #[test]
    fn a_display_image_is_an_inline_image() {
        let mut b = Buffer::new("x");
        b.set_contents("before\n[image here]\nafter");
        let spec = crate::buffer::DisplaySpec {
            name: "image".to_string(),
            args: vec![
                crate::buffer::DisplayValue::Symbol("file".to_string()),
                crate::buffer::DisplayValue::Str("/tmp/x.png".to_string()),
            ],
        };
        b.add_display_prop(8, 13, spec);
        assert!(b.has_display_image());
        assert_eq!(
            b.display_image(1),
            Some(std::path::PathBuf::from("/tmp/x.png"))
        );
    }

    #[test]
    fn display_properties_add_get_and_remove() {
        let mut b = Buffer::new("x");
        b.set_contents("hello world");
        let height = crate::buffer::DisplaySpec {
            name: "height".to_string(),
            args: vec![crate::buffer::DisplayValue::Real(2.0)],
        };
        b.add_display_prop(0, 5, height.clone());
        assert_eq!(b.display_prop(0, "height"), Some(&height));
        assert_eq!(b.display_prop(5, "height"), None, "the range is [0, 5)");
        // Adding another spec keeps the first.
        let raise = crate::buffer::DisplaySpec {
            name: "raise".to_string(),
            args: vec![crate::buffer::DisplayValue::Real(0.5)],
        };
        b.add_display_prop(0, 5, raise.clone());
        assert_eq!(b.display_prop(0, "height"), Some(&height));
        assert_eq!(b.display_prop(0, "raise"), Some(&raise));
        b.remove_display_prop(0, 5, "height");
        assert_eq!(b.display_prop(0, "height"), None);
        assert_eq!(b.display_prop(0, "raise"), Some(&raise));
    }

    #[test]
    fn highlighting_follows_edits() {
        let mut b = rust("fn a() {}\n");
        assert_eq!(b.syntax_faces(0..2), [Some(SyntaxFace::Keyword); 2]);
        b.set_point(Pos::new(0, 0));
        b.insert_str("// ");
        assert_eq!(b.syntax_faces(0..5), [Some(SyntaxFace::Comment); 5]);
        b.undo();
        assert_eq!(b.syntax_faces(0..2), [Some(SyntaxFace::Keyword); 2]);
        // A copy rebuilds its own tree.
        let mut c = b.clone();
        assert_eq!(c.syntax_faces(0..2), [Some(SyntaxFace::Keyword); 2]);
        b.set_language(None);
        assert_eq!(b.syntax_faces(0..2), [None, None]);
    }

    #[test]
    fn indent_line_keeps_point_on_its_text() {
        let mut b = rust("fn f() {\nx();\n}\n");
        b.set_point(Pos::new(1, 2));
        assert!(b.indent_line(1));
        assert_eq!(b.line(1), "    x();");
        assert_eq!(b.point(), Pos::new(1, 6));
        b.set_point(Pos::new(1, 1));
        b.indent_line(1);
        assert_eq!(b.point(), Pos::new(1, 4));
        b.set_option("indent-width", "2").unwrap();
        b.indent_line(1);
        assert_eq!(b.line(1), "  x();");
        b.set_option("indent-tabs", "true").unwrap();
        b.set_option("indent-width", "8").unwrap();
        b.indent_line(1);
        assert_eq!(b.line(1), "\tx();");
        let mut t = Buffer::new("t");
        t.set_contents("  a\nb\n");
        assert!(!t.indent_line(1));
    }

    #[test]
    fn decorations_and_options() {
        let mut b = Buffer::new("t");
        b.set_contents("hello wrold\n");
        b.set_decorations(
            "spell",
            vec![
                (6, 11, DecoFace::SpellError, Some("wrold?".into())),
                (3, 3, DecoFace::Info, None),
            ],
        );
        assert_eq!(b.decorations().len(), 1, "empty ones are skipped");
        assert_eq!(b.decoration_message_at(7), Some("wrold?"));
        assert_eq!(b.decoration_message_at(5), None);
        b.set_point(Pos::new(0, 0));
        b.insert_str(">> ");
        assert_eq!((b.decorations()[0].start, b.decorations()[0].end), (9, 14));
        b.clear_decorations("other");
        assert_eq!(b.decorations().len(), 1);
        b.clear_decorations("spell");
        assert!(b.decorations().is_empty());

        b.set_option("line-numbers", "relative").unwrap();
        assert_eq!(b.option("line-numbers").as_deref(), Some("relative"));
        b.set_option("fill-column", "72").unwrap();
        assert_eq!(b.fill_column, 72);
        assert!(b.set_option("line-numbers", "sometimes").is_err());
        assert!(
            b.set_option("tabs", "1")
                .unwrap_err()
                .contains("known: cursor")
        );
        b.set_option("line-numbers", "").unwrap();
        assert_eq!(b.option("line-numbers"), None);

        // The fill column line: variable names, with the old
        // `fill-column-indicator` still accepted.
        b.set_option("display-fill-column-indicator", "true")
            .unwrap();
        assert_eq!(
            b.option("display-fill-column-indicator").as_deref(),
            Some("true")
        );
        assert_eq!(b.option("fill-column-indicator").as_deref(), Some("true"));
        b.set_option("display-fill-column-indicator-column", "72")
            .unwrap();
        assert_eq!(
            b.option("display-fill-column-indicator-column").as_deref(),
            Some("72")
        );
        b.set_option("display-fill-column-indicator-column", "t")
            .unwrap();
        assert_eq!(
            b.option("display-fill-column-indicator-column").as_deref(),
            Some("t")
        );
        b.set_option("display-fill-column-indicator-character", "|")
            .unwrap();
        assert_eq!(
            b.option("display-fill-column-indicator-character")
                .as_deref(),
            Some("|")
        );
        assert!(
            b.set_option("display-fill-column-indicator-character", "ab")
                .is_err()
        );
    }

    #[test]
    fn a_region_keymap_rides_on_a_decoration() {
        let mut b = Buffer::new("t");
        b.set_contents("hello wrold\n");
        // A click on the text resolves in the map the decoration names.
        b.set_decoration_keymap("dired", 0, 5, "dired-entry");
        assert_eq!(b.decoration_keymaps_at(2), ["dired-entry"]);
        assert!(
            b.decoration_keymaps_at(7).is_empty(),
            "the rest of the line is not part of the link"
        );
        // A decoration already there just gains the keymap, keeping its
        // face: one call colours the text, another makes it clickable.
        b.set_decorations("lint", vec![DecoSpec::region(6, 11, "spell-word")]);
        b.set_decorations(
            "lint",
            vec![DecoSpec::new(6, 11, DecoFace::SpellError).keymap("spell-word")],
        );
        assert_eq!(b.decoration_keymaps_at(7), ["spell-word"]);
        assert_eq!(b.decorations().len(), 2, "one per owner");
        // The text moves, and so does the map with it.
        b.set_point(Pos::new(0, 0));
        b.insert_str(">> ");
        assert_eq!(b.decoration_keymaps_at(10), ["spell-word"]);
    }
}

/// A line laid out for text taller than a cell (see [`Buffer::tall_line`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TallLine {
    /// The text with a blank cell after each character that is wider than one.
    pub text: String,
    /// For each character of `text`, the source column it stands for.
    pub src: Vec<usize>,
    /// Display columns (start, end) of the stretches that are not normal
    /// size, with their height in percent.
    pub runs: Vec<(usize, usize, u16)>,
}

/// The text of a `margin` specification for `side` among `props`.
fn margin_of<'a>(props: &'a [crate::buffer::DisplaySpec], side: &str) -> Option<&'a str> {
    use crate::buffer::DisplayValue::{Str, Symbol};
    props
        .iter()
        .find_map(|s| match (s.name.as_str(), s.args.first(), s.args.get(1)) {
            ("margin", Some(Str(which) | Symbol(which)), Some(Str(text))) if which == side => {
                Some(text.as_str())
            }
            _ => None,
        })
}

/// The glyph of a `left-fringe` or `right-fringe` specification for `side`.
fn fringe_of(props: &[crate::buffer::DisplaySpec], side: &str) -> Option<char> {
    use crate::buffer::DisplayValue::{Str, Symbol};
    props
        .iter()
        .find_map(|s| match (s.name.as_str(), s.args.first()) {
            (name, Some(Str(bitmap) | Symbol(bitmap))) if name == side => {
                Some(fringe_glyph(bitmap))
            }
            _ => None,
        })
}

/// The character standing in for one of fringe bitmaps. A name that
/// is a single character is used as it is.
pub fn fringe_glyph(bitmap: &str) -> char {
    match bitmap {
        "left-arrow" => '←',
        "right-arrow" => '→',
        "up-arrow" => '↑',
        "down-arrow" => '↓',
        "left-curly-arrow" => '↶',
        "right-curly-arrow" => '↷',
        "left-triangle" => '◀',
        "right-triangle" => '▶',
        "up-triangle" => '▲',
        "down-triangle" => '▼',
        "question-mark" => '?',
        "exclamation-mark" => '!',
        "filled-rectangle" | "filled-square" => '█',
        "hollow-rectangle" | "hollow-square" => '□',
        "empty-line" => '·',
        "vertical-bar" => '│',
        "horizontal-bar" => '─',
        "top-left-angle" => '┌',
        "top-right-angle" => '┐',
        "bottom-left-angle" => '└',
        "bottom-right-angle" => '┘',
        "left-bracket" => '[',
        "right-bracket" => ']',
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => '•',
            }
        }
    }
}
