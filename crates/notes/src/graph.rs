// SPDX-License-Identifier: MIT OR Apache-2.0
//! The link graph, in the manner of Obsidian's graph view: every note a
//! dot, every link a line; the *global* graph is the whole vault, the
//! *local* graph is the neighbourhood of one note.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use forge_canvas::{Op, Palette, Scene, Stroke};

use crate::vault::Index;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Note(usize),
    Tag,
    /// A link to a note that doesn't exist (yet).
    Unresolved,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub label: String,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub pinned: bool,
    pub degree: usize,
    /// Which colour: the folder's, for notes.
    pub group: usize,
    /// BFS distance from the centre (local graph).
    pub depth: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<(usize, usize)>,
    pub center: Option<usize>,
    /// Notes left out because the neighbourhood was too big to draw.
    pub hidden: usize,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Tags as nodes joined to the notes that have them.
    pub tags: bool,
    /// Notes with no links at all.
    pub orphans: bool,
    pub unresolved: bool,
    /// Only notes under this folder.
    pub folder: Option<String>,
    /// Only notes with this tag.
    pub tag: Option<String>,
    pub max_nodes: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { tags: false, orphans: true, unresolved: false, folder: None, tag: None, max_nodes: 1500 }
    }
}

/// Whether a note passes the folder and tag filters. The folder prefix is
/// worked out once, not per note: this runs over every note in the vault.
fn keeper<'a>(index: &'a Index, o: &'a Options) -> impl Fn(usize) -> bool + 'a {
    let folder = o.folder.as_deref().map(|f| f.trim_matches('/')).filter(|f| !f.is_empty());
    let prefix = folder.map(|f| format!("{f}/"));
    move |i| {
        let n = &index.notes[i];
        folder.is_none_or(|f| n.folder == f || prefix.as_deref().is_some_and(|p| n.folder.starts_with(p))) && o.tag.as_ref().is_none_or(|t| n.has_tag(t))
    }
}

struct Builder<'a> {
    index: &'a Index,
    g: Graph,
    of_note: BTreeMap<usize, usize>,
    of_name: BTreeMap<String, usize>,
    folders: BTreeMap<String, usize>,
    seen: std::collections::HashSet<(usize, usize)>,
}

impl<'a> Builder<'a> {
    fn new(index: &'a Index) -> Self {
        Builder { index, g: Graph::default(), of_note: BTreeMap::new(), of_name: BTreeMap::new(), folders: BTreeMap::new(), seen: Default::default() }
    }

    fn note(&mut self, i: usize) -> usize {
        if let Some(&n) = self.of_note.get(&i) {
            return n;
        }
        let note = &self.index.notes[i];
        let next = self.folders.len();
        let group = *self.folders.entry(note.folder.split('/').next().unwrap_or("").to_string()).or_insert(next);
        self.g.nodes.push(Node { label: note.name.clone(), kind: Kind::Note(i), x: 0.0, y: 0.0, pinned: false, degree: 0, group, depth: 0 });
        self.of_note.insert(i, self.g.nodes.len() - 1);
        self.g.nodes.len() - 1
    }

    fn named(&mut self, key: String, label: String, kind: Kind) -> usize {
        if let Some(&n) = self.of_name.get(&key) {
            return n;
        }
        self.g.nodes.push(Node { label, kind, x: 0.0, y: 0.0, pinned: false, degree: 0, group: usize::MAX, depth: 0 });
        self.of_name.insert(key, self.g.nodes.len() - 1);
        self.g.nodes.len() - 1
    }

    fn edge(&mut self, a: usize, b: usize) {
        if a != b {
            let e = (a.min(b), a.max(b));
            if self.seen.insert(e) {
                self.g.edges.push(e);
            }
        }
    }

    fn finish(mut self) -> Graph {
        for &(a, b) in &self.g.edges {
            self.g.nodes[a].degree += 1;
            self.g.nodes[b].degree += 1;
        }
        self.g
    }
}

/// The most notes a local graph draws.
pub const LOCAL_CAP: usize = 250;

/// The whole vault. A vault bigger than `max_nodes` can't be drawn as dots
/// anyway, so it shows the best-connected notes (and the links among them),
/// and `hidden` says how many were left out; a folder or tag filter narrows
/// it to look at the rest.
pub fn global(index: &Index, o: &Options) -> Graph {
    let mut b = Builder::new(index);
    let keep = keeper(index, o);
    let linked = |i: usize| !index.outgoing(i).is_empty() || !index.backlinks(i).is_empty() || (o.unresolved && !index.unresolved(i).is_empty()) || (o.tags && !index.notes[i].tags.is_empty());
    let mut kept: Vec<usize> = (0..index.notes.len()).filter(|&i| keep(i) && (o.orphans || linked(i))).collect();
    let total = kept.len();
    if total > o.max_nodes {
        let score = |i: usize| index.outgoing(i).len() + index.backlinks(i).len();
        // The `max_nodes` highest scores, ties broken by position so the
        // same vault always draws the same notes.
        kept.select_nth_unstable_by_key(o.max_nodes, |&i| (std::cmp::Reverse(score(i)), i));
        kept.truncate(o.max_nodes);
        kept.sort_unstable();
    }
    b.g.hidden = total - kept.len();
    let mut shown = vec![false; index.notes.len()];
    for &i in &kept {
        shown[i] = true;
    }
    for &i in &kept {
        let a = b.note(i);
        for &j in index.outgoing(i) {
            if shown[j] {
                let t = b.note(j);
                b.edge(a, t);
            }
        }
        if o.unresolved {
            for u in index.unresolved(i) {
                let t = b.named(format!("ghost:{}", u.to_lowercase()), u.clone(), Kind::Unresolved);
                b.edge(a, t);
            }
        }
        if o.tags {
            for tag in &index.notes[i].tags {
                let t = b.named(format!("tag:{tag}"), format!("#{tag}"), Kind::Tag);
                b.edge(a, t);
            }
        }
    }
    b.finish()
}

/// `center` and the notes within `depth` links of it, either direction.
pub fn local(index: &Index, center: usize, depth: usize, o: &Options) -> Graph {
    let mut b = Builder::new(index);
    let keep = keeper(index, o);
    let mut dist: BTreeMap<usize, usize> = BTreeMap::from([(center, 0)]);
    let mut queue = VecDeque::from([center]);
    while let Some(i) = queue.pop_front() {
        let d = dist[&i];
        if d >= depth {
            continue;
        }
        for &j in index.outgoing(i).iter().chain(index.backlinks(i)) {
            if !dist.contains_key(&j) && dist.len() < o.max_nodes && (keep(j) || j == center) {
                dist.insert(j, d + 1);
                queue.push_back(j);
            }
        }
    }
    // A hub that links to hundreds of notes would draw as a solid disc:
    // keep the nearest and best connected, and say how many were left out.
    let mut hidden = 0;
    if dist.len() > LOCAL_CAP {
        let mut ranked: Vec<(usize, usize)> = dist.iter().filter(|e| *e.0 != center).map(|e| (*e.0, *e.1)).collect();
        ranked.sort_by_key(|&(i, d)| (d, std::cmp::Reverse(index.outgoing(i).len() + index.backlinks(i).len())));
        let keep: BTreeSet<usize> = ranked.iter().take(LOCAL_CAP - 1).map(|r| r.0).chain([center]).collect();
        hidden = dist.len() - keep.len();
        dist.retain(|i, _| keep.contains(i));
    }
    let c = b.note(center);
    b.g.center = Some(c);
    b.g.hidden = hidden;
    let order: Vec<usize> = dist.keys().copied().collect();
    for &i in &order {
        let a = b.note(i);
        b.g.nodes[a].depth = dist[&i];
    }
    for &i in &order {
        let a = b.of_note[&i];
        for &j in index.outgoing(i) {
            if let Some(&t) = b.of_note.get(&j) {
                b.edge(a, t);
            }
        }
        if o.unresolved && dist[&i] < depth.max(1) {
            for u in index.unresolved(i) {
                let t = b.named(format!("ghost:{}", u.to_lowercase()), u.clone(), Kind::Unresolved);
                b.g.nodes[t].depth = dist[&i] + 1;
                b.edge(a, t);
            }
        }
        if o.tags {
            for tag in &index.notes[i].tags {
                let t = b.named(format!("tag:{tag}"), format!("#{tag}"), Kind::Tag);
                b.g.nodes[t].depth = dist[&i] + 1;
                b.edge(a, t);
            }
        }
    }
    b.finish()
}

fn hash(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in s.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    h
}

/// Place the nodes: a force-directed layout (Fruchterman-Reingold) that
/// starts from positions derived from the labels, so the same graph always
/// settles the same way. Pinned nodes stay where they are; the local
/// graph's centre is held in the middle.
pub fn layout(g: &mut Graph, iterations: Option<usize>) {
    let n = g.nodes.len();
    if n == 0 {
        return;
    }
    for (i, node) in g.nodes.iter_mut().enumerate() {
        if node.pinned || (node.x != 0.0 || node.y != 0.0) {
            continue;
        }
        let h = hash(&node.label);
        let angle = if g.center.is_some() { (h % 3600) as f32 / 3600.0 * std::f32::consts::TAU } else { i as f32 * std::f32::consts::PI * (3.0 - 5f32.sqrt()) };
        let r = if g.center.is_some() { 60.0 + node.depth as f32 * 120.0 } else { 10.0 * (0.5 + i as f32).sqrt() };
        node.x = angle.cos() * r;
        node.y = angle.sin() * r;
    }
    if let Some(c) = g.center {
        g.nodes[c].x = 0.0;
        g.nodes[c].y = 0.0;
        g.nodes[c].pinned = true;
    }
    let ticks = iterations.unwrap_or(if n > 3000 { 120 } else { 300 });
    simulate(g, ticks, 1.0, 1.0 - 0.001f32.powf(1.0 / ticks.max(1) as f32));
}

/// A few more steps of the same physics, warm: what runs while a dot is
/// dragged (a step or two per pointer event, so the notes it links to are
/// pulled along and spring back) and for a moment after it is let go. Work
/// is bounded by `budget` node-steps, so a big graph on a phone takes fewer
/// steps rather than longer; nothing runs while the graph is left alone.
pub fn relax(g: &mut Graph, steps: usize, alpha: f32) {
    let n = g.nodes.len().max(1);
    let steps = steps.min((600_000 / n).max(1));
    simulate(g, steps, alpha, 0.04);
}

/// A square cell of the Barnes-Hut tree: the nodes inside are summed to one
/// far-away mass, so repulsion costs n log n rather than n squared.
struct Cell {
    x0: f32,
    y0: f32,
    size: f32,
    mass: f32,
    cx: f32,
    cy: f32,
    kids: [u32; 4],
    /// The one node here, when it is a leaf.
    leaf: u32,
}

const NONE: u32 = u32::MAX;

fn build_tree(xs: &[f32], ys: &[f32]) -> Vec<Cell> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..xs.len() {
        x0 = x0.min(xs[i]);
        y0 = y0.min(ys[i]);
        x1 = x1.max(xs[i]);
        y1 = y1.max(ys[i]);
    }
    let size = (x1 - x0).max(y1 - y0).max(1.0) + 1.0;
    let mut cells = vec![Cell { x0, y0, size, mass: 0.0, cx: 0.0, cy: 0.0, kids: [NONE; 4], leaf: NONE }];
    for i in 0..xs.len() {
        insert(&mut cells, xs, ys, 0, i);
    }
    cells
}

/// The child of `at` that holds (x, y), made if it isn't there yet.
fn child_for(cells: &mut Vec<Cell>, at: usize, x: f32, y: f32) -> usize {
    let (cx0, cy0, half) = (cells[at].x0, cells[at].y0, cells[at].size / 2.0);
    let q = usize::from(x >= cx0 + half) + 2 * usize::from(y >= cy0 + half);
    if cells[at].kids[q] == NONE {
        cells.push(Cell { x0: cx0 + (q & 1) as f32 * half, y0: cy0 + (q >> 1) as f32 * half, size: half, mass: 0.0, cx: 0.0, cy: 0.0, kids: [NONE; 4], leaf: NONE });
        let id = cells.len() - 1;
        cells[at].kids[q] = id as u32;
    }
    cells[at].kids[q] as usize
}

fn insert(cells: &mut Vec<Cell>, xs: &[f32], ys: &[f32], mut at: usize, i: usize) {
    loop {
        let before = cells[at].mass;
        let c = &mut cells[at];
        c.cx = (c.cx * before + xs[i]) / (before + 1.0);
        c.cy = (c.cy * before + ys[i]) / (before + 1.0);
        c.mass = before + 1.0;
        if before == 0.0 {
            c.leaf = i as u32;
            return;
        }
        if c.kids == [NONE; 4] && c.leaf != NONE {
            // A second node: the one here moves down a level.
            if c.size < 0.01 {
                return; // on top of each other: they stay one mass
            }
            let old = std::mem::replace(&mut c.leaf, NONE) as usize;
            let child = child_for(cells, at, xs[old], ys[old]);
            insert(cells, xs, ys, child, old);
        }
        at = child_for(cells, at, xs[i], ys[i]);
    }
}

/// Repulsion on node `i` from the tree.
fn repel(cells: &[Cell], i: usize, x: f32, y: f32, strength: f32, theta2: f32) -> (f32, f32) {
    let (mut fx, mut fy) = (0.0, 0.0);
    let mut stack = vec![0usize];
    while let Some(at) = stack.pop() {
        let c = &cells[at];
        if c.mass == 0.0 || (c.leaf == i as u32 && c.mass == 1.0) {
            continue;
        }
        let (dx, dy) = (c.cx - x, c.cy - y);
        let d2 = dx * dx + dy * dy;
        let far = c.size * c.size / theta2 < d2;
        if far || c.kids == [NONE; 4] {
            // Near enough in a crowd of one: jitter apart rather than divide by zero.
            let d2 = d2.max(1.0);
            let w = strength * c.mass / d2;
            fx += dx * w;
            fy += dy * w;
        } else {
            for &k in &c.kids {
                if k != NONE {
                    stack.push(k as usize);
                }
            }
        }
    }
    (fx, fy)
}

/// A force simulation in the manner of d3-force, which is what Obsidian's
/// graph uses: nodes repel (Barnes-Hut), links are springs that pull harder
/// on the less-connected end, and a gentle pull to the middle keeps loose
/// notes near the rest instead of out on a ring. Cooling is by `alpha`.
fn simulate(g: &mut Graph, ticks: usize, alpha0: f32, decay: f32) {
    let n = g.nodes.len();
    let mut xs: Vec<f32> = g.nodes.iter().map(|n| n.x).collect();
    let mut ys: Vec<f32> = g.nodes.iter().map(|n| n.y).collect();
    let (mut vx, mut vy) = (vec![0f32; n], vec![0f32; n]);
    let mut deg = vec![0f32; n];
    for &(a, b) in &g.edges {
        deg[a] += 1.0;
        deg[b] += 1.0;
    }
    let link_len = 38.0;
    let charge = -34.0;
    let center = 0.07;
    let mut alpha = alpha0;
    for _ in 0..ticks {
        alpha += (0.0 - alpha) * decay;
        let tree = build_tree(&xs, &ys);
        for i in 0..n {
            if g.nodes[i].pinned {
                continue;
            }
            let (fx, fy) = repel(&tree, i, xs[i], ys[i], charge * alpha, 0.81);
            vx[i] += fx;
            vy[i] += fy;
            // Toward the middle, harder for a lone note.
            let pull = center * alpha * if deg[i] == 0.0 { 6.0 } else { 1.0 };
            vx[i] -= xs[i] * pull;
            vy[i] -= ys[i] * pull;
        }
        for &(a, b) in &g.edges {
            let (dx, dy) = ((xs[b] + vx[b]) - (xs[a] + vx[a]), (ys[b] + vy[b]) - (ys[a] + vy[a]));
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            let strength = 1.0 / deg[a].min(deg[b]).max(1.0);
            let f = (d - link_len) / d * alpha * strength;
            let bias = deg[a] / (deg[a] + deg[b]).max(1.0);
            if !g.nodes[b].pinned {
                vx[b] -= dx * f * bias;
                vy[b] -= dy * f * bias;
            }
            if !g.nodes[a].pinned {
                vx[a] += dx * f * (1.0 - bias);
                vy[a] += dy * f * (1.0 - bias);
            }
        }
        for i in 0..n {
            if g.nodes[i].pinned {
                vx[i] = 0.0;
                vy[i] = 0.0;
                continue;
            }
            vx[i] *= 0.6;
            vy[i] *= 0.6;
            xs[i] += vx[i];
            ys[i] += vy[i];
        }
    }
    for (i, node) in g.nodes.iter_mut().enumerate() {
        if !node.pinned {
            node.x = xs[i];
            node.y = ys[i];
        }
    }
}

pub fn radius(n: &Node) -> f32 {
    (3.0 + 1.5 * (n.degree as f32).sqrt()).min(12.0)
}

/// A dot's colour: its folder's, a tag's, or a dangling link's.
pub fn color<'a>(n: &Node, p: &'a Palette) -> &'a str {
    match n.kind {
        Kind::Tag => &p.tag,
        Kind::Unresolved => &p.dim,
        Kind::Note(_) => p.accent(n.group),
    }
}

/// How layout coordinates map onto the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub scale: f32,
    pub ox: f32,
    pub oy: f32,
}

impl Fit {
    pub fn to_frame(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale + self.ox, y * self.scale + self.oy)
    }

    pub fn to_layout(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.ox) / self.scale, (y - self.oy) / self.scale)
    }
}

/// The fit that centres the whole graph in a `width` x `height` frame.
pub fn fit(g: &Graph, width: f32, height: f32) -> Fit {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for n in &g.nodes {
        x0 = x0.min(n.x);
        y0 = y0.min(n.y);
        x1 = x1.max(n.x);
        y1 = y1.max(n.y);
    }
    if x1 < x0 {
        return Fit { scale: 1.0, ox: width / 2.0, oy: height / 2.0 };
    }
    let pad = 70.0;
    let scale = ((width - 2.0 * pad) / (x1 - x0).max(1.0)).min((height - 2.0 * pad) / (y1 - y0).max(1.0)).min(1.5);
    Fit { scale, ox: (width - (x1 - x0) * scale) / 2.0 - x0 * scale, oy: (height - (y1 - y0) * scale) / 2.0 - y0 * scale }
}

/// Draw it, scaled to fit `width` x `height`. Node `i` has the id `n:i`.
/// `selected` is lit along with its neighbours; the rest dim.
pub fn scene(g: &Graph, selected: Option<usize>, width: f32, height: f32, labels: Option<bool>) -> Scene {
    scene_fit(g, selected, width, height, labels, fit(g, width, height))
}

pub fn scene_fit(g: &Graph, selected: Option<usize>, width: f32, height: f32, labels: Option<bool>, f: Fit) -> Scene {
    let pal = Palette::current();
    let mut s = Scene::new(width, height).background(&pal.sunken);
    if g.nodes.is_empty() {
        s.push(Op::text(width / 2.0 - 90.0, height / 2.0, "Nothing to show", 18.0, &pal.dim));
        return s;
    }
    let at = |n: &Node| f.to_frame(n.x, n.y);
    let lit: BTreeSet<usize> = selected
        .map(|s| g.edges.iter().filter_map(|&(a, b)| if a == s { Some(b) } else if b == s { Some(a) } else { None }).chain([s]).collect())
        .unwrap_or_default();
    // Links are hairlines, faint until one end is picked: a thousand of
    // them must read as texture, not as a wall.
    for &(a, b) in &g.edges {
        let ((ax, ay), (bx, by)) = (at(&g.nodes[a]), at(&g.nodes[b]));
        let on = selected.is_some_and(|s| a == s || b == s);
        let col = if on { &pal.fg } else if selected.is_some() { &pal.fainter } else { &pal.faint };
        s.push(Op::line(ax, ay, bx, by, Stroke::new(col, if on { 1.6 } else { 0.7 })));
    }
    for (i, n) in g.nodes.iter().enumerate() {
        let (x, y) = at(n);
        let r = radius(n);
        let dim = selected.is_some() && !lit.contains(&i);
        let col = if dim { &pal.faint } else { color(n, &pal) };
        let mut dot = Op::ellipse(x, y, r, r).fill(col).with_id(&format!("n:{i}"));
        if selected == Some(i) || g.center == Some(i) {
            dot = dot.stroked(Stroke::new(&pal.strong, 2.0));
        }
        s.push(dot);
    }
    // Labels: the picked note and its neighbours always; then the best
    // connected notes, each only where it doesn't land on another label.
    let show_all = labels.unwrap_or(g.nodes.len() <= 80);
    let mut order: Vec<usize> = (0..g.nodes.len()).collect();
    order.sort_by_key(|&i| {
        let must = selected == Some(i) || g.center == Some(i) || lit.contains(&i);
        (std::cmp::Reverse(must), std::cmp::Reverse(g.nodes[i].degree))
    });
    let mut taken: Vec<[f32; 4]> = Vec::new();
    let budget = if show_all { usize::MAX } else { 40 };
    for i in order {
        let n = &g.nodes[i];
        let must = selected == Some(i) || g.center == Some(i) || lit.contains(&i);
        if !must && (!show_all && n.degree < 3 || selected.is_some()) {
            continue;
        }
        if taken.len() >= budget && !must {
            break;
        }
        let (x, y) = at(n);
        let r = radius(n);
        let w = (n.label.chars().count().min(24) as f32) * 6.4;
        let b = [x - w / 2.0, y + r + 2.0, w, 14.0];
        let hit = |t: &[f32; 4]| b[0] < t[0] + t[2] && t[0] < b[0] + b[2] && b[1] < t[1] + t[3] && t[1] < b[1] + b[3];
        if !must && taken.iter().any(hit) {
            continue;
        }
        taken.push(b);
        let mut t = Op::text(x, y + r + 3.0, &n.label, 12.0, if selected == Some(i) { &pal.strong } else { &pal.soft });
        if let Op::Text { align, max_width, max_lines, .. } = &mut t {
            *align = forge_canvas::Align::Center;
            *max_width = Some(150.0);
            *max_lines = Some(1);
        }
        s.push(t);
    }
    s
}

/// What the pointer did to a graph.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Redraw,
    /// Open node `i` (a note, or a tag to filter by).
    Open(usize),
}

#[derive(Debug, Clone, Default)]
pub struct Pointer {
    dragging: Option<usize>,
}

impl Pointer {
    pub fn handle(&mut self, g: &mut Graph, selected: &mut Option<usize>, f: Fit, ev: &crate::jsoncanvas::Event) -> Vec<Act> {
        let node = ev.id.as_deref().and_then(|i| i.strip_prefix("n:")).and_then(|n| n.parse::<usize>().ok()).filter(|&n| n < g.nodes.len());
        match (ev.kind.as_str(), ev.button) {
            ("press", 1) => {
                *selected = node;
                self.dragging = node;
                vec![Act::Redraw]
            }
            ("drag", 1) => match self.dragging {
                Some(i) => {
                    let (x, y) = f.to_layout(ev.x, ev.y);
                    let (dx, dy) = (x - g.nodes[i].x, y - g.nodes[i].y);
                    g.nodes[i].x = x;
                    g.nodes[i].y = y;
                    g.nodes[i].pinned = true;
                    // The physics runs while it is held: its neighbours are
                    // pulled along and spring back.
                    let _ = (dx, dy);
                    relax(g, 2, 0.25);
                    vec![Act::Redraw]
                }
                None => vec![],
            },
            ("release", 1) => {
                let held = self.dragging.take();
                if held.is_some() {
                    // A moment to settle where it was dropped.
                    relax(g, 12, 0.2);
                    return vec![Act::Redraw];
                }
                vec![]
            }
            ("double", 1) => node.map(Act::Open).into_iter().collect(),
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{Note, RawNote};

    fn index() -> Index {
        let n = |p: &str, t: &str| RawNote { path: p.into(), text: t.into(), ..Default::default() };
        Index::build(&[
            n("Hub.md", "[[A]] [[B]] [[Ghost]] #hub"),
            n("A.md", "[[C]] #x"),
            n("B.md", "[[Hub]]"),
            n("C.md", "[[D]]"),
            n("D.md", ""),
            n("Lonely.md", ""),
            n("notes/E.md", "[[Hub]] #x"),
        ])
    }

    fn labels(g: &Graph) -> Vec<&str> {
        let mut l: Vec<&str> = g.nodes.iter().map(|n| n.label.as_str()).collect();
        l.sort();
        l
    }

    #[test]
    fn the_global_graph_has_notes_and_links() {
        let idx = index();
        let g = global(&idx, &Options::default());
        assert_eq!(g.nodes.len(), 7);
        assert_eq!(g.edges.len(), 5, "Hub-A, Hub-B (once, though B links back), A-C, C-D, E-Hub");
        let hub = g.nodes.iter().find(|n| n.label == "Hub").unwrap();
        assert_eq!(hub.degree, 3);
        let g = global(&idx, &Options { orphans: false, ..Default::default() });
        assert!(!labels(&g).contains(&"Lonely") && g.nodes.len() == 6);
        let g = global(&idx, &Options { unresolved: true, tags: true, ..Default::default() });
        assert!(labels(&g).contains(&"Ghost") && labels(&g).contains(&"#x") && labels(&g).contains(&"#hub"));
        let g = global(&idx, &Options { folder: Some("notes".into()), ..Default::default() });
        assert_eq!(labels(&g), ["E"]);
        let g = global(&idx, &Options { tag: Some("x".into()), ..Default::default() });
        assert_eq!(labels(&g), ["A", "E"]);
        let g = global(&idx, &Options { max_nodes: 3, ..Default::default() });
        assert!(g.nodes.len() <= 4);
    }

    #[test]
    fn the_local_graph_is_a_neighbourhood() {
        let idx = index();
        let hub = idx.find_path("Hub.md").unwrap();
        let g = local(&idx, hub, 1, &Options::default());
        assert_eq!(labels(&g), ["A", "B", "E", "Hub"]);
        assert_eq!(g.center.map(|c| g.nodes[c].label.as_str()), Some("Hub"));
        let g = local(&idx, hub, 2, &Options::default());
        assert!(labels(&g).contains(&"C") && !labels(&g).contains(&"D"));
        assert_eq!(g.nodes.iter().find(|n| n.label == "C").unwrap().depth, 2);
        let g = local(&idx, hub, 3, &Options { unresolved: true, ..Default::default() });
        assert!(labels(&g).contains(&"D") && labels(&g).contains(&"Ghost"));
        // Backlinks count: from D, depth 1 reaches C.
        let d = idx.find_path("D.md").unwrap();
        assert_eq!(labels(&local(&idx, d, 1, &Options::default())), ["C", "D"]);
    }

    #[test]
    fn layout_is_deterministic_and_spreads_nodes() {
        let idx = index();
        let mut a = global(&idx, &Options::default());
        let mut b = global(&idx, &Options::default());
        layout(&mut a, None);
        layout(&mut b, None);
        let pos = |g: &Graph| g.nodes.iter().map(|n| (n.x, n.y)).collect::<Vec<_>>();
        assert_eq!(pos(&a), pos(&b));
        for i in 0..a.nodes.len() {
            assert!(a.nodes[i].x.is_finite() && a.nodes[i].y.is_finite());
            for j in i + 1..a.nodes.len() {
                let d = ((a.nodes[i].x - a.nodes[j].x).powi(2) + (a.nodes[i].y - a.nodes[j].y).powi(2)).sqrt();
                assert!(d > 5.0, "{} and {} overlap", a.nodes[i].label, a.nodes[j].label);
            }
        }
        // Linked nodes end up nearer than unlinked ones, on average.
        let dist = |x: usize, y: usize| ((a.nodes[x].x - a.nodes[y].x).powi(2) + (a.nodes[x].y - a.nodes[y].y).powi(2)).sqrt();
        let linked: f32 = a.edges.iter().map(|&(x, y)| dist(x, y)).sum::<f32>() / a.edges.len() as f32;
        let mut all = 0.0;
        let mut count = 0;
        for i in 0..a.nodes.len() {
            for j in i + 1..a.nodes.len() {
                all += dist(i, j);
                count += 1;
            }
        }
        let _ = all / count as f32;
        // Springs hold linked notes near their rest length.
        assert!(linked > 10.0 && linked < 150.0, "{linked}");
        // The local graph holds its centre in the middle.
        let hub = idx.find_path("Hub.md").unwrap();
        let mut l = local(&idx, hub, 2, &Options::default());
        layout(&mut l, None);
        let c = l.center.unwrap();
        assert_eq!((l.nodes[c].x, l.nodes[c].y), (0.0, 0.0));
    }

    #[test]
    fn the_pointer_selects_drags_and_opens() {
        let idx = index();
        let mut g = global(&idx, &Options::default());
        layout(&mut g, None);
        let f = fit(&g, 1000.0, 700.0);
        let (px, py) = f.to_frame(g.nodes[2].x, g.nodes[2].y);
        assert!((f.to_layout(px, py).0 - g.nodes[2].x).abs() < 0.01, "fit round-trips");
        let ev = |kind: &str, x: f32, y: f32, id: Option<&str>| crate::jsoncanvas::Event { kind: kind.into(), button: 1, x, y, id: id.map(Into::into), shift: false, dy: 0.0 };
        let (mut p, mut sel) = (Pointer::default(), None);
        assert_eq!(p.handle(&mut g, &mut sel, f, &ev("press", px, py, Some("n:2"))), [Act::Redraw]);
        assert_eq!(sel, Some(2));
        p.handle(&mut g, &mut sel, f, &ev("drag", px + 50.0, py, Some("n:2")));
        assert!(g.nodes[2].pinned);
        let (nx, _) = f.to_frame(g.nodes[2].x, g.nodes[2].y);
        assert!((nx - (px + 50.0)).abs() < 0.01);
        p.handle(&mut g, &mut sel, f, &ev("release", 0.0, 0.0, None));
        assert!(p.handle(&mut g, &mut sel, f, &ev("drag", 5.0, 5.0, None)).is_empty());
        assert_eq!(p.handle(&mut g, &mut sel, f, &ev("double", px, py, Some("n:2"))), [Act::Open(2)]);
        p.handle(&mut g, &mut sel, f, &ev("press", 1.0, 1.0, None));
        assert_eq!(sel, None);
    }

    #[test]
    fn the_scene_has_a_clickable_dot_per_node() {
        let idx = index();
        let mut g = global(&idx, &Options::default());
        layout(&mut g, None);
        let s = scene(&g, None, 1000.0, 700.0, None);
        let ids: Vec<String> = s.ops.iter().filter_map(|o| o.id().map(String::from)).collect();
        assert_eq!(ids.len(), g.nodes.len());
        let b = s.bounds_of("n:0").unwrap();
        assert_eq!(s.hit(b[0] + b[2] / 2.0, b[1] + b[3] / 2.0).as_deref(), Some("n:0"));
        let lit = scene(&g, Some(0), 1000.0, 700.0, None);
        assert_ne!(lit.to_json(), s.to_json());
        assert!(scene(&Graph::default(), None, 400.0, 300.0, None).ops.len() == 1);
    }

    /// Writes a preview SVG of a vault-sized graph when FORGE_PREVIEW_DIR is set.
    #[test]
    fn preview_big_graph() {
        let Ok(dir) = std::env::var("FORGE_PREVIEW_DIR") else { return };
        let mut raws = Vec::new();
        let mut seed = 12345u64;
        let mut rnd = |m: u64| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) % m };
        let total = 1500;
        for i in 0..total {
            let mut text = String::new();
            if i < 1000 {
                // Clusters of notes that mostly link inside their own area, a few hubs between.
                let area = i / 100;
                for _ in 0..(1 + rnd(14)) {
                    let j = if rnd(10) < 8 { area * 100 + rnd(100) } else { rnd(1000) };
                    text.push_str(&format!("[[n{j}]] "));
                }
            }
            raws.push(RawNote { path: format!("area{}/n{i}.md", i / 100), text, ..Default::default() });
        }
        let idx = Index::build(&raws);
        let t = std::time::Instant::now();
        let mut g = global(&idx, &Options::default());
        let built = t.elapsed();
        layout(&mut g, None);
        eprintln!("{} nodes, {} links: built in {built:?}, laid out in {:?}", g.nodes.len(), g.edges.len(), t.elapsed() - built);
        std::fs::write(format!("{dir}/graph.svg"), scene(&g, None, 1400.0, 900.0, None).to_svg()).unwrap();
        let hub = (0..idx.notes.len()).max_by_key(|&i| idx.outgoing(i).len() + idx.backlinks(i).len()).unwrap();
        let mut l = local(&idx, hub, 2, &Options::default());
        layout(&mut l, None);
        std::fs::write(format!("{dir}/graph-local.svg"), scene(&l, None, 1400.0, 900.0, None).to_svg()).unwrap();
        let sel = g.nodes.iter().position(|n| n.degree > 15);
        std::fs::write(format!("{dir}/graph-selected.svg"), scene(&g, sel, 1400.0, 900.0, None).to_svg()).unwrap();
    }

    /// Timings on a vault of FORGE_BENCH_N notes (default 10000):
    /// `FORGE_BENCH_N=300000 cargo test --release -p forge-notes bench_scale -- --nocapture`.
    #[test]
    fn bench_scale() {
        let Ok(n) = std::env::var("FORGE_BENCH_N") else { return };
        let total: u64 = n.parse().unwrap_or(10_000);
        let mut seed = 99u64;
        let mut rnd = |m: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        let area = (total / 200).max(1);
        let mut raws = Vec::with_capacity(total as usize);
        for i in 0..total {
            let mut text = String::new();
            for _ in 0..rnd(9) {
                // Mostly inside its own folder, a few anywhere, some to a few popular notes.
                let j = match rnd(10) {
                    0..=6 => (i / area) * area + rnd(area).min(total - 1 - (i / area) * area),
                    7 | 8 => rnd(total),
                    _ => rnd(50),
                };
                text.push_str(&format!("[[n{j}]] "));
            }
            raws.push(RawNote { path: format!("a{}/s{}/n{i}.md", i / area / 10, i / area), text, ..Default::default() });
        }
        let t = std::time::Instant::now();
        let idx = Index::build(&raws);
        let t_index = t.elapsed();
        let hd = crate::cache::Header { saved: 1, vault: "v".into() };
        let t = std::time::Instant::now();
        let saved = crate::cache::encode_index(&idx, &hd);
        let t_save = t.elapsed();
        let t = std::time::Instant::now();
        let (_, idx2) = crate::cache::decode_index(&saved).unwrap();
        let t_load = t.elapsed();
        assert_eq!(idx2.notes.len(), idx.notes.len());
        let t = std::time::Instant::now();
        let mut g = global(&idx2, &Options::default());
        let t_graph = t.elapsed();
        let t = std::time::Instant::now();
        layout(&mut g, None);
        let t_layout = t.elapsed();
        let lay = crate::cache::encode_layout(&g, &idx2, "k", &hd).unwrap();
        let mut g2 = global(&idx2, &Options::default());
        let t = std::time::Instant::now();
        assert!(crate::cache::apply_layout(&lay, &mut g2, &idx2, "k"));
        let t_reuse = t.elapsed();
        let t = std::time::Instant::now();
        let snap = crate::cache::decode_snapshot(&lay, "k").unwrap();
        let t_snap = t.elapsed();
        assert_eq!(snap.graph.nodes.len(), g2.nodes.len());
        let t = std::time::Instant::now();
        let pos = idx2.positions_of(&snap.paths);
        let t_pos = t.elapsed();
        assert!(pos.iter().all(Option::is_some));
        let t = std::time::Instant::now();
        let json = scene(&g, None, 1400.0, 900.0, None).to_json();
        let t_scene = t.elapsed();
        let t = std::time::Instant::now();
        let hub = (0..idx.notes.len()).max_by_key(|&i| idx.outgoing(i).len() + idx.backlinks(i).len()).unwrap();
        let mut l = local(&idx, hub, 2, &Options::default());
        layout(&mut l, None);
        let t_local = t.elapsed();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            relax(&mut g, 2, 0.25);
        }
        let t_drag = t.elapsed() / 10;
        eprintln!(
            "{total} notes: save {t_save:?} ({} MB), LOAD {t_load:?}, reuse layout {t_reuse:?}, OPEN FROM SNAPSHOT {t_snap:?} ({} KB), then positions_of {t_pos:?}\n  -> drawn {} nodes {} links (hidden {}): index {t_index:?}, graph {t_graph:?}, layout {t_layout:?}, scene {t_scene:?} ({} KB json), local {t_local:?}, one drag step {t_drag:?}",
            saved.len() / 1_000_000,
            lay.len() / 1024,
            g.nodes.len(),
            g.edges.len(),
            g.hidden,
            json.len() / 1024
        );
    }

    #[test]
    fn a_vault_bigger_than_the_budget_shows_its_best_connected_notes() {
        // 3000 notes; the first 2900 link only to the first, the last 100
        // link to each other in a chain: with room for 200, the hub and the
        // chain are what is drawn, not simply the first 200 files.
        let mut raws = Vec::new();
        for i in 0..3000 {
            let text = if i < 2900 { "[[n2999]] ".to_string() } else if i < 2999 { format!("[[n{}]] ", i + 1) } else { String::new() };
            raws.push(RawNote { path: format!("f{}/n{i}.md", i % 7), text, ..Default::default() });
        }
        let idx = Index::build(&raws);
        let o = Options { max_nodes: 200, ..Options::default() };
        let g = global(&idx, &o);
        assert_eq!(g.nodes.len(), 200);
        assert_eq!(g.hidden, 2800);
        assert!(g.nodes.iter().any(|n| n.label == "n2999"), "the hub is drawn");
        assert!(g.nodes.iter().any(|n| n.label == "n2950"), "the chain is drawn");
        // The same vault draws the same notes every time.
        let again = global(&idx, &o);
        assert_eq!(g.nodes.iter().map(|n| n.label.clone()).collect::<Vec<_>>(), again.nodes.iter().map(|n| n.label.clone()).collect::<Vec<_>>());
        // A folder filter looks at the rest.
        let f = global(&idx, &Options { folder: Some("f3".into()), max_nodes: 200, ..Options::default() });
        assert!(f.nodes.iter().all(|n| matches!(&n.kind, Kind::Note(i) if idx.notes[*i].folder == "f3")));
    }

    #[test]
    fn a_vault_is_indexed_note_by_note_the_same_as_in_one_go() {
        let raws: Vec<RawNote> = (0..50).map(|i| RawNote { path: format!("d{}/n{i}.md", i % 3), text: format!("---\ntags: [t{}]\n---\nSee [[n{}]] and [[d1/n4]] #x\n", i % 5, (i + 1) % 50), ..Default::default() }).collect();
        let whole = Index::build(&raws);
        let light = Index::from_notes(raws.iter().map(Note::parse_light).collect());
        for i in 0..whole.notes.len() {
            assert_eq!(whole.outgoing(i), light.outgoing(i));
            assert_eq!(whole.backlinks(i), light.backlinks(i));
            assert_eq!(whole.notes[i].tags, light.notes[i].tags);
        }
        assert!(light.notes[0].excerpt.is_empty() && light.notes[0].props.is_empty());
    }

    #[test]
    fn a_huge_hub_is_capped_and_dragging_pulls_neighbours() {
        let mut raws = vec![RawNote { path: "index.md".into(), text: (0..600).map(|i| format!("[[n{i}]] ")).collect(), ..Default::default() }];
        for i in 0..600 {
            raws.push(RawNote { path: format!("n{i}.md"), text: if i % 3 == 0 { format!("[[n{}]]", (i + 1) % 600) } else { String::new() }, ..Default::default() });
        }
        let idx = Index::build(&raws);
        let hub = idx.find_path("index.md").unwrap();
        let mut g = local(&idx, hub, 1, &Options::default());
        assert_eq!(g.nodes.len(), LOCAL_CAP);
        assert_eq!(g.hidden, 601 - LOCAL_CAP);
        layout(&mut g, None);
        // Pull a linked pair apart: the neighbour of the dragged dot follows.
        let (a, b) = *g.edges.iter().find(|&&(x, y)| Some(x) != g.center && Some(y) != g.center).unwrap();
        let before = (g.nodes[b].x, g.nodes[b].y);
        g.nodes[a].x += 400.0;
        g.nodes[a].pinned = true;
        relax(&mut g, 6, 0.3);
        assert!((g.nodes[b].x - before.0).abs() + (g.nodes[b].y - before.1).abs() > 1.0, "the neighbour moved");
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
    }
}
