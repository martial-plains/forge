// SPDX-License-Identifier: MIT OR Apache-2.0
//! JSON Canvas (`.canvas`, the open format Obsidian's Canvas uses): the
//! model, the edits made to it, how it is drawn, and how a pointer moves
//! its parts about.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as J};

use forge_canvas::{Op, Palette, Scene, Stroke, Transform};

pub const GRID: f32 = 20.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What the format has that this doesn't use, kept as it was.
    #[serde(flatten)]
    pub extra: Map<String, J>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    #[serde(rename = "fromNode")]
    pub from_node: String,
    #[serde(rename = "fromSide", default, skip_serializing_if = "Option::is_none")]
    pub from_side: Option<String>,
    #[serde(rename = "fromEnd", default, skip_serializing_if = "Option::is_none")]
    pub from_end: Option<String>,
    #[serde(rename = "toNode")]
    pub to_node: String,
    #[serde(rename = "toSide", default, skip_serializing_if = "Option::is_none")]
    pub to_side: Option<String>,
    #[serde(rename = "toEnd", default, skip_serializing_if = "Option::is_none")]
    pub to_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, J>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Canvas {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(flatten)]
    pub extra: Map<String, J>,
}

impl Canvas {
    pub fn parse(text: &str) -> Result<Canvas, String> {
        if text.trim().is_empty() {
            return Ok(Canvas::default());
        }
        serde_json::from_str(text).map_err(|e| format!("not a JSON Canvas: {e}"))
    }

    /// Tab-indented, as Obsidian writes it.
    pub fn to_text(&self) -> String {
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b"\t");
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        if self.serialize(&mut ser).is_err() {
            return String::new();
        }
        String::from_utf8(out).unwrap_or_default()
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}

/// A canvas being edited.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub canvas: Canvas,
    pub selected: Option<Sel>,
    /// `connect` was asked for from this node: the next node clicked is the other end.
    pub connect_from: Option<String>,
    pub snap: bool,
    counter: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sel {
    Node(String),
    Edge(String),
}

impl Board {
    pub fn new(canvas: Canvas) -> Board {
        Board {
            canvas,
            snap: true,
            ..Default::default()
        }
    }

    fn fresh_id(&mut self) -> String {
        loop {
            self.counter += 1;
            let h = (self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (self.canvas.nodes.len() as u64) << 32
                ^ self.canvas.edges.len() as u64)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9);
            let id = format!("{:016x}", h);
            if self.canvas.node(&id).is_none() && !self.canvas.edges.iter().any(|e| e.id == id) {
                return id;
            }
        }
    }

    fn snapped(&self, v: f32) -> f32 {
        if self.snap {
            (v / GRID).round() * GRID
        } else {
            v.round()
        }
    }

    fn blank(&mut self, kind: &str, x: f32, y: f32, w: f32, h: f32) -> Node {
        Node {
            id: self.fresh_id(),
            kind: kind.into(),
            x: self.snapped(x),
            y: self.snapped(y),
            width: w,
            height: h,
            color: None,
            text: None,
            file: None,
            subpath: None,
            url: None,
            label: None,
            extra: Map::new(),
        }
    }

    fn push(&mut self, n: Node) -> String {
        let id = n.id.clone();
        self.canvas.nodes.push(n);
        self.selected = Some(Sel::Node(id.clone()));
        id
    }

    /// New nodes are centred on (x, y).
    pub fn add_text(&mut self, x: f32, y: f32, text: &str) -> String {
        let mut n = self.blank("text", x - 125.0, y - 50.0, 250.0, 100.0);
        n.text = Some(text.into());
        self.push(n)
    }

    pub fn add_file(&mut self, x: f32, y: f32, file: &str) -> String {
        let mut n = self.blank("file", x - 200.0, y - 150.0, 400.0, 300.0);
        n.file = Some(file.into());
        self.push(n)
    }

    pub fn add_link(&mut self, x: f32, y: f32, url: &str) -> String {
        let mut n = self.blank("link", x - 200.0, y - 100.0, 400.0, 200.0);
        n.url = Some(url.into());
        self.push(n)
    }

    pub fn add_group(&mut self, x: f32, y: f32, label: &str) -> String {
        let mut n = self.blank("group", x - 300.0, y - 200.0, 600.0, 400.0);
        n.label = Some(label.into());
        // Groups sit behind what they hold.
        let id = n.id.clone();
        self.canvas.nodes.insert(0, n);
        self.selected = Some(Sel::Node(id.clone()));
        id
    }

    /// Nodes wholly inside group `id`.
    pub fn members(&self, id: &str) -> Vec<String> {
        let Some(g) = self.canvas.node(id).filter(|g| g.kind == "group") else {
            return Vec::new();
        };
        self.canvas
            .nodes
            .iter()
            .filter(|n| {
                n.id != g.id
                    && n.x >= g.x
                    && n.y >= g.y
                    && n.x + n.width <= g.x + g.width
                    && n.y + n.height <= g.y + g.height
            })
            .map(|n| n.id.clone())
            .collect()
    }

    /// Move a node (and a group's members) by (dx, dy).
    pub fn move_by(&mut self, id: &str, dx: f32, dy: f32) {
        let mut ids = self.members(id);
        ids.push(id.to_string());
        for i in ids {
            if let Some(n) = self.canvas.node_mut(&i) {
                n.x += dx;
                n.y += dy;
            }
        }
    }

    pub fn place(&mut self, id: &str, x: f32, y: f32) {
        let (nx, ny) = (self.snapped(x), self.snapped(y));
        if let Some(n) = self.canvas.node(id) {
            let (dx, dy) = (nx - n.x, ny - n.y);
            self.move_by(id, dx, dy);
        }
    }

    pub fn resize(&mut self, id: &str, w: f32, h: f32) {
        let (w, h) = (
            self.snapped(w).max(GRID * 2.0),
            self.snapped(h).max(GRID * 2.0),
        );
        if let Some(n) = self.canvas.node_mut(id) {
            n.width = w;
            n.height = h;
        }
    }

    pub fn delete(&mut self, id: &str) {
        self.canvas.nodes.retain(|n| n.id != id);
        self.canvas
            .edges
            .retain(|e| e.id != id && e.from_node != id && e.to_node != id);
        let gone = match &self.selected {
            Some(Sel::Node(s)) => self.canvas.node(s).is_none(),
            Some(Sel::Edge(s)) => !self.canvas.edges.iter().any(|e| &e.id == s),
            None => false,
        };
        if gone {
            self.selected = None;
        }
    }

    pub fn delete_selected(&mut self) -> bool {
        match self.selected.clone() {
            Some(Sel::Node(id) | Sel::Edge(id)) => {
                self.delete(&id);
                true
            }
            None => false,
        }
    }

    pub fn set_text(&mut self, id: &str, text: &str) {
        if let Some(n) = self.canvas.node_mut(id) {
            match n.kind.as_str() {
                "group" => n.label = Some(text.into()),
                "link" => n.url = Some(text.into()),
                "file" => n.file = Some(text.into()),
                _ => n.text = Some(text.into()),
            }
        } else if let Some(e) = self.canvas.edges.iter_mut().find(|e| e.id == id) {
            e.label = (!text.is_empty()).then(|| text.into());
        }
    }

    pub fn set_color(&mut self, id: &str, color: Option<&str>) {
        let c = color
            .map(str::to_string)
            .filter(|c| !c.is_empty() && c != "0");
        if let Some(n) = self.canvas.node_mut(id) {
            n.color = c;
        } else if let Some(e) = self.canvas.edges.iter_mut().find(|e| e.id == id) {
            e.color = c;
        }
    }

    fn side_toward(from: &Node, to: &Node) -> (&'static str, &'static str) {
        let (fx, fy) = (from.x + from.width / 2.0, from.y + from.height / 2.0);
        let (tx, ty) = (to.x + to.width / 2.0, to.y + to.height / 2.0);
        let (dx, dy) = (tx - fx, ty - fy);
        if dx.abs() * from.height.max(1.0) > dy.abs() * from.width.max(1.0) {
            if dx > 0.0 {
                ("right", "left")
            } else {
                ("left", "right")
            }
        } else if dy > 0.0 {
            ("bottom", "top")
        } else {
            ("top", "bottom")
        }
    }

    pub fn connect(&mut self, from: &str, to: &str) -> Option<String> {
        if from == to {
            return None;
        }
        let (a, b) = (self.canvas.node(from)?, self.canvas.node(to)?);
        let (fs, ts) = Self::side_toward(a, b);
        let id = self.fresh_id();
        self.canvas.edges.push(Edge {
            id: id.clone(),
            from_node: from.into(),
            from_side: Some(fs.into()),
            from_end: None,
            to_node: to.into(),
            to_side: Some(ts.into()),
            to_end: None,
            color: None,
            label: None,
            extra: Map::new(),
        });
        self.selected = Some(Sel::Edge(id.clone()));
        Some(id)
    }

    /// (x, y, zoom) that shows everything in a `w` x `h` frame.
    pub fn fit(&self, w: f32, h: f32) -> (f32, f32, f32) {
        if self.canvas.nodes.is_empty() {
            return (-w / 2.0, -h / 2.0, 1.0);
        }
        let x0 = self
            .canvas
            .nodes
            .iter()
            .map(|n| n.x)
            .fold(f32::MAX, f32::min);
        let y0 = self
            .canvas
            .nodes
            .iter()
            .map(|n| n.y)
            .fold(f32::MAX, f32::min);
        let x1 = self
            .canvas
            .nodes
            .iter()
            .map(|n| n.x + n.width)
            .fold(f32::MIN, f32::max);
        let y1 = self
            .canvas
            .nodes
            .iter()
            .map(|n| n.y + n.height)
            .fold(f32::MIN, f32::max);
        let zoom = ((w - 80.0) / (x1 - x0).max(1.0))
            .min((h - 80.0) / (y1 - y0).max(1.0))
            .clamp(0.05, 1.5);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        (cx - w / zoom / 2.0, cy - h / zoom / 2.0, zoom)
    }
}

pub fn preset(color: &str) -> &str {
    match color {
        "1" => "#fb464c",
        "2" => "#e9973f",
        "3" => "#e0de71",
        "4" => "#44cf6e",
        "5" => "#53dfdd",
        "6" => "#a882ff",
        c => c,
    }
}

const IMAGE_EXT: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

pub fn is_image(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// What a file node can show of its file.
pub trait Files {
    /// An absolute path for a vault-relative file name.
    fn resolve(&self, file: &str) -> Option<String>;
    /// The start of a text file.
    fn preview(&self, file: &str) -> Option<String>;
}

pub struct NoFiles;

impl Files for NoFiles {
    fn resolve(&self, _: &str) -> Option<String> {
        None
    }
    fn preview(&self, _: &str) -> Option<String> {
        None
    }
}

fn anchor(n: &Node, side: &str) -> (f32, f32, f32, f32) {
    match side {
        "top" => (n.x + n.width / 2.0, n.y, 0.0, -1.0),
        "bottom" => (n.x + n.width / 2.0, n.y + n.height, 0.0, 1.0),
        "left" => (n.x, n.y + n.height / 2.0, -1.0, 0.0),
        _ => (n.x + n.width, n.y + n.height / 2.0, 1.0, 0.0),
    }
}

fn tinted(color: &str, alpha: f32) -> String {
    let c = preset(color);
    match c.strip_prefix('#') {
        Some(h) if h.len() == 6 => {
            let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(128);
            format!("rgba({},{},{},{alpha})", v(0), v(2), v(4))
        }
        _ => format!("rgba(128,128,128,{alpha})"),
    }
}

fn text_op(x: f32, y: f32, s: &str, size: f32, color: &str, w: f32, lines: Option<u32>) -> Op {
    let mut t = Op::text(x, y, s, size, color).wrapped(w);
    if let Op::Text { max_lines, .. } = &mut t {
        *max_lines = lines;
    }
    t
}

/// Draw it. Node `n` is `node:<id>`, edge `e` is `edge:<id>`, and the
/// selected node's bottom-right handle is `resize:<id>`.
pub fn scene(board: &Board, width: f32, height: f32, files: &dyn Files) -> Scene {
    let c = &board.canvas;
    let pal = Palette::current();
    let mut s = Scene::new(width, height).background(&pal.bg);
    let sel_node = match &board.selected {
        Some(Sel::Node(id)) => Some(id.as_str()),
        _ => None,
    };
    let sel_edge = match &board.selected {
        Some(Sel::Edge(id)) => Some(id.as_str()),
        _ => None,
    };
    let border = |n: &Node| {
        n.color
            .as_deref()
            .map_or(pal.line.clone(), |c| preset(c).to_string())
    };
    let draw = |n: &Node| -> Op {
        let col = border(n);
        let selected = sel_node == Some(n.id.as_str());
        let id = format!("node:{}", n.id);
        let mut ops = Vec::new();
        let stroke = Stroke::new(
            if selected { &pal.strong } else { &col },
            if selected { 3.0 } else { 2.0 },
        );
        if n.kind == "group" {
            let mut r = Op::rect(0.0, 0.0, n.width, n.height)
                .fill(tinted(n.color.as_deref().unwrap_or("0"), 0.10).as_str())
                .stroked(stroke)
                .rounded(14.0);
            if let Op::Rect {
                stroke: Some(st), ..
            } = &mut r
            {
                st.dash = vec![10.0, 6.0];
            }
            ops.push(r);
            if let Some(l) = &n.label {
                ops.push(text_op(10.0, -26.0, l, 18.0, &pal.soft, n.width, Some(1)));
            }
        } else {
            let fill = n
                .color
                .as_deref()
                .map_or(pal.card.clone(), |c| tinted(c, 0.22));
            ops.push(
                Op::rect(0.0, 0.0, n.width, n.height)
                    .fill(fill.as_str())
                    .stroked(stroke)
                    .rounded(10.0),
            );
            let inner = n.width - 24.0;
            match n.kind.as_str() {
                "text" => ops.push(text_op(
                    12.0,
                    10.0,
                    n.text.as_deref().unwrap_or(""),
                    16.0,
                    &pal.fg,
                    inner,
                    Some(((n.height - 20.0) / 21.0).max(1.0) as u32),
                )),
                "file" => {
                    let f = n.file.as_deref().unwrap_or("");
                    let abs = files.resolve(f);
                    if is_image(f) {
                        match abs {
                            Some(p) => ops.push(
                                Op::group(vec![Op::image(&p, 0.0, 0.0, n.width, n.height)])
                                    .clipped(0.0, 0.0, n.width, n.height),
                            ),
                            None => ops.push(text_op(
                                12.0,
                                10.0,
                                &format!("🖼 {f}"),
                                14.0,
                                &pal.soft,
                                inner,
                                Some(2),
                            )),
                        }
                    } else {
                        let name = f.rsplit('/').next().unwrap_or(f);
                        ops.push(text_op(
                            12.0,
                            10.0,
                            &format!("📄 {name}"),
                            17.0,
                            &pal.fg,
                            inner,
                            Some(1),
                        ));
                        if let Some(p) = files.preview(f) {
                            ops.push(text_op(
                                12.0,
                                40.0,
                                &p,
                                13.0,
                                &pal.soft,
                                inner,
                                Some(((n.height - 50.0) / 17.0).max(1.0) as u32),
                            ));
                        }
                    }
                }
                "link" => {
                    ops.push(text_op(
                        12.0,
                        10.0,
                        &format!("🔗 {}", n.url.as_deref().unwrap_or("")),
                        15.0,
                        pal.accent(0),
                        inner,
                        Some(3),
                    ));
                }
                _ => ops.push(text_op(
                    12.0,
                    10.0,
                    &format!("({})", n.kind),
                    14.0,
                    &pal.soft,
                    inner,
                    Some(1),
                )),
            }
        }
        Op::group(ops)
            .transformed(Transform::translate(n.x, n.y))
            .with_id(&id)
    };
    for n in c.nodes.iter().filter(|n| n.kind == "group") {
        s.push(draw(n));
    }
    // Edges between the groups and the cards: behind cards, over groups.
    let mut edge_ops = Vec::new();
    let mut label_ops = Vec::new();
    for e in &c.edges {
        let (Some(a), Some(b)) = (c.node(&e.from_node), c.node(&e.to_node)) else {
            continue;
        };
        let (fs, ts) = Board::side_toward(a, b);
        let (ax, ay, anx, any) = anchor(a, e.from_side.as_deref().unwrap_or(fs));
        let (bx, by, bnx, bny) = anchor(b, e.to_side.as_deref().unwrap_or(ts));
        let reach = (((bx - ax).powi(2) + (by - ay).powi(2)).sqrt() * 0.4).clamp(30.0, 160.0);
        let d = format!(
            "M {ax} {ay} C {} {} {} {} {bx} {by}",
            ax + anx * reach,
            ay + any * reach,
            bx + bnx * reach,
            by + bny * reach
        );
        let selected = sel_edge == Some(e.id.as_str());
        let col = if selected {
            pal.strong.clone()
        } else {
            e.color
                .as_deref()
                .map_or(pal.dim.clone(), |c| preset(c).to_string())
        };
        let mut st = Stroke::new(&col, if selected { 3.5 } else { 2.5 });
        st.arrow_end = e.to_end.as_deref() != Some("none");
        st.arrow_start = e.from_end.as_deref() == Some("arrow");
        edge_ops.push(Op::path(&d).stroked(st).with_id(&format!("edge:{}", e.id)));
        if let Some(l) = &e.label {
            let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
            let w = forge_canvas::measure(l, 14.0, false).0 + 16.0;
            label_ops.push(
                Op::rect(mx - w / 2.0, my - 12.0, w, 24.0)
                    .fill(&pal.bg)
                    .stroked(Stroke::new(&col, 1.0))
                    .rounded(6.0)
                    .with_id(&format!("edge:{}", e.id)),
            );
            let mut t = Op::text(mx, my - 9.0, l, 14.0, &pal.fg).with_id(&format!("edge:{}", e.id));
            if let Op::Text { align, .. } = &mut t {
                *align = forge_canvas::Align::Center;
            }
            label_ops.push(t);
        }
    }
    s.ops.extend(edge_ops);
    for n in c.nodes.iter().filter(|n| n.kind != "group") {
        s.push(draw(n));
    }
    s.ops.extend(label_ops);
    if let Some(n) = sel_node.and_then(|id| c.node(id)) {
        s.push(
            Op::rect(n.x + n.width - 9.0, n.y + n.height - 9.0, 18.0, 18.0)
                .fill(&pal.strong)
                .stroked(Stroke::new(pal.accent(0), 3.0))
                .rounded(4.0)
                .with_id(&format!("resize:{}", n.id)),
        );
    }
    if board.connect_from.is_some() {
        s.push(Op::text(
            16.0,
            12.0,
            "Connect: click the node to join to (Esc cancels)",
            16.0,
            pal.accent(1),
        ));
    }
    s
}

// --- pointer ----------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Event {
    pub kind: String,
    #[serde(default)]
    pub button: u8,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub shift: bool,
    /// The wheel's step on a `scroll` (positive is up).
    #[serde(default)]
    pub dy: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The picture changed.
    Redraw,
    /// The document changed: save it back to the file's text.
    Changed,
    /// Ask for new text for this node or edge.
    Edit(String),
    OpenFile(String),
    OpenUrl(String),
    Message(String),
}

#[derive(Debug, Clone)]
enum Drag {
    Move { id: String, last: (f32, f32) },
    Resize { id: String },
}

#[derive(Debug, Clone, Default)]
pub struct Pointer {
    drag: Option<Drag>,
    moved: bool,
}

impl Pointer {
    pub fn handle(&mut self, b: &mut Board, ev: &Event) -> Vec<Action> {
        let part = ev.id.as_deref().and_then(|i| i.split_once(':'));
        match (ev.kind.as_str(), ev.button) {
            ("press", 1) => {
                self.moved = false;
                match part {
                    Some(("resize", id)) => {
                        self.drag = Some(Drag::Resize { id: id.into() });
                        vec![]
                    }
                    Some(("node", id)) => {
                        if let Some(from) = b.connect_from.take() {
                            return if b.connect(&from, id).is_some() {
                                vec![Action::Changed, Action::Redraw]
                            } else {
                                vec![
                                    Action::Message("Can't connect a node to itself".into()),
                                    Action::Redraw,
                                ]
                            };
                        }
                        b.selected = Some(Sel::Node(id.into()));
                        self.drag = Some(Drag::Move {
                            id: id.into(),
                            last: (ev.x, ev.y),
                        });
                        vec![Action::Redraw]
                    }
                    Some(("edge", id)) => {
                        b.selected = Some(Sel::Edge(id.into()));
                        self.drag = None;
                        vec![Action::Redraw]
                    }
                    _ => {
                        b.selected = None;
                        b.connect_from = None;
                        self.drag = None;
                        vec![Action::Redraw]
                    }
                }
            }
            ("drag", 1) => match self.drag.clone() {
                Some(Drag::Move { id, last }) => {
                    let (dx, dy) = (ev.x - last.0, ev.y - last.1);
                    if dx == 0.0 && dy == 0.0 {
                        return vec![];
                    }
                    self.moved = true;
                    b.move_by(&id, dx, dy);
                    self.drag = Some(Drag::Move {
                        id,
                        last: (ev.x, ev.y),
                    });
                    vec![Action::Redraw]
                }
                Some(Drag::Resize { id }) => {
                    if let Some(n) = b.canvas.node(&id) {
                        let (w, h) = (ev.x - n.x, ev.y - n.y);
                        b.resize(&id, w, h);
                        self.moved = true;
                    }
                    vec![Action::Redraw]
                }
                None => vec![],
            },
            ("release", 1) => {
                let drag = self.drag.take();
                if !self.moved {
                    return vec![];
                }
                self.moved = false;
                if let Some(Drag::Move { id, .. }) = drag {
                    // Land on the grid.
                    if let Some(n) = b.canvas.node(&id) {
                        let (x, y) = (n.x, n.y);
                        b.place(&id, x, y);
                    }
                }
                vec![Action::Changed, Action::Redraw]
            }
            ("double", 1) => match part {
                Some(("node", id)) => match b.canvas.node(id) {
                    Some(n) if n.kind == "file" => {
                        vec![Action::OpenFile(n.file.clone().unwrap_or_default())]
                    }
                    Some(n) if n.kind == "link" => {
                        vec![Action::OpenUrl(n.url.clone().unwrap_or_default())]
                    }
                    Some(_) => vec![Action::Edit(id.into())],
                    None => vec![],
                },
                Some(("edge", id)) => vec![Action::Edit(id.into())],
                _ => {
                    let id = b.add_text(ev.x, ev.y, "");
                    vec![Action::Changed, Action::Edit(id)]
                }
            },
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
	"nodes": [
		{"id": "a", "type": "text", "text": "Hello", "x": 0, "y": 0, "width": 200, "height": 100, "color": "4", "styleAttributes": {"x": 1}},
		{"id": "b", "type": "file", "file": "notes/Idea.md", "x": 400, "y": 0, "width": 300, "height": 200},
		{"id": "g", "type": "group", "label": "Box", "x": -50, "y": -50, "width": 300, "height": 300},
		{"id": "l", "type": "link", "url": "https://example.com", "x": 0, "y": 400, "width": 250, "height": 100}
	],
	"edges": [
		{"id": "e1", "fromNode": "a", "fromSide": "right", "toNode": "b", "toSide": "left", "label": "leads to", "color": "#ff0000"}
	],
	"metadata": {"version": "1.0"}
}"##;

    fn board() -> Board {
        Board::new(Canvas::parse(SAMPLE).unwrap())
    }

    #[test]
    fn a_canvas_file_round_trips_and_keeps_unknown_fields() {
        let c = Canvas::parse(SAMPLE).unwrap();
        assert_eq!(c.nodes.len(), 4);
        assert_eq!(c.edges[0].label.as_deref(), Some("leads to"));
        let text = c.to_text();
        assert!(
            text.contains("\n\t\t{") || text.contains("\t"),
            "tab-indented"
        );
        assert!(text.contains("styleAttributes") && text.contains("metadata"));
        assert_eq!(Canvas::parse(&text).unwrap(), c);
        assert_eq!(Canvas::parse("").unwrap(), Canvas::default());
        assert!(Canvas::parse("[1]").is_err());
        assert!(!text.contains("\"subpath\""), "absent fields stay absent");
    }

    #[test]
    fn edits_add_move_resize_connect_delete() {
        let mut b = board();
        let t = b.add_text(1000.0, 1000.0, "new");
        let n = b.canvas.node(&t).unwrap();
        assert_eq!(
            (n.x, n.y, n.width),
            (880.0, 960.0, 250.0),
            "centred on the point, on the grid"
        );
        assert_eq!(b.selected, Some(Sel::Node(t.clone())));
        b.resize(&t, 301.0, 99.0);
        assert_eq!(
            (
                b.canvas.node(&t).unwrap().width,
                b.canvas.node(&t).unwrap().height
            ),
            (300.0, 100.0)
        );
        let e = b.connect("a", &t).unwrap();
        let edge = b.canvas.edges.iter().find(|x| x.id == e).unwrap();
        assert_eq!(
            (edge.from_side.as_deref(), edge.to_side.as_deref()),
            (Some("bottom"), Some("top"))
        );
        assert!(b.connect("a", "a").is_none() && b.connect("a", "zzz").is_none());
        let ids: std::collections::BTreeSet<_> = b
            .canvas
            .nodes
            .iter()
            .map(|n| n.id.clone())
            .chain(b.canvas.edges.iter().map(|e| e.id.clone()))
            .collect();
        assert_eq!(
            ids.len(),
            b.canvas.nodes.len() + b.canvas.edges.len(),
            "ids are unique"
        );
        b.set_text(&t, "changed");
        assert_eq!(b.canvas.node(&t).unwrap().text.as_deref(), Some("changed"));
        b.set_color(&t, Some("2"));
        assert_eq!(b.canvas.node(&t).unwrap().color.as_deref(), Some("2"));
        b.set_color(&t, Some("0"));
        assert_eq!(b.canvas.node(&t).unwrap().color, None);
        b.delete(&t);
        assert!(
            b.canvas.node(&t).is_none() && !b.canvas.edges.iter().any(|x| x.id == e),
            "edges go with their nodes"
        );
        assert_eq!(b.selected, None);
        let g = b.add_group(0.0, 0.0, "G");
        assert_eq!(b.canvas.nodes[0].id, g, "groups go to the back");
    }

    #[test]
    fn moving_a_group_moves_what_it_holds() {
        let mut b = board();
        assert_eq!(b.members("g"), ["a"], "b and l are outside");
        b.move_by("g", 100.0, 20.0);
        assert_eq!(
            (b.canvas.node("a").unwrap().x, b.canvas.node("a").unwrap().y),
            (100.0, 20.0)
        );
        assert_eq!(b.canvas.node("b").unwrap().x, 400.0);
    }

    struct Fs;
    impl Files for Fs {
        fn resolve(&self, f: &str) -> Option<String> {
            Some(format!("/vault/{f}"))
        }
        fn preview(&self, _: &str) -> Option<String> {
            Some("first lines of the note".into())
        }
    }

    #[test]
    fn it_draws_every_part_with_ids() {
        let mut b = board();
        b.selected = Some(Sel::Node("a".into()));
        let s = scene(&b, 1000.0, 700.0, &Fs);
        for id in [
            "node:a", "node:b", "node:g", "node:l", "edge:e1", "resize:a",
        ] {
            assert!(s.bounds_of(id).is_some(), "{id} is drawn");
        }
        // A click in a card hits the card, the empty page hits nothing, and
        // the card beats the group behind it.
        assert_eq!(s.hit(100.0, 50.0).as_deref(), Some("node:a"));
        assert_eq!(s.hit(-40.0, 240.0).as_deref(), Some("node:g"));
        assert_eq!(s.hit(900.0, 600.0), None);
        let svg = s.to_svg();
        assert!(svg.contains("leads to") && svg.contains("marker-end"));
        assert!(
            scene(&Board::new(Canvas::default()), 100.0, 100.0, &NoFiles)
                .ops
                .is_empty()
        );
        let (x, y, z) = b.fit(1000.0, 700.0);
        assert!(z > 0.5 && z <= 1.5 && x < 0.0 && y < 0.0, "{x} {y} {z}");
    }

    fn ev(kind: &str, x: f32, y: f32, id: Option<&str>) -> Event {
        Event {
            kind: kind.into(),
            button: 1,
            x,
            y,
            id: id.map(Into::into),
            shift: false,
            dy: 0.0,
        }
    }

    #[test]
    fn the_pointer_selects_drags_resizes_and_connects() {
        let mut b = board();
        let mut p = Pointer::default();
        assert_eq!(
            p.handle(&mut b, &ev("press", 50.0, 50.0, Some("node:a"))),
            [Action::Redraw]
        );
        assert_eq!(b.selected, Some(Sel::Node("a".into())));
        p.handle(&mut b, &ev("drag", 77.0, 63.0, Some("node:a")));
        assert_eq!(
            (b.canvas.node("a").unwrap().x, b.canvas.node("a").unwrap().y),
            (27.0, 13.0)
        );
        let acts = p.handle(&mut b, &ev("release", 77.0, 63.0, Some("node:a")));
        assert!(acts.contains(&Action::Changed));
        assert_eq!(
            (b.canvas.node("a").unwrap().x, b.canvas.node("a").unwrap().y),
            (20.0, 20.0),
            "lands on the grid"
        );
        // A press and release without moving changes nothing.
        p.handle(&mut b, &ev("press", 50.0, 50.0, Some("node:a")));
        assert!(
            p.handle(&mut b, &ev("release", 50.0, 50.0, Some("node:a")))
                .is_empty()
        );
        // Resize by the handle.
        p.handle(&mut b, &ev("press", 220.0, 120.0, Some("resize:a")));
        p.handle(&mut b, &ev("drag", 300.0, 200.0, Some("resize:a")));
        assert_eq!(
            (
                b.canvas.node("a").unwrap().width,
                b.canvas.node("a").unwrap().height
            ),
            (280.0, 180.0)
        );
        p.handle(&mut b, &ev("release", 300.0, 200.0, None));
        // Connect.
        b.connect_from = Some("a".into());
        let acts = p.handle(&mut b, &ev("press", 500.0, 50.0, Some("node:l")));
        assert!(acts.contains(&Action::Changed) && b.connect_from.is_none());
        assert_eq!(b.canvas.edges.len(), 2);
        // Empty space deselects; double-click there adds a note to edit.
        p.handle(&mut b, &ev("press", 900.0, 900.0, None));
        assert_eq!(b.selected, None);
        let acts = p.handle(&mut b, &ev("double", 900.0, 900.0, None));
        assert!(matches!(acts.last(), Some(Action::Edit(_))));
        assert_eq!(b.canvas.nodes.len(), 5);
        // Double-click: text edits, files and links open.
        assert_eq!(
            p.handle(&mut b, &ev("double", 0.0, 0.0, Some("node:a"))),
            [Action::Edit("a".into())]
        );
        assert_eq!(
            p.handle(&mut b, &ev("double", 0.0, 0.0, Some("node:b"))),
            [Action::OpenFile("notes/Idea.md".into())]
        );
        assert_eq!(
            p.handle(&mut b, &ev("double", 0.0, 0.0, Some("node:l"))),
            [Action::OpenUrl("https://example.com".into())]
        );
        assert_eq!(
            p.handle(&mut b, &ev("double", 0.0, 0.0, Some("edge:e1"))),
            [Action::Edit("e1".into())]
        );
    }
}
