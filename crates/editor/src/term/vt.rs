// SPDX-License-Identifier: MIT OR Apache-2.0
//! The terminal emulator behind `term`: an xterm-compatible screen fed
//! with the bytes a program writes. It keeps the visible grid, a
//! scrollback, the alternate screen full-screen programs use, 16/256/true
//! colours and the attributes (bold, italic, underline, reverse...), and
//! the replies programs wait for (cursor position, device attributes).
//! OSC 0/2 set the title, OSC 7 reports the directory the shell is in,
//! and OSC 133 marks prompts (the shell integration in `term`'s docs).

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Rgb(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u8,
}

impl Cell {
    pub const BLANK: Cell = Cell {
        ch: ' ',
        fg: Color::Default,
        bg: Color::Default,
        attrs: 0,
    };

    /// The right half of a double-width character.
    pub const CONT: char = '\0';

    pub fn style(&self) -> (Color, Color, u8) {
        (self.fg, self.bg, self.attrs)
    }
}

pub const BOLD: u8 = 1;
pub const DIM: u8 = 2;
pub const ITALIC: u8 = 4;
pub const UNDERLINE: u8 = 8;
pub const REVERSE: u8 = 16;
pub const STRIKE: u8 = 32;

/// The xterm palette: the 16 named colours, the 6x6x6 cube, the greys.
pub fn palette(i: u8) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
        0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let n = u32::from(i - 16);
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36) << 16) | (level(n / 6 % 6) << 8) | level(n % 6)
        }
        _ => {
            let g = 8 + 10 * u32::from(i - 232);
            (g << 16) | (g << 8) | g
        }
    }
}

#[derive(Clone)]
struct Pen {
    fg: Color,
    bg: Color,
    attrs: u8,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            fg: Color::Default,
            bg: Color::Default,
            attrs: 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    /// ESC followed by an intermediate (`ESC ( B`): swallow one more byte.
    EscSkip,
    Csi,
    Osc,
    OscEsc,
    /// DCS, APC, PM, SOS: ignored up to ST.
    Str,
    StrEsc,
}

/// One screen of the terminal (the main one, or the alternate).
#[derive(Clone)]
struct Grid {
    cells: Vec<Vec<Cell>>,
}

impl Grid {
    fn new(rows: usize, cols: usize) -> Self {
        Grid {
            cells: vec![vec![Cell::BLANK; cols]; rows],
        }
    }
}

pub struct Vt {
    pub rows: usize,
    pub cols: usize,
    main: Grid,
    alt: Grid,
    on_alt: bool,
    pub scrollback: Vec<Vec<Cell>>,
    pub scrollback_limit: usize,
    pub row: usize,
    pub col: usize,
    saved: (usize, usize, Pen),
    pen: Pen,
    top: usize,
    bottom: usize,
    wrap_pending: bool,
    tabs: Vec<bool>,
    state: State,
    params: Vec<u8>,
    osc: Vec<u8>,
    utf8: Vec<u8>,
    pub title: String,
    pub cwd: Option<String>,
    /// Set when OSC 133;A (prompt start) arrived: the line it was on.
    pub prompt_rows: Vec<usize>,
    pub cursor_visible: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub autowrap: bool,
    pub mouse: bool,
    origin: bool,
    insert: bool,
    /// Bytes to write back to the program (answers to queries).
    pub reply: Vec<u8>,
    /// Anything changed since the last [`take_dirty`](Self::take_dirty).
    dirty: bool,
    /// Rows scrolled off the top so far (prompt rows track against it).
    scrolled_total: usize,
}

impl Vt {
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Vt {
            rows,
            cols,
            main: Grid::new(rows, cols),
            alt: Grid::new(rows, cols),
            on_alt: false,
            scrollback: Vec::new(),
            scrollback_limit: 5000,
            row: 0,
            col: 0,
            saved: (0, 0, Pen::default()),
            pen: Pen::default(),
            top: 0,
            bottom: rows - 1,
            wrap_pending: false,
            tabs: (0..cols).map(|c| c % 8 == 0).collect(),
            state: State::Ground,
            params: Vec::new(),
            osc: Vec::new(),
            utf8: Vec::new(),
            title: String::new(),
            cwd: None,
            prompt_rows: Vec::new(),
            cursor_visible: true,
            app_cursor: false,
            bracketed_paste: false,
            autowrap: true,
            mouse: false,
            origin: false,
            insert: false,
            reply: Vec::new(),
            dirty: true,
            scrolled_total: 0,
        }
    }

    pub fn on_alt_screen(&self) -> bool {
        self.on_alt
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn g(&mut self) -> &mut Vec<Vec<Cell>> {
        if self.on_alt {
            &mut self.alt.cells
        } else {
            &mut self.main.cells
        }
    }

    pub fn grid(&self) -> &Vec<Vec<Cell>> {
        if self.on_alt {
            &self.alt.cells
        } else {
            &self.main.cells
        }
    }

    /// Everything to show: scrollback (not on the alternate screen), then
    /// the screen. Trailing blanks are the caller's to trim.
    pub fn all_lines(&self) -> impl Iterator<Item = &Vec<Cell>> {
        let sb: &[Vec<Cell>] = if self.on_alt { &[] } else { &self.scrollback };
        sb.iter().chain(self.grid().iter())
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        for grid in [&mut self.main, &mut self.alt] {
            for line in &mut grid.cells {
                line.resize(cols, Cell::BLANK);
            }
        }
        // Shrinking: lines pushed off the top of the main screen go to the
        // scrollback, keeping the cursor's line on screen.
        while self.main.cells.len() > rows {
            if self.row > 0 && !self.on_alt {
                let l = self.main.cells.remove(0);
                self.scrollback.push(l);
                self.row -= 1;
            } else {
                self.main.cells.pop();
            }
        }
        while self.alt.cells.len() > rows {
            self.alt.cells.pop();
        }
        for grid in [&mut self.main, &mut self.alt] {
            while grid.cells.len() < rows {
                grid.cells.push(vec![Cell::BLANK; cols]);
            }
        }
        self.rows = rows;
        self.cols = cols;
        self.top = 0;
        self.bottom = rows - 1;
        self.row = self.row.min(rows - 1);
        self.col = self.col.min(cols - 1);
        self.wrap_pending = false;
        self.tabs = (0..cols).map(|c| c % 8 == 0).collect();
        self.trim_scrollback();
        self.dirty = true;
    }

    fn trim_scrollback(&mut self) {
        if self.scrollback.len() > self.scrollback_limit {
            let extra = self.scrollback.len() - self.scrollback_limit;
            self.scrollback.drain(..extra);
        }
    }

    // ------------------------------------------------------------ input

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
        self.dirty = true;
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Ground => self.ground(b),
            State::Esc => self.esc(b),
            State::EscSkip => self.state = State::Ground,
            State::Csi => self.csi_byte(b),
            State::Osc => match b {
                0x07 => self.finish_osc(),
                0x1b => self.state = State::OscEsc,
                0x18 | 0x1a => self.state = State::Ground,
                _ => self.osc.push(b),
            },
            State::OscEsc => {
                // ESC \ is ST; anything else abandons the string.
                if b == b'\\' {
                    self.finish_osc();
                } else {
                    self.state = State::Ground;
                    self.esc(b);
                }
            }
            State::Str => match b {
                0x1b => self.state = State::StrEsc,
                0x07 | 0x18 | 0x1a => self.state = State::Ground,
                _ => {}
            },
            State::StrEsc => self.state = State::Ground,
        }
    }

    fn ground(&mut self, b: u8) {
        if !self.utf8.is_empty() || b >= 0x80 {
            if b < 0x80 {
                self.utf8.clear();
            } else if b >= 0xc0 {
                self.utf8.clear();
                self.utf8.push(b);
                return self.try_utf8();
            } else if !self.utf8.is_empty() {
                self.utf8.push(b);
                return self.try_utf8();
            } else {
                return; // a stray continuation byte
            }
        }
        match b {
            0x07 => {}
            0x08 => {
                self.wrap_pending = false;
                self.col = self.col.saturating_sub(1);
            }
            0x09 => {
                self.wrap_pending = false;
                let mut c = self.col + 1;
                while c < self.cols - 1 && !self.tabs[c] {
                    c += 1;
                }
                self.col = c.min(self.cols - 1);
            }
            0x0a..=0x0c => self.linefeed(),
            0x0d => {
                self.wrap_pending = false;
                self.col = 0;
            }
            0x1b => self.state = State::Esc,
            0x20..=0x7e => self.put(b as char),
            _ => {}
        }
    }

    fn try_utf8(&mut self) {
        let need = match self.utf8[0] {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        if self.utf8.len() < need {
            return;
        }
        let s = std::str::from_utf8(&self.utf8).map(|s| s.chars().next());
        self.utf8.clear();
        if let Ok(Some(c)) = s {
            self.put(c);
        }
    }

    fn put(&mut self, c: char) {
        let w = c.width().unwrap_or(0);
        if w == 0 {
            // A combining mark: it goes on the cell before the cursor. A
            // cell holds one char, so it is dropped.
            return;
        }
        if self.wrap_pending {
            if self.autowrap {
                self.col = 0;
                self.linefeed();
            }
            self.wrap_pending = false;
        }
        if w == 2 && self.col + 1 >= self.cols {
            if self.autowrap {
                self.col = 0;
                self.linefeed();
            } else {
                return;
            }
        }
        let cols = self.cols;
        let (row, col) = (self.row, self.col);
        let cell = Cell {
            ch: c,
            fg: self.pen.fg,
            bg: self.pen.bg,
            attrs: self.pen.attrs,
        };
        let insert = self.insert;
        let line = &mut self.g()[row];
        if insert {
            for _ in 0..w {
                line.pop();
                line.insert(col, Cell::BLANK);
            }
            line.truncate(cols);
        }
        line[col] = cell;
        if w == 2 {
            line[col + 1] = Cell {
                ch: Cell::CONT,
                ..cell
            };
        }
        if col + w >= cols {
            self.col = cols - 1;
            self.wrap_pending = true;
        } else {
            self.col = col + w;
        }
    }

    fn linefeed(&mut self) {
        if self.row == self.bottom {
            self.scroll_up(1);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    fn blank(&self) -> Cell {
        // Erased cells take the current background (BCE).
        Cell {
            bg: self.pen.bg,
            ..Cell::BLANK
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.top, self.bottom);
        let blank = vec![self.blank(); self.cols];
        for _ in 0..n.min(bottom - top + 1) {
            let gone = self.g().remove(top);
            self.g().insert(bottom, blank.clone());
            if !self.on_alt && top == 0 {
                self.scrollback.push(gone);
                self.scrolled_total += 1;
            }
        }
        self.trim_scrollback();
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bottom) = (self.top, self.bottom);
        let blank = vec![self.blank(); self.cols];
        for _ in 0..n.min(bottom - top + 1) {
            self.g().remove(bottom);
            self.g().insert(top, blank.clone());
        }
    }

    fn esc(&mut self, b: u8) {
        self.state = State::Ground;
        match b {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
            }
            b']' => {
                self.state = State::Osc;
                self.osc.clear();
            }
            b'P' | b'X' | b'^' | b'_' => self.state = State::Str,
            b'(' | b')' | b'*' | b'+' | b'#' | b'%' | b' ' => self.state = State::EscSkip,
            b'7' => self.saved = (self.row, self.col, self.pen.clone()),
            b'8' => {
                let (r, c, p) = self.saved.clone();
                self.row = r.min(self.rows - 1);
                self.col = c.min(self.cols - 1);
                self.pen = p;
                self.wrap_pending = false;
            }
            b'D' => self.linefeed(),
            b'E' => {
                self.col = 0;
                self.linefeed();
            }
            b'M' => {
                if self.row == self.top {
                    self.scroll_down(1);
                } else {
                    self.row = self.row.saturating_sub(1);
                }
            }
            b'H' => self.tabs[self.col] = true,
            b'c' => {
                let (rows, cols) = (self.rows, self.cols);
                let limit = self.scrollback_limit;
                *self = Vt::new(rows, cols);
                self.scrollback_limit = limit;
            }
            _ => {}
        }
    }

    fn csi_byte(&mut self, b: u8) {
        match b {
            0x20..=0x3f => self.params.push(b),
            0x40..=0x7e => {
                self.state = State::Ground;
                let params = std::mem::take(&mut self.params);
                self.csi(b, &params);
            }
            0x1b => self.state = State::Esc,
            0x18 | 0x1a => self.state = State::Ground,
            _ => {}
        }
    }

    fn finish_osc(&mut self) {
        self.state = State::Ground;
        let s = String::from_utf8_lossy(&std::mem::take(&mut self.osc)).into_owned();
        let (code, rest) = s.split_once(';').unwrap_or((&s, ""));
        match code {
            "0" | "2" => self.title = rest.to_string(),
            "7" => {
                // file://host/path
                let path = rest
                    .strip_prefix("file://")
                    .map(|r| r.find('/').map_or("/", |i| &r[i..]))
                    .unwrap_or(rest);
                self.cwd = Some(percent_decode(path));
            }
            "133" if rest.starts_with('A') => {
                let at = self.scrollback.len() + self.row;
                self.prompt_rows.push(at);
            }
            "133" => {}
            _ => {}
        }
    }

    fn csi(&mut self, fin: u8, raw: &[u8]) {
        let private = raw
            .first()
            .is_some_and(|b| matches!(b, b'?' | b'>' | b'<' | b'='));
        let lead = if private { raw[0] } else { 0 };
        let body = if private { &raw[1..] } else { raw };
        let inter = body.iter().any(|b| (0x20..=0x2f).contains(b));
        let nums: Vec<Option<usize>> = std::str::from_utf8(
            &body
                .iter()
                .copied()
                .filter(|b| !(0x20..=0x2f).contains(b))
                .collect::<Vec<_>>(),
        )
        .unwrap_or("")
        .split([';', ':'])
        .map(|p| p.parse::<usize>().ok())
        .collect();
        let p = |i: usize, d: usize| {
            nums.get(i)
                .copied()
                .flatten()
                .filter(|&v| v != 0)
                .unwrap_or(d)
        };
        let p0 = |i: usize| nums.get(i).copied().flatten().unwrap_or(0);
        self.wrap_pending = false;
        if inter {
            // DECSCUSR and friends: no effect here.
            return;
        }
        match (lead, fin) {
            (0, b'A') => self.row = self.row.saturating_sub(p(0, 1)).max(self.limit_top()),
            (0, b'B' | b'e') => self.row = (self.row + p(0, 1)).min(self.limit_bottom()),
            (0, b'C' | b'a') => self.col = (self.col + p(0, 1)).min(self.cols - 1),
            (0, b'D') => self.col = self.col.saturating_sub(p(0, 1)),
            (0, b'E') => {
                self.col = 0;
                self.row = (self.row + p(0, 1)).min(self.limit_bottom());
            }
            (0, b'F') => {
                self.col = 0;
                self.row = self.row.saturating_sub(p(0, 1)).max(self.limit_top());
            }
            (0, b'G' | b'`') => self.col = (p(0, 1) - 1).min(self.cols - 1),
            (0, b'H' | b'f') => {
                let base = if self.origin { self.top } else { 0 };
                self.row = (base + p(0, 1) - 1).min(self.limit_bottom());
                self.col = (p(1, 1) - 1).min(self.cols - 1);
            }
            (0, b'd') => {
                let base = if self.origin { self.top } else { 0 };
                self.row = (base + p(0, 1) - 1).min(self.limit_bottom());
            }
            (0, b'I') => {
                for _ in 0..p(0, 1) {
                    self.ground(b'\t');
                }
            }
            (0, b'J') => self.erase_display(p0(0)),
            (0, b'K') => self.erase_line(p0(0)),
            (0, b'L') => {
                if (self.top..=self.bottom).contains(&self.row) {
                    let (t, row) = (self.top, self.row);
                    self.top = row;
                    self.scroll_down(p(0, 1));
                    self.top = t;
                }
            }
            (0, b'M') => {
                if (self.top..=self.bottom).contains(&self.row) {
                    let (t, row) = (self.top, self.row);
                    self.top = row;
                    self.scroll_up(p(0, 1));
                    self.top = t;
                }
            }
            (0, b'@') => {
                let (row, col, n) = (self.row, self.col, p(0, 1).min(self.cols - self.col));
                let blank = self.blank();
                let line = &mut self.g()[row];
                for _ in 0..n {
                    line.pop();
                    line.insert(col, blank);
                }
            }
            (0, b'P') => {
                let (row, col, n) = (self.row, self.col, p(0, 1).min(self.cols - self.col));
                let blank = self.blank();
                let line = &mut self.g()[row];
                line.drain(col..col + n);
                line.extend(std::iter::repeat_n(blank, n));
            }
            (0, b'X') => {
                let (row, col, n) = (self.row, self.col, p(0, 1).min(self.cols - self.col));
                let blank = self.blank();
                for c in col..col + n {
                    self.g()[row][c] = blank;
                }
            }
            (0, b'S') => self.scroll_up(p(0, 1)),
            (0, b'T') => self.scroll_down(p(0, 1)),
            (0, b'g') => match p0(0) {
                0 => self.tabs[self.col] = false,
                3 => self.tabs.iter_mut().for_each(|t| *t = false),
                _ => {}
            },
            (0, b'm') => self.sgr(&nums),
            (0, b'r') => {
                let top = p(0, 1) - 1;
                let bottom = p(1, self.rows).min(self.rows) - 1;
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.row = if self.origin { top } else { 0 };
                    self.col = 0;
                }
            }
            (0, b's') => self.saved = (self.row, self.col, self.pen.clone()),
            (0, b'u') => {
                let (r, c, _) = self.saved.clone();
                self.row = r.min(self.rows - 1);
                self.col = c.min(self.cols - 1);
            }
            (0, b'n') => match p0(0) {
                5 => self.reply.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let base = if self.origin { self.top } else { 0 };
                    let r = format!("\x1b[{};{}R", self.row - base + 1, self.col + 1);
                    self.reply.extend_from_slice(r.as_bytes());
                }
                _ => {}
            },
            (0, b'c') => self.reply.extend_from_slice(b"\x1b[?62;22c"),
            (b'>', b'c') => self.reply.extend_from_slice(b"\x1b[>0;0;0c"),
            (0, b'h') => {
                if p0(0) == 4 {
                    self.insert = true;
                }
            }
            (0, b'l') => {
                if p0(0) == 4 {
                    self.insert = false;
                }
            }
            (b'?', b'h') => nums.iter().flatten().for_each(|&m| self.mode(m, true)),
            (b'?', b'l') => nums.iter().flatten().for_each(|&m| self.mode(m, false)),
            _ => {}
        }
    }

    fn limit_top(&self) -> usize {
        if self.origin && self.row >= self.top {
            self.top
        } else {
            0
        }
    }

    fn limit_bottom(&self) -> usize {
        if self.origin && self.row <= self.bottom {
            self.bottom
        } else {
            self.rows - 1
        }
    }

    fn mode(&mut self, m: usize, on: bool) {
        match m {
            1 => self.app_cursor = on,
            6 => {
                self.origin = on;
                self.row = if on { self.top } else { 0 };
                self.col = 0;
            }
            7 => self.autowrap = on,
            25 => self.cursor_visible = on,
            1000 | 1002 | 1003 | 1006 => self.mouse = on,
            47 | 1047 | 1049 => {
                if on == self.on_alt {
                    return;
                }
                if on {
                    if m == 1049 {
                        self.saved = (self.row, self.col, self.pen.clone());
                    }
                    self.on_alt = true;
                    self.alt = Grid::new(self.rows, self.cols);
                    self.top = 0;
                    self.bottom = self.rows - 1;
                } else {
                    self.on_alt = false;
                    if m == 1049 {
                        let (r, c, _) = self.saved.clone();
                        self.row = r.min(self.rows - 1);
                        self.col = c.min(self.cols - 1);
                    }
                    self.top = 0;
                    self.bottom = self.rows - 1;
                }
            }
            2004 => self.bracketed_paste = on,
            _ => {}
        }
    }

    fn erase_display(&mut self, mode: usize) {
        let (row, col, rows) = (self.row, self.col, self.rows);
        let blank = self.blank();
        match mode {
            0 => {
                let g = self.g();
                g[row][col..].fill(blank);
                for line in g.iter_mut().take(rows).skip(row + 1) {
                    line.fill(blank);
                }
            }
            1 => {
                let g = self.g();
                for line in g.iter_mut().take(row) {
                    line.fill(blank);
                }
                g[row][..=col].fill(blank);
            }
            2 => {
                for l in self.g().iter_mut() {
                    l.fill(blank);
                }
            }
            3 => self.scrollback.clear(),
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let (row, col) = (self.row, self.col);
        let blank = self.blank();
        let line = &mut self.g()[row];
        match mode {
            0 => line[col..].fill(blank),
            1 => line[..=col].fill(blank),
            _ => line.fill(blank),
        }
    }

    fn sgr(&mut self, nums: &[Option<usize>]) {
        let mut i = 0;
        let get = |i: usize| nums.get(i).copied().flatten();
        if nums.is_empty() {
            self.pen = Pen::default();
            return;
        }
        while i < nums.len() {
            let n = get(i).unwrap_or(0);
            match n {
                0 => self.pen = Pen::default(),
                1 => self.pen.attrs |= BOLD,
                2 => self.pen.attrs |= DIM,
                3 => self.pen.attrs |= ITALIC,
                4 => self.pen.attrs |= UNDERLINE,
                7 => self.pen.attrs |= REVERSE,
                9 => self.pen.attrs |= STRIKE,
                22 => self.pen.attrs &= !(BOLD | DIM),
                23 => self.pen.attrs &= !ITALIC,
                24 => self.pen.attrs &= !UNDERLINE,
                27 => self.pen.attrs &= !REVERSE,
                29 => self.pen.attrs &= !STRIKE,
                30..=37 => self.pen.fg = Color::Rgb(palette((n - 30) as u8)),
                40..=47 => self.pen.bg = Color::Rgb(palette((n - 40) as u8)),
                90..=97 => self.pen.fg = Color::Rgb(palette((n - 90 + 8) as u8)),
                100..=107 => self.pen.bg = Color::Rgb(palette((n - 100 + 8) as u8)),
                39 => self.pen.fg = Color::Default,
                49 => self.pen.bg = Color::Default,
                38 | 48 => {
                    let color = match get(i + 1) {
                        Some(5) => {
                            let c = get(i + 2).map(|v| Color::Rgb(palette(v.min(255) as u8)));
                            i += 2;
                            c
                        }
                        Some(2) => {
                            let ch = |k: usize| get(i + k).unwrap_or(0).min(255) as u32;
                            let c = Color::Rgb((ch(2) << 16) | (ch(3) << 8) | ch(4));
                            i += 4;
                            Some(c)
                        }
                        _ => None,
                    };
                    if let Some(c) = color {
                        if n == 38 {
                            self.pen.fg = c;
                        } else {
                            self.pen.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Rows scrolled into the scrollback since the start.
    pub fn scrolled_total(&self) -> usize {
        self.scrolled_total
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Vt, r: usize) -> String {
        v.grid()[r]
            .iter()
            .map(|c| c.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn text_wraps_and_scrolls_into_scrollback() {
        let mut v = Vt::new(3, 5);
        v.feed(b"abcdefg\r\nxx\r\nyy\r\nzz");
        assert_eq!(v.scrollback.len(), 2);
        assert_eq!(text(&v, 0), "xx");
        assert_eq!(text(&v, 2), "zz");
    }

    #[test]
    fn colours_and_attributes() {
        let mut v = Vt::new(2, 10);
        v.feed(b"\x1b[1;31mR\x1b[0m\x1b[38;2;1;2;3mT\x1b[48;5;21mB\x1b[mN");
        let r = v.grid()[0][0];
        assert_eq!(r.fg, Color::Rgb(0xcd3131));
        assert_eq!(r.attrs, BOLD);
        assert_eq!(v.grid()[0][1].fg, Color::Rgb(0x010203));
        assert_eq!(v.grid()[0][2].bg, Color::Rgb(palette(21)));
        assert_eq!(v.grid()[0][3].fg, Color::Default);
    }

    #[test]
    fn cursor_movement_and_erase() {
        let mut v = Vt::new(3, 10);
        v.feed(b"hello\x1b[2;3Hab\x1b[1;1H\x1b[K");
        assert_eq!(text(&v, 0), "");
        assert_eq!(text(&v, 1), "  ab");
        v.feed(b"\x1b[2J");
        assert_eq!(text(&v, 1), "");
    }

    #[test]
    fn alt_screen_comes_and_goes() {
        let mut v = Vt::new(2, 6);
        v.feed(b"main\x1b[?1049h\x1b[Halt\x1b[?1049l");
        assert_eq!(text(&v, 0), "main");
        assert!(!v.on_alt_screen());
    }

    #[test]
    fn replies_to_cursor_position_and_title_and_cwd() {
        let mut v = Vt::new(5, 20);
        v.feed(b"ab\x1b[6n\x1b]0;my title\x07\x1b]7;file://h/tmp/a%20b\x1b\\");
        assert_eq!(v.reply, b"\x1b[1;3R");
        assert_eq!(v.title, "my title");
        assert_eq!(v.cwd.as_deref(), Some("/tmp/a b"));
    }

    #[test]
    fn utf8_and_wide_characters() {
        let mut v = Vt::new(2, 6);
        v.feed("é世x".as_bytes());
        assert_eq!(v.grid()[0][0].ch, 'é');
        assert_eq!(v.grid()[0][1].ch, '世');
        assert_eq!(v.grid()[0][2].ch, Cell::CONT);
        assert_eq!(v.grid()[0][3].ch, 'x');
    }

    #[test]
    fn scroll_region_and_insert_delete() {
        let mut v = Vt::new(4, 4);
        v.feed(b"1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(text(&v, 0), "1");
        assert_eq!(text(&v, 1), "3");
        assert_eq!(text(&v, 3), "4");
        v.feed(b"\x1b[1;1Habc\x1b[1;1H\x1b[2P");
        assert_eq!(text(&v, 0), "c");
    }

    #[test]
    fn resize_keeps_the_cursor_line() {
        let mut v = Vt::new(4, 6);
        v.feed(b"a\r\nb\r\nc\r\nd");
        v.resize(2, 6);
        assert_eq!(text(&v, 1), "d");
        assert_eq!(v.scrollback.len(), 2);
    }
}
