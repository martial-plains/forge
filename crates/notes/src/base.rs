// SPDX-License-Identifier: MIT OR Apache-2.0
//! Obsidian Bases: a `.base` file (YAML) describes views of notes -- which
//! notes, which properties, in what order -- and this runs them.

use std::collections::BTreeMap;

use forge_canvas::{ImageFit, Op, Palette, Scene, Stroke, Transform};
use serde_yaml::Value as Y;

use crate::expr::{Ctx, eval_str};
use crate::value::Value;
use crate::vault::Index;

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Expr(String),
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Vec<Filter>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub prop: String,
    pub desc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Table,
    Cards,
    List,
    /// Cards in columns, one per value of the grouped property.
    Kanban,
    /// Pins on a map, from a coordinates property.
    Map,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub kind: Kind,
    pub name: String,
    pub filters: Option<Filter>,
    pub order: Vec<String>,
    pub sort: Vec<SortKey>,
    pub group_by: Option<SortKey>,
    pub limit: Option<usize>,
    /// Cards: the property holding a cover picture.
    pub image: Option<String>,
    /// Cards: the picture's height as a multiple of its width (1.5 is a
    /// poster, 0.56 a video still).
    pub image_ratio: f32,
    /// Cards: `cover` fills the picture's box (cropping), `contain` shows
    /// all of it.
    pub image_fit: ImageFit,
    /// Kanban: column order (values of the grouped property); other values
    /// follow in sort order.
    pub columns: Vec<String>,
    /// Map: where the coordinates are.
    pub map: crate::geo::MapOpts,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Base {
    pub filters: Option<Filter>,
    pub formulas: BTreeMap<String, String>,
    /// Property -> the name to show for it.
    pub names: BTreeMap<String, String>,
    pub views: Vec<View>,
}

fn filter_of(y: &Y) -> Result<Filter, String> {
    match y {
        Y::String(s) => Ok(Filter::Expr(s.clone())),
        Y::Bool(b) => Ok(Filter::Expr(b.to_string())),
        Y::Mapping(m) => {
            let (k, v) = m.iter().next().ok_or("an empty filter")?;
            let list = |v: &Y| -> Result<Vec<Filter>, String> {
                v.as_sequence().ok_or("and / or / not take a list")?.iter().map(filter_of).collect()
            };
            match k.as_str() {
                Some("and") => Ok(Filter::And(list(v)?)),
                Some("or") => Ok(Filter::Or(list(v)?)),
                Some("not") => Ok(Filter::Not(list(v)?)),
                other => Err(format!("unknown filter operator {other:?}")),
            }
        }
        _ => Err("a filter is an expression or and / or / not".into()),
    }
}

fn sort_key(y: &Y) -> Option<SortKey> {
    let prop = y.get("property")?.as_str()?.to_string();
    let desc = y.get("direction").and_then(Y::as_str).is_some_and(|d| d.eq_ignore_ascii_case("desc"));
    Some(SortKey { prop, desc })
}

impl Base {
    pub fn parse(text: &str) -> Result<Base, String> {
        let y: Y = serde_yaml::from_str(text).map_err(|e| format!("not a Base file: {e}"))?;
        if y.is_null() {
            return Ok(Base::default());
        }
        let mut b = Base::default();
        if let Some(f) = y.get("filters") {
            b.filters = Some(filter_of(f)?);
        }
        if let Some(m) = y.get("formulas").and_then(Y::as_mapping) {
            for (k, v) in m {
                if let Some(k) = k.as_str() {
                    b.formulas.insert(k.to_string(), v.as_str().map_or_else(|| serde_yaml::to_string(v).unwrap_or_default().trim().to_string(), str::to_string));
                }
            }
        }
        if let Some(m) = y.get("properties").and_then(Y::as_mapping) {
            for (k, v) in m {
                if let (Some(k), Some(n)) = (k.as_str(), v.get("displayName").and_then(Y::as_str)) {
                    b.names.insert(k.to_string(), n.to_string());
                }
            }
        }
        for (i, v) in y.get("views").and_then(Y::as_sequence).map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
            let kind = match v.get("type").and_then(Y::as_str).unwrap_or("table") {
                "table" => Kind::Table,
                "cards" => Kind::Cards,
                "list" => Kind::List,
                "kanban" => Kind::Kanban,
                "map" => Kind::Map,
                o => Kind::Other(o.to_string()),
            };
            b.views.push(View {
                kind,
                name: v.get("name").and_then(Y::as_str).map_or_else(|| format!("View {}", i + 1), str::to_string),
                filters: v.get("filters").map(filter_of).transpose()?,
                order: v.get("order").and_then(Y::as_sequence).map(|s| s.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                sort: v.get("sort").and_then(Y::as_sequence).map(|s| s.iter().filter_map(sort_key).collect()).unwrap_or_default(),
                group_by: v.get("groupBy").and_then(sort_key),
                limit: v.get("limit").and_then(Y::as_u64).map(|n| n as usize),
                image: v.get("image").and_then(Y::as_str).map(str::to_string),
                image_ratio: v.get("imageAspectRatio").and_then(Y::as_f64).map_or(1.0, |r| (r as f32).clamp(0.2, 3.0)),
                image_fit: match v.get("imageFit").and_then(Y::as_str) {
                    Some("contain") => ImageFit::Contain,
                    _ => ImageFit::Cover,
                },
                columns: v.get("columns").and_then(Y::as_sequence).map(|s| s.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                map: crate::geo::MapOpts::from_yaml(v),
            });
        }
        Ok(b)
    }

    /// A starting point for a new `.base` file.
    pub fn template(tag: Option<&str>) -> String {
        let filter = tag.map_or(String::new(), |t| format!("filters:\n  and:\n    - file.hasTag(\"{t}\")\n"));
        format!(
            "{filter}formulas:\n  age: \"(now() - file.ctime).days\"\nproperties:\n  formula.age:\n    displayName: Age (days)\nviews:\n  - type: table\n    name: Notes\n    order:\n      - file.name\n      - file.folder\n      - formula.age\n      - file.mtime\n    sort:\n      - property: file.mtime\n        direction: DESC\n  - type: cards\n    name: Cards\n    order:\n      - file.name\n      - file.folder\n  - type: kanban\n    name: Board\n    groupBy:\n      property: status\n    order:\n      - file.name\n      - file.folder\n"
        )
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub prop: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub note: usize,
    pub cells: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub key: Option<Value>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub columns: Vec<Column>,
    pub groups: Vec<Group>,
    /// Notes that matched before `limit` cut them.
    pub total: usize,
    /// Problems in filters and formulas, each reported once.
    pub errors: Vec<String>,
}

fn title_of(base: &Base, prop: &str) -> String {
    if let Some(n) = base.names.get(prop) {
        return n.clone();
    }
    prop.rsplit('.').next().unwrap_or(prop).to_string()
}

fn passes(f: &Filter, ctx: &Ctx, errors: &mut Vec<String>) -> bool {
    match f {
        Filter::Expr(src) => match eval_str(src, ctx) {
            Ok(v) => v.truthy(),
            Err(e) => {
                let msg = format!("{src}: {e}");
                if !errors.contains(&msg) {
                    errors.push(msg);
                }
                false
            }
        },
        Filter::And(l) => l.iter().all(|x| passes(x, ctx, errors)),
        Filter::Or(l) => l.iter().any(|x| passes(x, ctx, errors)),
        Filter::Not(l) => !l.iter().any(|x| passes(x, ctx, errors)),
    }
}

pub fn prop_value(prop: &str, ctx: &Ctx, errors: &mut Vec<String>) -> Value {
    let direct = |name: &str| ctx.index.notes[ctx.note].props.get(name).cloned().unwrap_or(Value::Null);
    if prop.starts_with("file.") || prop.starts_with("formula.") {
        return eval_str(prop, ctx).unwrap_or_else(|e| {
            let msg = format!("{prop}: {e}");
            if !errors.contains(&msg) {
                errors.push(msg);
            }
            Value::Null
        });
    }
    direct(prop.strip_prefix("note.").unwrap_or(prop))
}

/// Run one view. `extra` is a filter typed at the moment (an expression).
pub fn run(index: &Index, base: &Base, view: usize, now: i64, extra: Option<&str>) -> Table {
    let mut t = Table::default();
    let Some(v) = base.views.get(view) else {
        t.errors.push("no such view".into());
        return t;
    };
    let order: Vec<String> = if v.order.is_empty() { vec!["file.name".into()] } else { v.order.clone() };
    t.columns = order.iter().map(|p| Column { prop: p.clone(), title: title_of(base, p) }).collect();
    let mut matched: Vec<usize> = Vec::new();
    for i in 0..index.notes.len() {
        let ctx = Ctx::new(index, i, &base.formulas, now);
        let ok = base.filters.as_ref().is_none_or(|f| passes(f, &ctx, &mut t.errors))
            && v.filters.as_ref().is_none_or(|f| passes(f, &ctx, &mut t.errors))
            && extra.is_none_or(|e| passes(&Filter::Expr(e.to_string()), &ctx, &mut t.errors));
        if ok {
            matched.push(i);
        }
    }
    // Sort by the keys (a stable sort, applied last key first), then the
    // group key.
    let mut keys: Vec<SortKey> = v.sort.clone();
    if keys.is_empty() {
        keys.push(SortKey { prop: "file.name".into(), desc: false });
    }
    let mut errors = std::mem::take(&mut t.errors);
    let mut sorted: Vec<(usize, Vec<Value>, Value)> = matched
        .iter()
        .map(|&i| {
            let ctx = Ctx::new(index, i, &base.formulas, now);
            let ks = keys.iter().map(|k| prop_value(&k.prop, &ctx, &mut errors)).collect();
            let g = v.group_by.as_ref().map_or(Value::Null, |g| prop_value(&g.prop, &ctx, &mut errors));
            (i, ks, g)
        })
        .collect();
    sorted.sort_by(|a, b| {
        if let Some(g) = &v.group_by {
            let o = a.2.compare(&b.2);
            let o = if g.desc { o.reverse() } else { o };
            if o.is_ne() {
                return o;
            }
        }
        for (n, k) in keys.iter().enumerate() {
            let o = a.1[n].compare(&b.1[n]);
            let o = if k.desc && !matches!(a.1[n], Value::Null) && !matches!(b.1[n], Value::Null) { o.reverse() } else { o };
            if o.is_ne() {
                return o;
            }
        }
        std::cmp::Ordering::Equal
    });
    t.total = sorted.len();
    if let Some(n) = v.limit {
        sorted.truncate(n);
    }
    for (i, _, g) in sorted {
        let ctx = Ctx::new(index, i, &base.formulas, now);
        let cells = order.iter().map(|p| prop_value(p, &ctx, &mut errors)).collect();
        let key = v.group_by.is_some().then_some(g);
        match t.groups.last_mut() {
            Some(last) if last.key == key => last.rows.push(Row { note: i, cells }),
            _ => t.groups.push(Group { key, rows: vec![Row { note: i, cells }] }),
        }
    }
    t.errors = errors;
    t
}

fn clip(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let mut o: String = s.chars().take(max.saturating_sub(1)).collect();
        o.push('…');
        o
    }
}

/// A table drawn as text: the text, and per line the note it shows.
pub struct Rendered {
    pub text: String,
    pub line_notes: Vec<Option<usize>>,
    /// The header line's column starts (in characters), for sorting by click.
    pub column_starts: Vec<usize>,
    /// Columns left out because the window is too narrow for them.
    pub hidden: usize,
}

pub fn render_table(t: &Table, max_col: usize) -> Rendered {
    render_table_fit(t, max_col, None)
}

/// [`render_table`] within `budget` columns of text: the title column is
/// narrowed first, then columns are left off the right, so a phone-sized
/// window shows a readable table rather than a line to scroll sideways.
pub fn render_table_fit(t: &Table, max_col: usize, budget: Option<usize>) -> Rendered {
    let all = t.columns.len();
    let mut n = all;
    let cell = |v: &Value| clip(&v.to_string(), max_col);
    let mut widths: Vec<usize> = t.columns.iter().map(|c| clip(&c.title, max_col).chars().count()).collect();
    for g in &t.groups {
        for r in &g.rows {
            for (i, c) in r.cells.iter().enumerate() {
                widths[i] = widths[i].max(cell(c).chars().count());
            }
        }
    }
    if let Some(b) = budget {
        let span = |k: usize| widths[..k].iter().sum::<usize>() + 2 * k.saturating_sub(1);
        while n > 1 && span(n) > b {
            n -= 1;
        }
        widths.truncate(n);
        if let Some(w0) = widths.first_mut() {
            *w0 = (*w0).min(b.max(8));
        }
    }
    let pad = |s: &str, w: usize| format!("{s}{}", " ".repeat(w.saturating_sub(s.chars().count())));
    let mut text = String::new();
    let mut line_notes = Vec::new();
    let mut starts = Vec::new();
    let mut at = 0;
    for w in &widths {
        starts.push(at);
        at += w + 2;
    }
    let header: Vec<String> = t.columns.iter().zip(&widths).map(|(c, w)| pad(&clip(&c.title, (*w).min(max_col)), *w)).collect();
    text.push_str(header.join("  ").trim_end());
    text.push('\n');
    line_notes.push(None);
    let total_w: usize = widths.iter().sum::<usize>() + 2 * n.saturating_sub(1);
    text.push_str(&"─".repeat(total_w.max(1)));
    text.push('\n');
    line_notes.push(None);
    for g in &t.groups {
        if let Some(k) = &g.key {
            let label = if k.is_empty() { "(none)".to_string() } else { k.to_string() };
            text.push_str(&format!("▸ {label}  ({})\n", g.rows.len()));
            line_notes.push(None);
        }
        for r in &g.rows {
            let cells: Vec<String> = r.cells.iter().zip(&widths).map(|(c, w)| pad(&clip(&c.to_string(), *w), *w)).collect();
            text.push_str(cells.join("  ").trim_end());
            text.push('\n');
            line_notes.push(Some(r.note));
        }
    }
    let shown: usize = t.groups.iter().map(|g| g.rows.len()).sum();
    text.push('\n');
    line_notes.push(None);
    text.push_str(&if shown < t.total { format!("{shown} of {} notes\n", t.total) } else { format!("{shown} notes\n") });
    line_notes.push(None);
    for e in &t.errors {
        text.push_str(&format!("! {e}\n"));
        line_notes.push(None);
    }
    Rendered { text, line_notes, column_starts: starts, hidden: all - n }
}

pub fn render_list(t: &Table) -> Rendered {
    let mut text = String::new();
    let mut line_notes = Vec::new();
    for g in &t.groups {
        if let Some(k) = &g.key {
            text.push_str(&format!("▸ {}\n", if k.is_empty() { "(none)".to_string() } else { k.to_string() }));
            line_notes.push(None);
        }
        for r in &g.rows {
            let rest: Vec<String> = r.cells.iter().skip(1).filter(|c| !c.is_empty()).map(ToString::to_string).collect();
            text.push_str(&format!("• {}{}\n", r.cells.first().map(ToString::to_string).unwrap_or_default(), if rest.is_empty() { String::new() } else { format!(" — {}", rest.join(" · ")) }));
            line_notes.push(Some(r.note));
        }
    }
    Rendered { text, line_notes, column_starts: Vec::new(), hidden: 0 }
}

pub fn to_csv(t: &Table) -> String {
    let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = t.columns.iter().map(|c| q(&c.title)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for g in &t.groups {
        for r in &g.rows {
            out.push_str(&r.cells.iter().map(|c| q(&c.to_string())).collect::<Vec<_>>().join(","));
            out.push('\n');
        }
    }
    out
}

pub const CARD_W: f32 = 240.0;
pub const CARD_H: f32 = 170.0;

/// Property names that usually hold a cover picture, in the order tried.
const COVER_NAMES: &[&str] = &["image", "cover", "poster", "thumbnail", "banner", "photo", "picture", "img", "cover_image", "cover-image", "coverimage", "preview", "header"];

/// The property to take a card's picture from: the view's `image`, else a
/// column or note property named like a cover (Obsidian shows one too).
pub fn guess_image_prop(index: &Index, t: &Table, v: &View) -> Option<String> {
    if let Some(p) = &v.image {
        return Some(p.clone());
    }
    let bare = |p: &str| p.rsplit('.').next().unwrap_or(p).to_ascii_lowercase();
    for name in COVER_NAMES {
        if let Some(c) = t.columns.iter().find(|c| bare(&c.prop) == *name) {
            return Some(c.prop.clone());
        }
    }
    for name in COVER_NAMES {
        let has = t.groups.iter().flat_map(|g| g.rows.iter()).take(50).any(|r| index.notes[r.note].props.get(*name).is_some_and(|v| !v.to_string().is_empty()));
        if has {
            return Some((*name).to_string());
        }
    }
    None
}

const TITLE: f32 = 16.0;
const LABEL: f32 = 12.0;
const LINE: f32 = LABEL * 1.3 + 5.0;

/// Cards: a grid of notes, each a group with id `note:<index>`. Like
/// Obsidian's: the picture across the top (when there is one), then the
/// bold title, then the properties as a label and its value. Text is
/// measured, so nothing overlaps. `image_of` turns a cover property's value
/// into a picture file (or web address).
pub fn cards_scene(index: &Index, t: &Table, v: &View, width: f32, image_of: &dyn Fn(&str, &str) -> Option<String>) -> Scene {
    let gap = 16.0;
    let cols = (((width - gap) / (CARD_W + gap)).floor() as usize).max(1);
    // Cards share the row's width, so a wide window isn't left half empty.
    let cw = ((width - gap * (cols as f32 + 1.0)) / cols as f32).max(CARD_W);
    let image_prop = guess_image_prop(index, t, v);
    let pic_of = |r: &Row| -> Option<String> {
        let p = image_prop.as_deref()?;
        let note = &index.notes[r.note];
        let val = t.columns.iter().position(|c| c.prop == p).map(|i| r.cells[i].to_string()).filter(|s| !s.is_empty()).or_else(|| note.props.get(p).map(ToString::to_string))?;
        image_of(&note.path, &val)
    };
    let pic_h = (cw * v.image_ratio).round();
    let pad = 12.0;
    let pal = Palette::current();
    // What each card shows, measured, so its height is what it needs.
    struct Card<'a> {
        row: &'a Row,
        pic: Option<String>,
        title: Vec<String>,
        props: Vec<(String, String)>,
    }
    let mut groups: Vec<Vec<Card>> = Vec::new();
    let mut card_h = 0f32;
    for g in &t.groups {
        let mut cards = Vec::new();
        for r in &g.rows {
            let pic = pic_of(r);
            let mut title = forge_canvas::wrap(&index.notes[r.note].name, TITLE, true, cw - 2.0 * pad);
            title.truncate(2);
            let props: Vec<(String, String)> = t
                .columns
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(c, col)| !r.cells[*c].to_string().is_empty() && image_prop.as_deref() != Some(col.prop.as_str()))
                .map(|(c, col)| (col.title.clone(), r.cells[c].to_string()))
                .take(5)
                .collect();
            let h = if pic.is_some() { pic_h } else { 0.0 } + pad + title.len().max(1) as f32 * TITLE * 1.3 + if props.is_empty() { 0.0 } else { 8.0 + props.len() as f32 * LINE } + pad;
            card_h = card_h.max(h);
            cards.push(Card { row: r, pic, title, props });
        }
        groups.push(cards);
    }
    let card_h = card_h.max(70.0);
    let mut y = gap;
    let mut ops = Vec::new();
    for (g, cards) in t.groups.iter().zip(&groups) {
        if let Some(k) = &g.key {
            ops.push(Op::text(gap, y, &if k.is_empty() { "(none)".to_string() } else { k.to_string() }, 20.0, pal.accent(1)));
            y += 34.0;
        }
        for (n, c) in cards.iter().enumerate() {
            let (cx, cy) = (gap + (n % cols) as f32 * (cw + gap), y + (n / cols) as f32 * (card_h + gap));
            let mut inner = vec![Op::rect(0.0, 0.0, cw, card_h).fill(&pal.card).stroked(Stroke::new(&pal.line, 1.0)).rounded(10.0)];
            let mut ty = pad;
            if let Some(pic) = &c.pic {
                // Across the top, the card's rounded corners clipping it.
                let mut img = Op::image(pic, 0.0, 0.0, cw, pic_h);
                if let Op::Image { fit, .. } = &mut img {
                    *fit = v.image_fit;
                }
                inner.push(Op::rect(0.0, 0.0, cw, pic_h).fill(&pal.sunken).rounded(10.0));
                inner.push(Op::group(vec![img]).clipped(0.0, 0.0, cw, pic_h));
                ty = pic_h + pad;
            }
            let lines = c.title.len().max(1) as f32;
            let mut title = Op::text(pad, ty, &c.title.join(" "), TITLE, &pal.strong).wrapped(cw - 2.0 * pad);
            if let Op::Text { bold, max_lines, .. } = &mut title {
                *bold = true;
                *max_lines = Some(2);
            }
            inner.push(title);
            ty += lines * TITLE * 1.3 + 8.0;
            let lw = c.props.iter().map(|(l, _)| forge_canvas::measure(l, LABEL, false).0).fold(0.0, f32::max).min((cw - 2.0 * pad) * 0.4) + 10.0;
            for (label, val) in &c.props {
                let mut l = Op::text(pad, ty, label, LABEL, &pal.dim).wrapped(lw);
                let mut line = Op::text(pad + lw, ty, val, LABEL, &pal.soft).wrapped((cw - 2.0 * pad - lw).max(40.0));
                for op in [&mut l, &mut line] {
                    if let Op::Text { max_lines, .. } = op {
                        *max_lines = Some(1);
                    }
                }
                inner.push(l);
                inner.push(line);
                ty += LINE;
            }
            ops.push(Op::group(inner).transformed(Transform::translate(cx, cy)).with_id(&format!("note:{}", c.row.note)));
        }
        y += cards.len().div_ceil(cols) as f32 * (card_h + gap) + 8.0;
    }
    let mut s = Scene::new(width, y.max(200.0)).background(&pal.bg);
    s.ops = ops;
    s
}

/// Kanban columns: the grouped property's values in the view's `columns`
/// order, then the rest as they sorted. Each holds the table's rows (as
/// `(group, row)` pairs). A view with no `groupBy` is one column.
pub fn kanban_columns(t: &Table, v: &View) -> Vec<(String, Vec<(usize, usize)>)> {
    let mut cols: Vec<(String, Vec<(usize, usize)>)> = v.columns.iter().map(|c| (c.clone(), Vec::new())).collect();
    for (gi, g) in t.groups.iter().enumerate() {
        let key = g.key.as_ref().map_or_else(|| "All".to_string(), |k| if k.is_empty() { String::new() } else { k.to_string() });
        let at = match cols.iter().position(|c| c.0 == key) {
            Some(i) => i,
            None => {
                cols.push((key, Vec::new()));
                cols.len() - 1
            }
        };
        cols[at].1.extend((0..g.rows.len()).map(|ri| (gi, ri)));
    }
    cols
}

/// Lane colours, one per column in turn.

/// The gap between lanes and the width of each, for `n` lanes on a page
/// `width` wide: they share a wide page, and stay a readable width on a
/// narrow one (the page then scrolls sideways).
pub fn kanban_layout(n: usize, width: f32) -> (f32, f32) {
    let gap = 16.0;
    let n = n.max(1) as f32;
    let fit = (width - gap * (n + 1.0)) / n;
    if fit < KLANE_MIN {
        // Too narrow for them side by side (a phone): one lane about as
        // wide as the screen, with the next peeking in to say "swipe".
        return (gap, (width * 0.82).clamp(200.0, KLANE_MAX));
    }
    (gap, fit.min(KLANE_MAX))
}

pub const KLANE_MIN: f32 = 240.0;
pub const KLANE_MAX: f32 = 360.0;

/// Rough width of `text` at `size`, for sizing chips and wrapping.
fn text_w(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * 0.56
}

/// Kanban: a lane per column with a card per note. Cards are groups with id
/// `note:<index>`, lanes are rects with id `col:<n>` (what a dragged card is
/// dropped on). Lanes share the page's width, headers carry the lane's
/// colour and count, and a card is as tall as what it shows: title, tags
/// as chips, and the view's other properties.
pub fn kanban_scene(index: &Index, t: &Table, v: &View, width: f32, height: f32) -> Scene {
    let cols = kanban_columns(t, v);
    let (gap, lane_w) = kanban_layout(cols.len(), width);
    let card_w = lane_w - 20.0;
    let head_h = 46.0;
    let pal = Palette::current();
    let mut ops = Vec::new();
    let mut lanes: Vec<(Vec<Op>, f32)> = Vec::new();
    for (n, (key, rows)) in cols.iter().enumerate() {
        let accent = pal.accent(n);
        let x = gap + n as f32 * (lane_w + gap);
        let mut body = Vec::new();
        let mut y = head_h;
        for &(gi, ri) in rows {
            let r = &t.groups[gi].rows[ri];
            let note = &index.notes[r.note];
            let mut inner = Vec::new();
            // Title: up to two lines.
            let lines = forge_canvas::wrap(&note.name, 15.0, true, card_w - 28.0).len().clamp(1, 2) as f32;
            let mut title = Op::text(14.0, 12.0, &note.name, 15.0, &pal.strong).wrapped(card_w - 28.0);
            if let Op::Text { bold, max_lines, .. } = &mut title {
                *bold = true;
                *max_lines = Some(2);
            }
            inner.push(title);
            let mut ty = 12.0 + lines * 15.0 * 1.3 + 8.0;
            // Tags as chips, as many as fit on a line.
            let mut cx = 14.0;
            let mut chips = false;
            for tag in note.tags.iter().take(6) {
                let label = format!("#{tag}");
                let w = text_w(&label, 11.0) + 14.0;
                if cx + w > card_w - 12.0 {
                    break;
                }
                inner.push(Op::rect(cx, ty, w, 20.0).fill(&pal.chip).rounded(10.0));
                inner.push(Op::text(cx + 7.0, ty + 4.0, &label, 11.0, &pal.fg));
                cx += w + 6.0;
                chips = true;
            }
            if chips {
                ty += 28.0;
            }
            // The other properties, label muted and value bright.
            let mut shown = 0;
            for (c, col) in t.columns.iter().enumerate().skip(1) {
                let val = r.cells[c].to_string();
                if val.is_empty() || shown >= 3 || v.group_by.as_ref().is_some_and(|g| g.prop == col.prop) {
                    continue;
                }
                let lw = text_w(&col.title, 11.0) + 8.0;
                inner.push(Op::text(14.0, ty, &col.title, 11.0, &pal.dim));
                let mut line = Op::text(14.0 + lw, ty, &val, 11.0, &pal.soft).wrapped((card_w - 28.0 - lw).max(40.0));
                if let Op::Text { max_lines, .. } = &mut line {
                    *max_lines = Some(1);
                }
                inner.push(line);
                ty += 17.0;
                shown += 1;
            }
            let card_h = ty + 10.0;
            let mut card = vec![Op::rect(0.0, 0.0, card_w, card_h).fill(&pal.card).stroked(Stroke::new(&pal.chip, 1.0)).rounded(10.0)];
            card.push(Op::rect(0.0, 10.0, 3.0, (card_h - 20.0).max(4.0)).fill(accent).rounded(1.5));
            card.extend(inner);
            body.push(Op::group(card).transformed(Transform::translate(10.0, y)).with_id(&format!("note:{}", r.note)));
            y += card_h + 10.0;
        }
        if rows.is_empty() {
            body.push(Op::text(lane_w / 2.0 - 40.0, y + 14.0, "Drop cards here", 12.0, &pal.dim));
            y += 48.0;
        }
        // The header: a coloured dot, the name, and the count in a pill.
        let title = if key.is_empty() { "(none)" } else { key.as_str() };
        let count = rows.len().to_string();
        let mut head = vec![
            Op::ellipse(20.0, 23.0, 5.0, 5.0).fill(accent),
            Op::text(34.0, 14.0, title, 15.0, &pal.fg).wrapped(lane_w - 90.0),
        ];
        if let Op::Text { bold, max_lines, .. } = &mut head[1] {
            *bold = true;
            *max_lines = Some(1);
        }
        let pw = text_w(&count, 11.0) + 14.0;
        head.push(Op::rect(lane_w - pw - 12.0, 13.0, pw, 20.0).fill(&pal.card).rounded(10.0));
        head.push(Op::text(lane_w - pw - 5.0, 17.0, &count, 11.0, &pal.soft));
        body.extend(head);
        lanes.push((body, y + 10.0));
        let _ = x;
    }
    let lane_h = lanes.iter().map(|l| l.1).fold(height - 2.0 * gap, f32::max);
    for (n, (body, _)) in lanes.into_iter().enumerate() {
        let x = gap + n as f32 * (lane_w + gap);
        ops.push(Op::rect(x, gap, lane_w, lane_h).fill(&pal.sunken).stroked(Stroke::new(&pal.card, 1.0)).rounded(14.0).with_id(&format!("col:{n}")));
        // A thin colour bar along the top of the lane.
        ops.push(Op::rect(x + 14.0, gap, lane_w - 28.0, 3.0).fill(pal.accent(n)).rounded(1.5));
        // One group per shape, so a long lane can be drawn a card at a time.
        for op in body {
            ops.push(Op::group(vec![op]).transformed(Transform::translate(x, gap)));
        }
    }
    let total = gap + cols.len().max(1) as f32 * (lane_w + gap);
    let mut s = Scene::new(total.max(width), lane_h + 2.0 * gap).background(&pal.bg);
    s.ops = ops;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::RawNote;

    const BASE: &str = r#"
filters:
  and:
    - file.hasTag("project")
    - 'status != "dropped"'
formulas:
  age: "(now() - file.ctime).days"
  big: "price > 10"
properties:
  formula.age:
    displayName: Age
views:
  - type: table
    name: Projects
    order: [file.name, status, price, formula.age]
    sort:
      - property: price
        direction: DESC
  - type: table
    name: By status
    filters: price >= 5
    groupBy:
      property: status
    order: [file.name, formula.big]
    limit: 2
  - type: cards
    name: Cards
    order: [file.name, status]
"#;

    fn index() -> Index {
        let note = |p: &str, status: &str, price: i32, tag: &str| RawNote {
            path: p.into(),
            text: format!("---\nstatus: {status}\nprice: {price}\ntags: [{tag}]\n---\nbody of {p}"),
            ctime: 1_717_977_600_000 - 3 * crate::value::DAY,
            ..Default::default()
        };
        Index::build(&[
            note("Alpha.md", "active", 20, "project"),
            note("Beta.md", "done", 5, "project"),
            note("Gamma.md", "active", 12, "project"),
            note("Delta.md", "dropped", 99, "project"),
            note("Other.md", "active", 1, "misc"),
        ])
    }

    #[test]
    fn a_base_file_parses() {
        let b = Base::parse(BASE).unwrap();
        assert_eq!(b.views.len(), 3);
        assert_eq!(b.views[0].kind, Kind::Table);
        assert_eq!(b.views[0].sort, [SortKey { prop: "price".into(), desc: true }]);
        assert_eq!(b.views[1].group_by, Some(SortKey { prop: "status".into(), desc: false }));
        assert_eq!(b.views[1].limit, Some(2));
        assert_eq!(b.names["formula.age"], "Age");
        assert_eq!(b.formulas["big"], "price > 10");
        assert!(matches!(b.filters, Some(Filter::And(ref l)) if l.len() == 2));
        assert!(Base::parse("views: [").is_err());
        assert_eq!(Base::parse("").unwrap(), Base::default());
        assert!(Base::parse(&Base::template(Some("x"))).unwrap().views.len() == 3);
    }

    #[test]
    fn filters_sort_and_formulas_run() {
        let (idx, b) = (index(), Base::parse(BASE).unwrap());
        let t = run(&idx, &b, 0, 1_717_977_600_000, None);
        let names: Vec<String> = t.groups[0].rows.iter().map(|r| idx.notes[r.note].name.clone()).collect();
        assert_eq!(names, ["Alpha", "Gamma", "Beta"], "tagged, not dropped, dearest first");
        assert_eq!(t.columns[3].title, "Age");
        assert_eq!(t.groups[0].rows[0].cells[3], Value::Num(3.0));
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let t = run(&idx, &b, 0, 0, Some("price < 15"));
        assert_eq!(t.total, 2);
    }

    #[test]
    fn grouping_and_limits() {
        let (idx, b) = (index(), Base::parse(BASE).unwrap());
        let t = run(&idx, &b, 1, 0, None);
        assert_eq!(t.total, 3, "price >= 5 among the base's own matches");
        assert_eq!(t.groups.iter().map(|g| g.rows.len()).sum::<usize>(), 2, "limit 2");
        assert_eq!(t.groups[0].key, Some(Value::Str("active".into())));
    }

    #[test]
    fn bad_expressions_are_reported_not_fatal() {
        let idx = index();
        let b = Base::parse("filters: 'nonsense('\nviews:\n  - type: table\n").unwrap();
        let t = run(&idx, &b, 0, 0, None);
        assert!(t.groups.is_empty() && !t.errors.is_empty());
        let r = render_table(&t, 20);
        assert!(r.text.contains("! nonsense("));
    }

    #[test]
    fn tables_and_cards_render() {
        let (idx, b) = (index(), Base::parse(BASE).unwrap());
        let t = run(&idx, &b, 0, 1_717_977_600_000, None);
        let r = render_table(&t, 20);
        let lines: Vec<&str> = r.text.lines().collect();
        assert!(lines[0].starts_with("name") || lines[0].starts_with("Name") || lines[0].contains("status"), "{}", lines[0]);
        assert_eq!(r.line_notes.len(), r.text.lines().count());
        assert_eq!(r.line_notes[2].map(|i| idx.notes[i].name.as_str()), Some("Alpha"));
        assert!(r.text.contains("3 notes"));
        assert_eq!(to_csv(&t).lines().count(), 4);
        let list = render_list(&t);
        assert!(list.text.starts_with("• Alpha — active"));

        let c = run(&idx, &b, 2, 0, None);
        let s = cards_scene(&idx, &c, &b.views[2], 800.0, &|_, _| None);
        assert!(s.ops.len() >= 3);
        let first = s.ops.iter().find_map(|o| o.id()).unwrap();
        assert!(first.starts_with("note:"));
        assert_eq!(s.hit(30.0, 30.0).as_deref(), Some(first));
    }

    #[test]
    fn kanban_lanes_follow_the_columns_setting() {
        let raws = vec![
            RawNote { path: "a.md".into(), text: "---\nstatus: done\n---\n".into(), size: 1, ctime: 0, mtime: 0 },
            RawNote { path: "b.md".into(), text: "---\nstatus: todo\n---\n".into(), size: 1, ctime: 0, mtime: 0 },
            RawNote { path: "c.md".into(), text: "---\nstatus: todo\n---\n".into(), size: 1, ctime: 0, mtime: 0 },
        ];
        let idx = Index::build(&raws);
        let b = Base::parse("views:\n  - type: kanban\n    name: K\n    groupBy:\n      property: status\n    columns: [todo, doing, done]\n    order: [file.name, status]\n").unwrap();
        assert_eq!(b.views[0].kind, Kind::Kanban);
        let t = run(&idx, &b, 0, 0, None);
        let cols = kanban_columns(&t, &b.views[0]);
        let names: Vec<&str> = cols.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["todo", "doing", "done"]);
        assert_eq!(cols[0].1.len(), 2);
        assert!(cols[1].1.is_empty());
        let s = kanban_scene(&idx, &t, &b.views[0], 900.0, 600.0);
        let (gap, lw) = kanban_layout(3, 900.0);
        // The middle lane's empty body is the second drop target.
        assert_eq!(s.hit(gap + (lw + gap) + lw / 2.0, 300.0).as_deref(), Some("col:1"));
        assert!(s.bounds_of("note:0").is_some());
        // A narrow page keeps lanes readable and grows sideways instead.
        let narrow = kanban_scene(&idx, &t, &b.views[0], 300.0, 600.0);
        let (_, lw) = kanban_layout(3, 300.0);
        assert!(lw < 300.0 && lw > 200.0 - 1.0, "one lane fills most of the screen: {lw}");
        assert!(narrow.width > 300.0 * 2.0, "the rest is a swipe away");
    }

    #[test]
    fn cards_show_a_cover_and_never_overlap() {
        let raws = vec![RawNote {
            path: "Movies/A very long film title that has to wrap onto a second line.md".into(),
            text: "---\ncover: poster.jpg\nyear: 1999\ngenre: drama\n---\n".into(),
            size: 1,
            ctime: 0,
            mtime: 0,
        }];
        let idx = Index::build(&raws);
        let b = Base::parse("views:\n  - type: cards\n    name: C\n    imageAspectRatio: 1.5\n    imageFit: contain\n    order: [file.name, year, genre]\n").unwrap();
        assert_eq!(b.views[0].image_ratio, 1.5);
        assert_eq!(b.views[0].image_fit, ImageFit::Contain);
        let t = run(&idx, &b, 0, 0, None);
        assert_eq!(guess_image_prop(&idx, &t, &b.views[0]).as_deref(), Some("cover"));
        let with = cards_scene(&idx, &t, &b.views[0], 400.0, &|_, v| Some(format!("/v/{v}")));
        let without = cards_scene(&idx, &t, &b.views[0], 400.0, &|_, _| None);
        let svg = with.to_svg();
        assert!(svg.contains("/v/poster.jpg") && svg.contains("meet"), "{svg}");
        assert!(!without.to_svg().contains("<image"));
        // The picture adds its height, and the cover is not listed as a property.
        assert!(with.height > without.height + 300.0);
        assert!(!without.to_svg().contains("poster.jpg"));
        // The long title takes two lines and the properties start below it.
        let title_y: Vec<f32> = text_ys(&without, "A very");
        let year_y: Vec<f32> = text_ys(&without, "year");
        assert!(year_y[0] >= title_y[0] + 2.0 * 16.0 * 1.3, "{title_y:?} {year_y:?}");
    }

    #[test]
    fn scenes_name_colour_roles_so_a_theme_change_needs_no_redraw_by_the_plugin() {
        let raws = vec![RawNote { path: "A.md".into(), text: "---\nstatus: x\n---\n".into(), size: 1, ctime: 0, mtime: 0 }];
        let idx = Index::build(&raws);
        let b = Base::parse("views:\n  - type: cards\n    name: C\n    order: [file.name, status]\n").unwrap();
        let t = run(&idx, &b, 0, 0, None);
        let s = cards_scene(&idx, &t, &b.views[0], 400.0, &|_, _| None);
        assert!(s.has_roles(), "nothing in a card scene is a fixed colour");
        let json = s.to_json();
        assert!(json.contains("@card") && json.contains("@bg"), "{json}");
        let dark = s.resolved(&Palette::dark()).to_svg();
        let light = s.resolved(&Palette::light()).to_svg();
        assert!(dark.contains("#1e1e2e") && !dark.contains('@'));
        assert!(light.contains("#eff1f5") && !light.contains('@'));
        assert_ne!(dark, light);
    }

    fn text_ys(s: &Scene, starts: &str) -> Vec<f32> {
        fn walk(ops: &[Op], dy: f32, starts: &str, out: &mut Vec<f32>) {
            for op in ops {
                match op {
                    Op::Text { y, text, .. } if text.starts_with(starts) => out.push(y + dy),
                    Op::Group { ops, transform, .. } => walk(ops, dy + transform.as_ref().map_or(0.0, |t| t.y), starts, out),
                    _ => {}
                }
            }
        }
        let mut v = Vec::new();
        walk(&s.ops, 0.0, starts, &mut v);
        v
    }

    #[test]
    fn map_views_parse() {
        let b = Base::parse("views:\n  - type: map\n    name: M\n    coordinates: loc\n    defaultZoom: 5\n    center: [10.6, -61.2]\n    markerColor: '#fff'\n").unwrap();
        assert_eq!(b.views[0].kind, Kind::Map);
        assert_eq!(b.views[0].map.coordinates.as_deref(), Some("loc"));
        assert_eq!(b.views[0].map.center, Some((10.6, -61.2)));
        assert_eq!(b.views[0].map.zoom, Some(5.0));
    }

    /// The starter Bases shipped in crates/notes/tests/fixtures/bases/ all parse and
    /// run against a vault with Markdown and Org notes, without a single
    /// filter or formula error.
    #[test]
    fn the_example_bases_work() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bases");
        let day = crate::value::DAY;
        let now = 1_717_977_600_000_i64;
        let note = |p: &str, text: &str, age_days: i64| RawNote {
            path: p.into(),
            text: text.into(),
            ctime: now - age_days * day,
            mtime: now - age_days * day,
            ..Default::default()
        };
        let index = Index::build(&[
            note("Alpha.md", "---\nstatus: doing\ndue: 2024-06-20\ntags: [work]\ncoordinates: [10.66, -61.51]\n---\nSee [[Beta]]", 2),
            note("Beta.md", "---\nstatus: todo\n---\nback", 120),
            note("projects/Gamma.md", "no properties at all", 400),
            note("church.org", "#+TITLE: Church\n#+FILETAGS: :church:\n#+status: done\n\n* TODO thing", 1),
        ]);
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).expect("the examples folder") {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "base") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let base = Base::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!base.views.is_empty(), "{} has no views", path.display());
            for v in 0..base.views.len() {
                let t = run(&index, &base, v, now, None);
                assert!(t.errors.is_empty(), "{} view {}: {:?}", path.display(), base.views[v].name, t.errors);
                assert!(!matches!(base.views[v].kind, Kind::Other(_)), "{}: unknown view type", path.display());
            }
            seen += 1;
        }
        assert!(seen >= 8, "found {seen} example Bases");
        // A few answers worth checking by hand.
        let task = Base::parse(&std::fs::read_to_string(dir.join("Task Board.base")).unwrap()).unwrap();
        let t = run(&index, &task, 0, now, None);
        let names: Vec<_> = t.groups.iter().flat_map(|g| g.rows.iter()).map(|r| index.notes[r.note].name.clone()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        let tags = Base::parse(&std::fs::read_to_string(dir.join("Tags.base")).unwrap()).unwrap();
        let t = run(&index, &tags, 0, now, None);
        let keys: Vec<String> = t.groups.iter().filter_map(|g| g.key.as_ref().map(|k| k.to_string())).collect();
        assert!(keys.contains(&"work".to_string()) && keys.contains(&"church".to_string()) && keys.contains(&"(untagged)".to_string()), "{keys:?}");
        let kinds = Base::parse(&std::fs::read_to_string(dir.join("Org Files.base")).unwrap()).unwrap();
        let t = run(&index, &kinds, 0, now, None);
        let keys: Vec<String> = t.groups.iter().filter_map(|g| g.key.as_ref().map(|k| k.to_string())).collect();
        assert_eq!(keys, ["Markdown", "Org"], "{keys:?}");
        let stale = Base::parse(&std::fs::read_to_string(dir.join("Stale Notes.base")).unwrap()).unwrap();
        let t = run(&index, &stale, 0, now, None);
        assert_eq!(t.groups.iter().map(|g| g.rows.len()).sum::<usize>(), 2);
    }

    #[test]
    fn a_narrow_window_drops_columns_instead_of_running_off() {
        let index = Index::build(&[RawNote { path: "a.md".into(), text: "---\nauthor: Someone Long Named\nyear: 1999\n---\n".into(), ..Default::default() }]);
        let base = Base::parse("views:\n  - type: table\n    name: T\n    order: [file.name, author, year]\n").unwrap();
        let t = run(&index, &base, 0, 0, None);
        let wide = render_table_fit(&t, 32, Some(80));
        assert_eq!(wide.hidden, 0);
        let narrow = render_table_fit(&t, 32, Some(14));
        assert!(narrow.hidden >= 1, "{}", narrow.text);
        assert!(narrow.text.lines().all(|l| l.chars().count() <= 14), "{}", narrow.text);
    }

    /// The media Bases read the property drawers of media
    /// notes (movies, shows, books, games) and answer sensibly.
    #[test]
    fn the_media_bases_work() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bases/media");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        let year = crate::value::civil_from_days(now.div_euclid(crate::value::DAY)).0;
        let note = |path: &str, kind: &str, props: &str| RawNote {
            path: path.into(),
            text: format!("#+title: T\n\n* T :{kind}:\n:PROPERTIES:\n:TYPE: {kind}\n{props}:END:\n\n** Review\n"),
            ..Default::default()
        };
        let index = Index::build(&[
            note("movies/a.org", "movie", ":STATUS: watched\n:RATING: 9\n:YEAR: 2021\n:GENRES: Sci-Fi, Drama\n:RECOMMEND_TO: Sam\n"),
            note("movies/b.org", "movie", ":STATUS: watched\n:RATING:\n"),
            note("shows/c.org", "show", ":STATUS: watching\n"),
            note("books/d.org", "book", &format!(":STATUS: read\n:RATING: 7\n:AUTHOR: Ann\n:FINISHED: {year}-02-01\n")),
            note("games/e/e.org", "game", ":STATUS: backlog\n:HOURS: 0\n:ADDED: 2025-01-01\n"),
            RawNote { path: "games/e/guide.org".into(), text: ":PROPERTIES:\n:ID: g\n:END:\n#+title: Guide\n".into(), ..Default::default() },
        ]);
        let count = |file: &str, view: usize| {
            let base = Base::parse(&std::fs::read_to_string(dir.join(file)).unwrap()).unwrap_or_else(|e| panic!("{file}: {e}"));
            let t = run(&index, &base, view, now, None);
            assert!(t.errors.is_empty(), "{file} view {view}: {:?}", t.errors);
            t.groups.iter().map(|g| g.rows.len()).sum::<usize>()
        };
        // Every view of every file runs clean.
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let base = Base::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for v in 0..base.views.len() {
                count(path.file_name().unwrap().to_str().unwrap(), v);
            }
        }
        assert_eq!(count("Media Library.base", 0), 5, "the guide has no TYPE");
        assert_eq!(count("Movies.base", 0), 2);
        assert_eq!(count("Movies.base", 2), 1, "watched but not rated");
        assert_eq!(count("Shows.base", 2), 1);
        assert_eq!(count("Books.base", 0), 1);
        assert_eq!(count("Games.base", 2), 1);
        assert_eq!(count("Top Rated.base", 0), 1);
        assert_eq!(count("Top Rated.base", 2), 1);
        assert_eq!(count("Finished This Year.base", 0), 1);
        assert_eq!(count("Recommendations.base", 0), 1);
    }

    /// Writes preview SVGs when FORGE_PREVIEW_DIR is set (not a check).
    #[test]
    fn preview_scenes() {
        let Ok(dir) = std::env::var("FORGE_PREVIEW_DIR") else { return };
        let mut raws = Vec::new();
        for (i, (n, st, tags)) in [("Dune", "todo", "[scifi, classic]"), ("The Left Hand of Darkness", "todo", "[scifi]"), ("A Pattern Language", "doing", "[architecture]"), ("Invisible Cities", "done", "[fiction, italy, classic]"), ("Kitchen Confidential", "done", "[food]"), ("Exhalation", "todo", "[scifi]")].iter().enumerate() {
            raws.push(RawNote { path: format!("{n}.md"), text: format!("---\nstatus: {st}\ntags: {tags}\nauthor: Author {i}\nyear: {}\npages: {}\n---\n", 1960 + i * 11, 200 + i * 40), size: 1, ctime: 0, mtime: 0 });
        }
        let idx = Index::build(&raws);
        let b = Base::parse("views:\n  - type: kanban\n    name: K\n    groupBy:\n      property: status\n    columns: [todo, doing, done]\n    order: [file.name, status, author, year, pages]\n  - type: cards\n    name: C\n    order: [file.name, author, year, pages]\n").unwrap();
        for (name, w) in [("desktop", 1200.0f32), ("phone", 390.0)] {
            let t = run(&idx, &b, 0, 0, None);
            std::fs::write(format!("{dir}/kanban-{name}.svg"), kanban_scene(&idx, &t, &b.views[0], w, 500.0).to_svg()).unwrap();
            let t = run(&idx, &b, 1, 0, None);
            std::fs::write(format!("{dir}/cards-{name}.svg"), cards_scene(&idx, &t, &b.views[1], w, &|_, _| None).to_svg()).unwrap();
        }
    }
}
