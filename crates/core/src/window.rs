// SPDX-License-Identifier: MIT OR Apache-2.0
//! Windows: views onto buffers, arranged as a tree of splits.
//!
//! Every window has its own point and scroll position. The selected
//! window's point lives in its buffer (so editing code only ever deals
//! with `Buffer::point`); the others remember theirs as a char offset plus
//! the buffer version it was taken at, and the buffer's edit journal
//! carries it across edits made through other windows.
//!
//! Frames: a frame owns one window tree. All the
//! window methods above act on the *selected* frame, so callers that only
//! ever think about windows need not know frames exist. [`Windows`] also
//! carries the other frames: their own trees, their selected window, and
//! the screen area each was last laid out in.

use crate::app::BufferId;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

/// A direction on the screen, for windmove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// A window arrangement saved by [`Windows::configuration`].
#[derive(Debug, Clone, PartialEq)]
pub struct WindowConfig {
    root: Node,
    selected: WindowId,
    shown: Vec<(WindowId, BufferId, usize)>,
}

/// How a split arranges its two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// One above the other (C-x 2, split-window-below).
    Rows,
    /// Side by side (C-x 3, split-window-right).
    Columns,
}

#[derive(Debug, Clone)]
pub struct Window {
    pub id: WindowId,
    pub buffer: BufferId,
    /// Point while this window is not selected: (offset, buffer version).
    pub(crate) saved_point: (usize, u64),
    /// First buffer line shown.
    pub scroll_top: usize,
    /// Fractional vertical pixel offset of `scroll_top` within the window
    /// (true pixel scrolling). Applied at redisplay; 0 means the line starts
    /// at the window's top.
    pub scroll_y_px: usize,
    /// First display column shown (long lines scroll sideways).
    pub hscroll: usize,
    /// Text rows and columns at the last redisplay.
    pub height: usize,
    pub width: usize,
    /// Columns of line numbers left of the text at the last redisplay.
    pub gutter: usize,
    /// The clickable parts of the mode line at the last redisplay.
    pub mode_line_hits: Vec<crate::display::ModeLineHit>,
    /// What each screen row showed at the last redisplay: (line, first
    /// column). Wrapped lines take several rows.
    pub rows: Vec<(usize, usize)>,
    /// Rows above the text at the last redisplay (Org's table header line).
    pub header_rows: usize,
    /// Where the completion popup was drawn: (x, y, width, rows) in
    /// screen cells, for clicks.
    pub popup_area: Option<(u16, u16, u16, u16)>,
    /// The vertical scroll bar's thumb at the last redisplay: (start row,
    /// rows), relative to the text area, for clicks (scroll-bar-mode).
    pub scroll_bar: Option<(u16, u16)>,
    /// The horizontal scroll bar's thumb at the last redisplay: (start
    /// column, columns), relative to the text area, for clicks
    /// (horizontal-scroll-bar-mode).
    pub horizontal_scroll_bar: Option<(u16, u16)>,
    /// C-l cycles center / top / bottom.
    pub recenter_step: u8,
    /// Scroll position per buffer this window showed before.
    prev_scroll: HashMap<BufferId, usize>,
}

impl Window {
    fn new(id: WindowId, buffer: BufferId) -> Self {
        Self {
            id,
            buffer,
            saved_point: (0, 0),
            scroll_top: 0,
            scroll_y_px: 0,
            hscroll: 0,
            height: 20,
            width: 80,
            gutter: 0,
            mode_line_hits: Vec::new(),
            rows: Vec::new(),
            header_rows: 0,
            popup_area: None,
            scroll_bar: None,
            horizontal_scroll_bar: None,
            recenter_step: 0,
            prev_scroll: HashMap::new(),
        }
    }

    /// Show another buffer, remembering where this one was scrolled to.
    pub(crate) fn set_buffer(&mut self, buffer: BufferId) {
        if buffer == self.buffer {
            return;
        }
        self.prev_scroll.insert(self.buffer, self.scroll_top);
        self.buffer = buffer;
        self.scroll_top = self.prev_scroll.get(&buffer).copied().unwrap_or(0);
        self.hscroll = 0;
        self.scroll_y_px = 0;
    }

    pub(crate) fn forget_buffer(&mut self, buffer: BufferId) {
        self.prev_scroll.remove(&buffer);
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(WindowId),
    Split {
        dir: Direction,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    fn leaves(&self, out: &mut Vec<WindowId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    fn contains(&self, id: WindowId) -> bool {
        match self {
            Node::Leaf(w) => *w == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Replace the leaf `id` with `new`.
    fn replace(&mut self, id: WindowId, new: Node) -> bool {
        match self {
            Node::Leaf(w) if *w == id => {
                *self = new;
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.replace(id, new.clone()) || b.replace(id, new),
        }
    }

    /// Remove leaf `id`; its sibling takes the parent's place.
    fn remove(&mut self, id: WindowId) -> bool {
        let Node::Split { a, b, .. } = self else {
            return false;
        };
        if matches!(**a, Node::Leaf(w) if w == id) {
            *self = (**b).clone();
            return true;
        }
        if matches!(**b, Node::Leaf(w) if w == id) {
            *self = (**a).clone();
            return true;
        }
        a.remove(id) || b.remove(id)
    }

    /// Leaves counted along `dir` (for balancing).
    fn weight(&self, dir: Direction) -> f32 {
        match self {
            Node::Split { dir: d, a, b, .. } if *d == dir => a.weight(dir) + b.weight(dir),
            _ => 1.0,
        }
    }

    /// The same tree, with every leaf rewritten through `map` (a copy of
    /// the tab gets fresh window ids).
    fn remap(&self, map: &HashMap<WindowId, WindowId>) -> Node {
        match self {
            Node::Leaf(id) => Node::Leaf(*map.get(id).unwrap_or(id)),
            Node::Split { dir, ratio, a, b } => Node::Split {
                dir: *dir,
                ratio: *ratio,
                a: Box::new(a.remap(map)),
                b: Box::new(b.remap(map)),
            },
        }
    }

    fn balance(&mut self) {
        if let Node::Split { dir, ratio, a, b } = self {
            let (wa, wb) = (a.weight(*dir), b.weight(*dir));
            *ratio = wa / (wa + wb);
            a.balance();
            b.balance();
        }
    }

    fn layout(&self, r: Rect, out: &mut Vec<(WindowId, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, r)),
            Node::Split { dir, ratio, a, b } => {
                let (ra, rb) = split_rect(r, *dir, *ratio);
                a.layout(ra, out);
                b.layout(rb, out);
            }
        }
    }

    /// Grow the window `id` by `delta` cells along `dir`, by moving the
    /// nearest enclosing split of that direction. Returns false if there
    /// is no such split or the result would be too small.
    fn resize(&mut self, id: WindowId, dir: Direction, delta: isize, r: Rect) -> bool {
        let Node::Split {
            dir: d,
            ratio,
            a,
            b,
        } = self
        else {
            return false;
        };
        let (ra, rb) = split_rect(r, *d, *ratio);
        let in_a = a.contains(id);
        let (child, child_rect) = if in_a { (a, ra) } else { (b, rb) };
        if child.resize(id, dir, delta, child_rect) {
            return true;
        }
        if *d != dir {
            return false;
        }
        let total = match dir {
            Direction::Rows => r.height,
            Direction::Columns => r.width,
        } as isize;
        let size_a = match dir {
            Direction::Rows => ra.height,
            Direction::Columns => ra.width,
        } as isize;
        let new_a = if in_a { size_a + delta } else { size_a - delta };
        let min = min_size(dir) as isize;
        if new_a < min || total - new_a < min {
            return false;
        }
        *ratio = new_a as f32 / total as f32;
        true
    }
}

fn min_size(dir: Direction) -> u16 {
    match dir {
        // One text line plus the mode line.
        Direction::Rows => 2,
        Direction::Columns => 10,
    }
}

fn split_rect(r: Rect, dir: Direction, ratio: f32) -> (Rect, Rect) {
    match dir {
        Direction::Rows => {
            let h = ((r.height as f32 * ratio).round() as u16)
                .clamp(1, r.height.saturating_sub(1).max(1));
            (
                Rect { height: h, ..r },
                Rect {
                    y: r.y + h,
                    height: r.height - h,
                    ..r
                },
            )
        }
        Direction::Columns => {
            let w = ((r.width as f32 * ratio).round() as u16)
                .clamp(1, r.width.saturating_sub(1).max(1));
            (
                Rect { width: w, ..r },
                Rect {
                    x: r.x + w,
                    width: r.width - w,
                    ..r
                },
            )
        }
    }
}

/// A screen rectangle in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// One tab: a window tree, its selected window, its name, and the area
/// it was last laid out in.
#[derive(Debug, Clone)]
struct Tab {
    /// The name a default (derived from the buffers it shows) had when
    /// the tab was made; `custom` overrides it (tab-rename).
    name: String,
    custom: Option<String>,
    root: Node,
    windows: BTreeMap<WindowId, Window>,
    selected: WindowId,
    /// Screen area the tab was last laid out in (for split size checks).
    area: Rect,
}

impl Tab {
    fn new(name: String, buffer: BufferId, window: WindowId) -> Self {
        let mut windows = BTreeMap::new();
        windows.insert(window, Window::new(window, buffer));
        Self {
            name,
            custom: None,
            root: Node::Leaf(window),
            windows,
            selected: window,
            area: Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 23,
            },
        }
    }
}

/// One frame: contains tabs (each tab has its own window configuration).
#[derive(Debug, Clone)]
struct FrameWin {
    name: String,
    tabs: BTreeMap<TabId, Tab>,
    tab_order: Vec<TabId>,
    selected_tab: TabId,
    next_tab_id: u32,
    /// Screen area the frame was last laid out in (for split size checks).
    area: Rect,
}

impl FrameWin {
    fn new(name: String, buffer: BufferId, window: WindowId) -> Self {
        let tab_id = TabId(0);
        let tab = Tab::new(name.clone(), buffer, window);
        let mut tabs = BTreeMap::new();
        tabs.insert(tab_id, tab);
        Self {
            name,
            tabs,
            tab_order: vec![tab_id],
            selected_tab: tab_id,
            next_tab_id: 1,
            area: Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 23,
            },
        }
    }
}

/// All frames, and within each frame all windows and their arrangement.
/// The window methods act on the selected frame.
#[derive(Debug, Clone)]
pub struct Windows {
    frames: BTreeMap<FrameId, FrameWin>,
    /// Frame ids in creation order (how `other-frame` cycles).
    order: Vec<FrameId>,
    selected: FrameId,
    next_frame_id: u32,
    next_window_id: u32,
    /// Recently deleted frames, oldest first: (id, the frame, where it sat
    /// in `order`), for `undelete-frame`.
    deleted: Vec<(FrameId, FrameWin, usize)>,
    /// Recently deleted tabs, oldest first: (frame, id, the tab, where it
    /// sat in the frame's order), for `tab-undo`.
    deleted_tabs: Vec<(FrameId, TabId, Tab, usize)>,
}

impl Windows {
    pub fn new(buffer: BufferId) -> Self {
        let fid = FrameId(0);
        let wid = WindowId(0);
        let mut frames = BTreeMap::new();
        frames.insert(fid, FrameWin::new("F1".to_string(), buffer, wid));
        Self {
            frames,
            order: vec![fid],
            selected: fid,
            next_frame_id: 1,
            next_window_id: 1,
            deleted: Vec::new(),
            deleted_tabs: Vec::new(),
        }
    }

    // ---------------------------------------------------------- windows

    fn frame_ref(&self) -> &FrameWin {
        &self.frames[&self.selected]
    }

    fn frame_mut(&mut self) -> &mut FrameWin {
        let id = self.selected;
        self.frames.get_mut(&id).expect("selected frame exists")
    }

    fn tab_ref(&self) -> &Tab {
        let frame = self.frame_ref();
        &frame.tabs[&frame.selected_tab]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        let frame_id = self.selected;
        let tab_id = self.frames[&frame_id].selected_tab;
        let frame = self
            .frames
            .get_mut(&frame_id)
            .expect("selected frame exists");
        frame.tabs.get_mut(&tab_id).expect("selected tab exists")
    }

    pub fn selected_id(&self) -> WindowId {
        self.tab_ref().selected
    }

    pub fn selected(&self) -> &Window {
        &self.tab_ref().windows[&self.selected_id()]
    }

    pub fn selected_mut(&mut self) -> &mut Window {
        let id = self.selected_id();
        self.tab_mut()
            .windows
            .get_mut(&id)
            .expect("selected window exists")
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.frames
            .values()
            .find_map(|f| f.tabs.values().find_map(|t| t.windows.get(&id)))
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.frames
            .values_mut()
            .find_map(|f| f.tabs.values_mut().find_map(|t| t.windows.get_mut(&id)))
    }

    /// Window ids of the selected frame in screen order, as C-x o visits.
    pub fn ids(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.tab_ref().root.leaves(&mut out);
        out
    }

    pub fn count(&self) -> usize {
        self.tab_ref().windows.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Window> {
        self.tab_ref().windows.values()
    }

    /// Every window in every frame (killing a buffer has to reach them all),
    /// including the frames kept for `undelete-frame`: a buffer killed while
    /// a frame is deleted must not come back dangling when it is undeleted.
    pub(crate) fn all_windows_mut(&mut self) -> impl Iterator<Item = &mut Window> {
        let live = self
            .frames
            .values_mut()
            .flat_map(|f| f.tabs.values_mut().flat_map(|t| t.windows.values_mut()));
        let deleted = self
            .deleted
            .iter_mut()
            .flat_map(|(_, f, _)| f.tabs.values_mut().flat_map(|t| t.windows.values_mut()));
        let deleted_tabs = self
            .deleted_tabs
            .iter_mut()
            .flat_map(|(_, _, t, _)| t.windows.values_mut());
        live.chain(deleted).chain(deleted_tabs)
    }

    /// The window `n` steps after the selected one (negative: before).
    pub fn nth_from_selected(&self, n: isize) -> WindowId {
        let ids = self.ids();
        let i = ids
            .iter()
            .position(|w| *w == self.selected_id())
            .unwrap_or(0) as isize;
        let len = ids.len() as isize;
        ids[((i + n) % len + len) as usize % ids.len()]
    }

    pub(crate) fn set_selected(&mut self, id: WindowId) {
        assert!(self.get(id).is_some());
        self.tab_mut().selected = id;
    }

    /// Screen area of every window in the selected frame (text rows plus
    /// mode line).
    pub fn layout(&self, area: Rect) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        self.tab_ref().root.layout(area, &mut out);
        out
    }

    fn rect_of(&self, id: WindowId) -> Option<Rect> {
        let frame = self.tab_ref();
        let mut out = Vec::new();
        frame.root.layout(frame.area, &mut out);
        out.into_iter().find(|(w, _)| *w == id).map(|(_, r)| r)
    }

    /// Split the selected window. The new window shows the same buffer
    /// with the same scroll position; the selection stays put.
    pub(crate) fn split(
        &mut self,
        dir: Direction,
        point: (usize, u64),
    ) -> Result<WindowId, String> {
        let r = self.rect_of(self.selected_id()).unwrap_or(self.frame());
        let size = match dir {
            Direction::Rows => r.height,
            Direction::Columns => r.width,
        };
        if size < 2 * min_size(dir) {
            return Err(format!(
                "Window {} is too small for splitting",
                self.selected_id().0
            ));
        }
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        let (buffer, scroll_top, hscroll, scroll_y_px) = {
            let old = self.selected();
            (old.buffer, old.scroll_top, old.hscroll, old.scroll_y_px)
        };
        let mut w = Window::new(id, buffer);
        w.scroll_top = scroll_top;
        w.hscroll = hscroll;
        w.scroll_y_px = scroll_y_px;
        w.saved_point = point;
        let sel = self.selected_id();
        let tab = self.tab_mut();
        tab.windows.insert(id, w);
        let node = Node::Split {
            dir,
            ratio: 0.5,
            a: Box::new(Node::Leaf(sel)),
            b: Box::new(Node::Leaf(id)),
        };
        tab.root.replace(sel, node);
        Ok(id)
    }

    /// Delete a window in the selected frame (not the last one).
    pub(crate) fn delete(&mut self, id: WindowId) -> Result<(), String> {
        if self.count() == 1 {
            return Err("Cannot delete the minibuffer or the only window".into());
        }
        let tab = self.tab_mut();
        if !tab.root.remove(id) {
            return Err("No such window".into());
        }
        tab.windows.remove(&id);
        Ok(())
    }

    /// Keep only the selected window (of the selected frame).
    pub(crate) fn delete_others(&mut self) {
        let sel = self.selected_id();
        let tab = self.tab_mut();
        tab.windows.retain(|id, _| *id == sel);
        tab.root = Node::Leaf(sel);
    }

    /// Make all windows in the selected frame the same size (C-x +).
    pub fn balance(&mut self) {
        self.tab_mut().root.balance();
    }

    /// Grow the selected window by `delta` rows or columns (negative shrinks).
    pub fn enlarge(&mut self, dir: Direction, delta: isize) -> Result<(), String> {
        let (sel, area) = (self.selected_id(), self.tab_ref().area);
        if self.tab_mut().root.resize(sel, dir, delta, area) {
            Ok(())
        } else {
            Err("Cannot resize this window further".into())
        }
    }

    // ------------------------------------------------- windmove / winner

    /// The window next to the selected one in `side` (windmove): the
    /// adjacent window sharing the most of the selected window's edge.
    pub fn neighbor(&self, side: Side) -> Option<WindowId> {
        let me = self.selected_id();
        let frame = self.tab_ref();
        let mut all = Vec::new();
        frame.root.layout(frame.area, &mut all);
        let r = all.iter().find(|(w, _)| *w == me)?.1;
        let (rx0, rx1) = (r.x as i32, r.x as i32 + r.width as i32);
        let (ry0, ry1) = (r.y as i32, r.y as i32 + r.height as i32);
        let overlap = |a0: i32, a1: i32, b0: i32, b1: i32| (a1.min(b1) - a0.max(b0)).max(0);
        all.iter()
            .filter(|(w, _)| *w != me)
            .filter_map(|(w, c)| {
                let (cx0, cx1) = (c.x as i32, c.x as i32 + c.width as i32);
                let (cy0, cy1) = (c.y as i32, c.y as i32 + c.height as i32);
                // Gap between the two edges, and how much of them touch.
                let (gap, shared) = match side {
                    Side::Left => (rx0 - cx1, overlap(ry0, ry1, cy0, cy1)),
                    Side::Right => (cx0 - rx1, overlap(ry0, ry1, cy0, cy1)),
                    Side::Up => (ry0 - cy1, overlap(rx0, rx1, cx0, cx1)),
                    Side::Down => (cy0 - ry1, overlap(rx0, rx1, cx0, cx1)),
                };
                (0..=1).contains(&gap).then_some((*w, gap, shared))
            })
            .filter(|(_, _, shared)| *shared > 0)
            .max_by_key(|(w, gap, shared)| (*shared, -*gap, std::cmp::Reverse(w.0)))
            .map(|(w, _, _)| w)
    }

    /// The selected frame's window arrangement, for winner-undo.
    pub fn configuration(&self) -> WindowConfig {
        let tab = self.tab_ref();
        WindowConfig {
            root: tab.root.clone(),
            selected: tab.selected,
            shown: tab
                .windows
                .values()
                .map(|w| (w.id, w.buffer, w.scroll_top))
                .collect(),
        }
    }

    /// Put an arrangement back. Windows that were deleted since come back
    /// showing the buffer they showed (or `fallback` if it is gone).
    pub fn restore(
        &mut self,
        cfg: &WindowConfig,
        alive: impl Fn(BufferId) -> bool,
        fallback: BufferId,
    ) {
        let tab = self.tab_mut();
        tab.root = cfg.root.clone();
        tab.windows
            .retain(|id, _| cfg.shown.iter().any(|(w, _, _)| w == id));
        for (id, buffer, top) in &cfg.shown {
            let buffer = if alive(*buffer) { *buffer } else { fallback };
            let w = tab
                .windows
                .entry(*id)
                .or_insert_with(|| Window::new(*id, buffer));
            w.set_buffer(buffer);
            w.scroll_top = *top;
        }
        tab.selected = if tab.windows.contains_key(&cfg.selected) {
            cfg.selected
        } else {
            *tab.windows.keys().next().expect("a window remains")
        };
    }

    // ----------------------------------------------------------- frames

    pub fn selected_frame_id(&self) -> FrameId {
        self.selected
    }

    pub fn frame_selected_tab_id(&self, id: FrameId) -> TabId {
        self.frames[&id].selected_tab
    }

    pub fn selected_tab_id(&self) -> TabId {
        self.frame_ref().selected_tab
    }

    pub fn tab_count(&self) -> usize {
        self.frame_ref().tabs.len()
    }

    pub fn tab_ids(&self) -> Vec<TabId> {
        self.frame_ref().tab_order.clone()
    }

    /// The same as [`Self::tab_count`]/[`Self::tab_ids`], for a frame that
    /// isn't the selected one (the macOS app draws every frame's bar).
    pub fn frame_tab_count(&self, frame: FrameId) -> usize {
        self.frames.get(&frame).map_or(0, |f| f.tabs.len())
    }

    pub fn frame_tab_ids(&self, frame: FrameId) -> Vec<TabId> {
        self.frames
            .get(&frame)
            .map(|f| f.tab_order.clone())
            .unwrap_or_default()
    }

    pub fn tab_name(&self, tab_id: TabId) -> Option<&str> {
        let tab = self.frame_ref().tabs.get(&tab_id)?;
        Some(tab.custom.as_deref().unwrap_or(tab.name.as_str()))
    }

    pub fn current_tab_name(&self) -> Option<String> {
        let tab = self.tab_ref();
        Some(tab.name.clone())
    }

    /// A new tab in the selected frame that is a copy of the current one:
    /// the same window configuration, buffers, scroll positions and points
    ///. The caller moves point into it via
    /// [`crate::Editor::select_tab`].
    pub(crate) fn clone_current_tab(&mut self, name: String) -> TabId {
        let (tab_id, src) = {
            let frame = self.frame_mut();
            let tid = TabId(frame.next_tab_id);
            frame.next_tab_id += 1;
            let sid = frame.selected_tab;
            (tid, frame.tabs[&sid].clone())
        };
        let mut map = HashMap::new();
        let mut windows = BTreeMap::new();
        for (id, w) in &src.windows {
            let nid = WindowId(self.next_window_id);
            self.next_window_id += 1;
            map.insert(*id, nid);
            let mut w = w.clone();
            w.id = nid;
            windows.insert(nid, w);
        }
        let tab = Tab {
            name,
            custom: None,
            root: src.root.remap(&map),
            windows,
            selected: map[&src.selected],
            area: src.area,
        };
        let frame = self.frame_mut();
        frame.tabs.insert(tab_id, tab);
        frame.tab_order.push(tab_id);
        tab_id
    }

    /// Rename the tab: an empty name clears a custom one, so the default
    /// (derived from its buffers) comes back.
    pub(crate) fn set_tab_name(&mut self, id: TabId, name: &str) -> bool {
        let frame = self.frame_ref();
        if !frame.tabs.contains_key(&id) {
            return false;
        }
        let frame = self.frame_mut();
        let tab = frame.tabs.get_mut(&id).expect("checked");
        tab.custom = if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        };
        true
    }

    /// The tab's name as `tab-rename` gave it (None: the derived one).
    pub fn custom_tab_name(&self, id: TabId) -> Option<String> {
        self.frame_ref().tabs.get(&id)?.custom.clone()
    }

    /// The same as [`Self::custom_tab_name`], for any frame.
    pub fn frame_custom_tab_name(&self, frame: FrameId, id: TabId) -> Option<String> {
        self.frames.get(&frame)?.tabs.get(&id)?.custom.clone()
    }

    /// The buffers each of the tab's windows shows, for its derived name.
    pub fn tab_window_buffers(&self, id: TabId) -> Vec<BufferId> {
        self.frame_ref()
            .tabs
            .get(&id)
            .map(|t| t.windows.values().map(|w| w.buffer).collect())
            .unwrap_or_default()
    }

    /// The same as [`Self::tab_window_buffers`], for any frame.
    pub fn frame_tab_window_buffers(&self, frame: FrameId, id: TabId) -> Vec<BufferId> {
        self.frames
            .get(&frame)
            .and_then(|f| f.tabs.get(&id))
            .map(|t| t.windows.values().map(|w| w.buffer).collect())
            .unwrap_or_default()
    }

    /// Move the selected tab `n` places in the order (tab-move).
    pub(crate) fn move_tab(&mut self, n: isize) {
        let frame = self.frame_mut();
        let cur = frame.selected_tab;
        let Some(at) = frame.tab_order.iter().position(|t| *t == cur) else {
            return;
        };
        let len = frame.tab_order.len() as isize;
        let to = if n >= 0 {
            (at as isize + n).min(len - 1)
        } else {
            (at as isize + n).max(0)
        } as usize;
        let tab = frame.tab_order.remove(at);
        frame.tab_order.insert(to, tab);
    }

    /// Put a frame's tab at `to` in its order (dragging a tab in the app's
    /// tab strip). `to` is clamped to the last place. False when the frame
    /// or tab doesn't exist.
    pub(crate) fn move_tab_to(&mut self, frame: FrameId, tab: TabId, to: usize) -> bool {
        let Some(frame) = self.frames.get_mut(&frame) else {
            return false;
        };
        let Some(at) = frame.tab_order.iter().position(|t| *t == tab) else {
            return false;
        };
        let moved = frame.tab_order.remove(at);
        let to = to.min(frame.tab_order.len());
        frame.tab_order.insert(to, moved);
        true
    }

    pub(crate) fn select_tab(&mut self, id: TabId) -> bool {
        if self.frame_ref().tabs.contains_key(&id) {
            let frame = self.frame_mut();
            frame.selected_tab = id;
            true
        } else {
            false
        }
    }

    pub(crate) fn delete_tab(&mut self, id: TabId) -> Result<(), String> {
        if self.tab_count() == 1 {
            return Err("Attempt to delete the sole tab".into());
        }
        let frame_id = self.selected;
        let (tab, at, next) = {
            let frame = self.frame_mut();
            if !frame.tabs.contains_key(&id) {
                return Err("No such tab".into());
            }
            let at = frame.tab_order.iter().position(|t| *t == id).unwrap_or(0);
            frame.tab_order.remove(at);
            let tab = frame.tabs.remove(&id).expect("checked");
            // The next tab along takes over (or the one before, at the end).
            let next = frame
                .tab_order
                .get(at)
                .or_else(|| frame.tab_order.get(at.saturating_sub(1)))
                .copied()
                .expect("not the sole tab");
            (tab, at, next)
        };
        {
            let frame = self.frame_mut();
            if frame.selected_tab == id {
                frame.selected_tab = next;
            }
        }
        // Remembered for tab-undo, like a deleted frame is.
        self.deleted_tabs.push((frame_id, id, tab, at));
        Ok(())
    }

    /// Bring back a tab deleted with [`Self::delete_tab`]: `arg` is 1 for
    /// the most recently deleted, 2 for the one before it, and so on. It
    /// goes back where it sat in its frame's order. Returns the frame and
    /// the tab, or None if there is no such deleted tab (or its frame is
    /// gone).
    pub(crate) fn undelete_tab(&mut self, arg: usize) -> Option<(FrameId, TabId)> {
        if arg == 0 || arg > self.deleted_tabs.len() {
            return None;
        }
        let i = self.deleted_tabs.len() - arg;
        let (fid, mut tid, tab, at) = self.deleted_tabs.remove(i);
        let frame = self.frames.get_mut(&fid)?;
        if frame.tabs.contains_key(&tid) {
            tid = TabId(frame.next_tab_id);
            frame.next_tab_id += 1;
        }
        frame.tabs.insert(tid, tab);
        let at = at.min(frame.tab_order.len());
        frame.tab_order.insert(at, tid);
        Some((fid, tid))
    }

    /// How many tabs `tab-undo` could bring back.
    pub fn deleted_tab_count(&self) -> usize {
        self.deleted_tabs.len()
    }

    pub fn other_tab(&self, n: isize) -> TabId {
        let frame = self.frame_ref();
        let i = frame
            .tab_order
            .iter()
            .position(|t| *t == frame.selected_tab)
            .unwrap_or(0) as isize;
        let len = frame.tab_order.len() as isize;
        frame.tab_order[((i + n) % len + len) as usize % frame.tab_order.len()]
    }

    pub fn nth_tab(&self, n: usize) -> Option<TabId> {
        self.frame_ref().tab_order.get(n).copied()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Frame ids in creation order.
    pub fn frame_ids(&self) -> Vec<FrameId> {
        self.order.clone()
    }

    pub fn frame_name(&self, id: FrameId) -> Option<&str> {
        self.frames.get(&id).map(|f| f.name.as_str())
    }

    /// The frame's selected window.
    pub fn frame_selected_id(&self, id: FrameId) -> WindowId {
        let frame = &self.frames[&id];
        let tab_id = frame.selected_tab;
        frame.tabs[&tab_id].selected
    }

    /// The selected frame's screen area (set by redisplay).
    pub fn frame(&self) -> Rect {
        self.tab_ref().area
    }

    /// Set the selected frame's screen area.
    pub fn set_frame(&mut self, area: Rect) {
        self.tab_mut().area = area;
    }

    /// Set any frame's screen area, and lay its windows out in it.
    pub fn frame_layout(&mut self, id: FrameId, area: Rect) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        if let Some(frame) = self.frames.get_mut(&id) {
            frame.area = area;
            let tab_id = frame.selected_tab;
            if let Some(tab) = frame.tabs.get_mut(&tab_id) {
                tab.area = area;
                tab.root.layout(area, &mut out);
            }
        }
        out
    }

    /// Select an existing frame. Returns false if there is no such frame.
    /// The caller is responsible for moving point between the old and new
    /// selected windows (see [`crate::Editor::select_frame`]).
    pub(crate) fn select_frame(&mut self, id: FrameId) -> bool {
        if self.frames.contains_key(&id) {
            self.selected = id;
            true
        } else {
            false
        }
    }

    /// The frame `n` steps after the selected one (negative: before), in
    /// creation order.
    pub fn other_frame(&self, n: isize) -> FrameId {
        let i = self
            .order
            .iter()
            .position(|f| *f == self.selected)
            .unwrap_or(0) as isize;
        let len = self.order.len() as isize;
        self.order[((i + n) % len + len) as usize % self.order.len()]
    }

    /// Create a frame showing `buffer` in one window, named `name`.
    /// `point` is where that window's point starts. Does not select it.
    pub(crate) fn make_frame(
        &mut self,
        name: String,
        buffer: BufferId,
        point: (usize, u64),
    ) -> FrameId {
        let fid = FrameId(self.next_frame_id);
        self.next_frame_id += 1;
        let wid = WindowId(self.next_window_id);
        self.next_window_id += 1;
        let mut frame = FrameWin::new(name, buffer, wid);
        let tab_id = frame.selected_tab;
        if let Some(tab) = frame.tabs.get_mut(&tab_id) {
            let wid = *tab.windows.keys().next().unwrap_or(&wid);
            if let Some(w) = tab.windows.get_mut(&wid) {
                w.saved_point = point;
            }
        }
        self.frames.insert(fid, frame);
        self.order.push(fid);
        fid
    }

    /// Remove a frame (not the last one). Returns the frame that becomes
    /// selected if the deleted one was selected.
    pub(crate) fn delete_frame(&mut self, id: FrameId) -> Result<Option<FrameId>, String> {
        if self.frames.len() == 1 {
            return Err("Attempt to delete the sole frame".into());
        }
        if !self.frames.contains_key(&id) {
            return Err("No such frame".into());
        }
        let frame = self.frames.remove(&id).unwrap();
        let at = self.order.iter().position(|f| *f == id).unwrap_or(0);
        self.order.remove(at);
        // Remembered for undelete-frame: the frame itself and where in
        // the creation order it sat.
        self.deleted.push((id, frame, at));
        if self.selected == id {
            let next = self.other_frame(0);
            self.selected = next;
            Ok(Some(next))
        } else {
            Ok(None)
        }
    }

    /// Keep only the selected frame (C-x 5 1, delete-other-frames).
    pub(crate) fn delete_other_frames(&mut self) {
        let keep = self.selected;
        let others: Vec<FrameId> = self
            .order
            .iter()
            .copied()
            .filter(|id| *id != keep)
            .collect();
        for id in others {
            let _ = self.delete_frame(id);
        }
    }

    /// The first frame (in creation order) with a window showing `buffer`.
    pub fn frame_showing(&self, buffer: BufferId) -> Option<FrameId> {
        self.order.iter().copied().find(|id| {
            self.frames[id]
                .tabs
                .values()
                .any(|t| t.windows.values().any(|w| w.buffer == buffer))
        })
    }

    /// Rename a frame.
    pub fn set_frame_name(&mut self, id: FrameId, name: impl Into<String>) -> bool {
        match self.frames.get_mut(&id) {
            Some(f) => {
                f.name = name.into();
                true
            }
            None => false,
        }
    }

    /// Undelete a frame deleted with [`Self::delete_frame`]: `arg` is 1 for
    /// the most recently deleted, 2 for the one
    /// before it, and so on. The frame keeps its name, windows, point and
    /// its original frame id, and goes back where it was in the creation
    /// order. Returns it, or None if there is no such deleted frame.
    pub(crate) fn undelete_frame(&mut self, arg: usize) -> Option<FrameId> {
        if arg == 0 || arg > self.deleted.len() {
            return None;
        }
        let i = self.deleted.len() - arg;
        let (id, frame, at) = self.deleted.remove(i);
        debug_assert!(!self.frames.contains_key(&id));
        self.frames.insert(id, frame);
        let at = at.min(self.order.len());
        self.order.insert(at, id);
        Some(id)
    }

    /// How many frames could be undeleted (for the "No frame was
    /// undeleted" error).
    pub fn deleted_frame_count(&self) -> usize {
        self.deleted.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn split_layout_and_delete() {
        let mut ws = Windows::new(BufferId(0));
        ws.set_frame(frame(80, 24));
        let below = ws.split(Direction::Rows, (0, 0)).unwrap();
        let right = ws.split(Direction::Columns, (0, 0)).unwrap();
        let l = ws.layout(ws.frame());
        assert_eq!(
            l[0],
            (
                WindowId(0),
                Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 12
                }
            )
        );
        assert_eq!(
            l[1],
            (
                right,
                Rect {
                    x: 40,
                    y: 0,
                    width: 40,
                    height: 12
                }
            )
        );
        assert_eq!(
            l[2],
            (
                below,
                Rect {
                    x: 0,
                    y: 12,
                    width: 80,
                    height: 12
                }
            )
        );
        assert_eq!(ws.ids(), vec![WindowId(0), right, below]);
        assert_eq!(ws.nth_from_selected(1), right);
        assert_eq!(ws.nth_from_selected(-1), below);

        ws.delete(right).unwrap();
        assert_eq!(
            ws.layout(ws.frame())[0].1,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 12
            }
        );
        ws.delete_others();
        assert_eq!(ws.count(), 1);
        assert!(ws.delete(WindowId(0)).is_err());
    }

    #[test]
    fn too_small_to_split() {
        let mut ws = Windows::new(BufferId(0));
        ws.set_frame(frame(80, 3));
        assert!(ws.split(Direction::Rows, (0, 0)).is_err());
        assert!(ws.split(Direction::Columns, (0, 0)).is_ok());
    }

    #[test]
    fn resize_and_balance() {
        let mut ws = Windows::new(BufferId(0));
        ws.set_frame(frame(80, 24));
        let below = ws.split(Direction::Rows, (0, 0)).unwrap();
        ws.enlarge(Direction::Rows, 4).unwrap();
        assert_eq!(ws.layout(ws.frame())[0].1.height, 16);
        assert!(
            ws.enlarge(Direction::Columns, 4).is_err(),
            "no side-by-side split to move"
        );
        assert!(ws.enlarge(Direction::Rows, 100).is_err());
        ws.set_selected(below);
        ws.enlarge(Direction::Rows, 2).unwrap();
        assert_eq!(ws.layout(ws.frame())[1].1.height, 10);
        // Three stacked windows balance to thirds.
        ws.split(Direction::Rows, (0, 0)).unwrap();
        ws.balance();
        let heights: Vec<u16> = ws
            .layout(ws.frame())
            .iter()
            .map(|(_, r)| r.height)
            .collect();
        assert_eq!(heights, vec![8, 8, 8]);
    }

    #[test]
    fn frames_are_independent_and_share_windows_api() {
        let mut ws = Windows::new(BufferId(0));
        ws.set_frame(frame(80, 24));
        let f2 = ws.make_frame("F2".into(), BufferId(1), (0, 0));
        assert_eq!(ws.frame_count(), 2);
        assert_eq!(ws.selected_frame_id(), FrameId(0));
        assert_eq!(ws.count(), 1, "each frame starts with one window");

        // A split in frame 2 does not touch frame 1.
        ws.select_frame(f2);
        ws.set_frame(frame(100, 30));
        let w2 = ws.split(Direction::Rows, (0, 0)).unwrap();
        assert_eq!(ws.count(), 2);
        assert_eq!(ws.selected().buffer, BufferId(1));

        ws.select_frame(FrameId(0));
        assert_eq!(ws.count(), 1, "frame 1 still has a single window");
        let w1 = ws.selected_id();
        assert_ne!(w1, w2, "window ids are unique across frames");

        assert_eq!(ws.other_frame(1), f2);
        assert_eq!(ws.other_frame(-1), f2, "two frames cycle to each other");
        ws.delete_frame(f2).unwrap();
        assert_eq!(ws.frame_count(), 1);
        assert!(ws.delete_frame(FrameId(0)).is_err(), "not the last frame");
    }

    #[test]
    fn a_deleted_frame_can_be_undeleted() {
        let mut ws = Windows::new(BufferId(0));
        ws.set_frame(frame(80, 24));
        let f2 = ws.make_frame("F2".into(), BufferId(1), (3, 0));
        let f3 = ws.make_frame("F3".into(), BufferId(2), (0, 0));
        assert_eq!(ws.frame_ids(), vec![FrameId(0), f2, f3]);

        ws.delete_frame(f2).unwrap();
        assert_eq!(ws.frame_count(), 2);
        assert_eq!(ws.deleted_frame_count(), 1);

        // Undelete the most recent: the same id, name and buffer, at its
        // old place in the creation order.
        assert_eq!(ws.undelete_frame(1), Some(f2));
        assert_eq!(ws.frame_count(), 3);
        assert_eq!(ws.frame_ids(), vec![FrameId(0), f2, f3]);
        assert_eq!(ws.frame_name(f2), Some("F2"));
        let win = ws.get(ws.frame_selected_id(f2)).unwrap();
        assert_eq!(win.buffer, BufferId(1));
        assert_eq!(win.saved_point.0, 3, "the window keeps its point");

        // The 2nd most recent, and no more.
        ws.delete_frame(f2).unwrap();
        ws.delete_frame(f3).unwrap();
        assert_eq!(ws.deleted_frame_count(), 2);
        assert_eq!(ws.undelete_frame(2), Some(f2), "two back is f2");
        assert_eq!(ws.undelete_frame(2), None, "only one is left");
        assert_eq!(ws.undelete_frame(1), Some(f3));
        assert_eq!(ws.deleted_frame_count(), 0);
        assert_eq!(ws.undelete_frame(0), None, "0 is never valid");
    }

    #[test]
    fn delete_other_frames_records_each_for_undelete() {
        let mut ws = Windows::new(BufferId(0));
        ws.set_frame(frame(80, 24));
        ws.make_frame("F2".into(), BufferId(1), (0, 0));
        ws.make_frame("F3".into(), BufferId(2), (0, 0));
        ws.delete_other_frames();
        assert_eq!(ws.frame_count(), 1);
        assert_eq!(ws.deleted_frame_count(), 2);
        // Bringing them back restores the whole set.
        assert!(ws.undelete_frame(1).is_some());
        assert!(ws.undelete_frame(1).is_some());
        assert_eq!(ws.frame_count(), 3);
    }
}
