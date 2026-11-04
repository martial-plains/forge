// SPDX-License-Identifier: MIT OR Apache-2.0
//! Places: reading coordinates out of properties, the web-mercator map
//! projection, and the scene the Map layout draws (pins over optional tile
//! pictures, or over a graticule when there are no tiles).

use std::f64::consts::PI;

use forge_canvas::{Op, Scene, Stroke, Transform};
use serde_yaml::Value as Y;

use crate::base::Table;
use crate::value::Value;
use crate::vault::Index;

/// Where a Map view finds its coordinates, from the view's YAML.
///
/// ```yaml
/// - type: map
///   coordinates: location        # a [lat, lng] list, "lat, lng" text or {lat, lng}
///   # or two properties:
///   latitude: lat
///   longitude: lng
///   defaultZoom: 5
///   center: [51.5, -0.12]
///   markerColor: "#f38ba8"
///   tiles: "~/maps/{z}/{x}/{y}.png"
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapOpts {
    pub coordinates: Option<String>,
    pub lat: Option<String>,
    pub lng: Option<String>,
    pub zoom: Option<f32>,
    pub center: Option<(f64, f64)>,
    pub color: Option<String>,
    /// A tile picture path with `{z}`, `{x}` and `{y}`.
    pub tiles: Option<String>,
}

impl MapOpts {
    pub fn from_yaml(v: &Y) -> MapOpts {
        let s = |k: &str| v.get(k).and_then(Y::as_str).map(str::to_string);
        MapOpts {
            coordinates: s("coordinates"),
            lat: s("latitude").or_else(|| s("lat")),
            lng: s("longitude").or_else(|| s("lng")),
            zoom: v.get("defaultZoom").and_then(Y::as_f64).map(|z| z as f32),
            center: v.get("center").and_then(Y::as_sequence).and_then(|c| Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))),
            color: s("markerColor"),
            tiles: s("tiles"),
        }
    }
}

/// One latitude and longitude out of a property's value.
pub fn coord(v: &Value) -> Option<(f64, f64)> {
    let ok = |lat: f64, lng: f64| ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lng)).then_some((lat, lng));
    match v {
        Value::List(l) if l.len() >= 2 => match (&l[0], &l[1]) {
            (Value::Num(a), Value::Num(b)) => ok(*a, *b),
            (a, b) => ok(a.to_string().trim().parse().ok()?, b.to_string().trim().parse().ok()?),
        },
        Value::Str(s) => {
            let (a, b) = s.split_once([',', ';']).or_else(|| s.split_once(' '))?;
            ok(a.trim().parse().ok()?, b.trim().parse().ok()?)
        }
        Value::Obj(m) => {
            let n = |k: &[&str]| k.iter().find_map(|k| m.get(*k)).and_then(|v| match v {
                Value::Num(n) => Some(*n),
                other => other.to_string().trim().parse().ok(),
            });
            ok(n(&["lat", "latitude"])?, n(&["lng", "lon", "long", "longitude"])?)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub note: usize,
    pub lat: f64,
    pub lng: f64,
    pub label: String,
}

/// The pins of a table: notes that have coordinates. The second number is
/// how many notes had none.
pub fn pins(index: &Index, t: &Table, o: &MapOpts) -> (Vec<Pin>, usize) {
    let mut out = Vec::new();
    let mut missing = 0;
    for g in &t.groups {
        for r in &g.rows {
            let note = &index.notes[r.note];
            let prop = |name: &str| note.props.get(name.strip_prefix("note.").unwrap_or(name)).cloned();
            let at = if let Some(c) = &o.coordinates {
                prop(c).and_then(|v| coord(&v))
            } else if let (Some(a), Some(b)) = (&o.lat, &o.lng) {
                match (prop(a), prop(b)) {
                    (Some(a), Some(b)) => coord(&Value::List(vec![a, b])),
                    _ => None,
                }
            } else {
                ["coordinates", "location", "coords", "geo"].iter().find_map(|k| note.props.get(*k).and_then(coord))
            };
            match at {
                Some((lat, lng)) => out.push(Pin { note: r.note, lat, lng, label: note.name.clone() }),
                None => missing += 1,
            }
        }
    }
    (out, missing)
}

const TILE: f64 = 256.0;
const MAX_LAT: f64 = 85.051_128_78;

/// World pixels at `zoom` (the whole world is `256 * 2^zoom` wide).
pub fn project(lat: f64, lng: f64, zoom: f64) -> (f64, f64) {
    let size = TILE * 2f64.powf(zoom);
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (lng + 180.0) / 360.0 * size;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0 * size;
    (x, y)
}

pub fn unproject(x: f64, y: f64, zoom: f64) -> (f64, f64) {
    let size = TILE * 2f64.powf(zoom);
    let lng = x / size * 360.0 - 180.0;
    let n = PI - 2.0 * PI * y / size;
    (n.sinh().atan().to_degrees(), lng)
}

/// Where a map is looking: the point at the middle of the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapView {
    pub lat: f64,
    pub lng: f64,
    pub zoom: f64,
}

pub const MAP_W: f32 = 1200.0;
pub const MAP_H: f32 = 760.0;

impl MapView {
    /// Show every pin (or the world when there are none).
    pub fn fit(pins: &[Pin], w: f32, h: f32, o: &MapOpts) -> MapView {
        if let Some((lat, lng)) = o.center {
            return MapView { lat, lng, zoom: f64::from(o.zoom.unwrap_or(4.0)) };
        }
        if pins.is_empty() {
            return MapView { lat: 20.0, lng: 0.0, zoom: f64::from(o.zoom.unwrap_or(1.0)) };
        }
        let (mut la0, mut la1, mut lo0, mut lo1) = (90.0f64, -90.0f64, 180.0f64, -180.0f64);
        for p in pins {
            la0 = la0.min(p.lat);
            la1 = la1.max(p.lat);
            lo0 = lo0.min(p.lng);
            lo1 = lo1.max(p.lng);
        }
        let (lat, lng) = ((la0 + la1) / 2.0, (lo0 + lo1) / 2.0);
        let zoom = if let Some(z) = o.zoom {
            f64::from(z)
        } else {
            let mut z = 16.0;
            while z > 0.0 {
                let (x0, y1) = project(la0, lo0, z);
                let (x1, y0) = project(la1, lo1, z);
                if (x1 - x0).abs() <= f64::from(w) * 0.8 && (y1 - y0).abs() <= f64::from(h) * 0.8 {
                    break;
                }
                z -= 0.5;
            }
            z
        };
        MapView { lat, lng, zoom }
    }

    /// Drag by picture pixels.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (cx, cy) = project(self.lat, self.lng, self.zoom);
        let (lat, lng) = unproject(cx - f64::from(dx), cy - f64::from(dy), self.zoom);
        self.lat = lat.clamp(-MAX_LAT, MAX_LAT);
        self.lng = ((lng + 180.0).rem_euclid(360.0)) - 180.0;
    }

    /// Zoom by `steps` (0.5 is a half step) keeping the point under (px, py).
    pub fn zoom_at(&mut self, steps: f64, px: f32, py: f32, w: f32, h: f32) {
        let before = self.at(px, py, w, h);
        self.zoom = (self.zoom + steps).clamp(0.0, 19.0);
        let (bx, by) = project(before.0, before.1, self.zoom);
        let (cx, cy) = (bx - f64::from(px) + f64::from(w) / 2.0, by - f64::from(py) + f64::from(h) / 2.0);
        let (lat, lng) = unproject(cx, cy, self.zoom);
        self.lat = lat.clamp(-MAX_LAT, MAX_LAT);
        self.lng = lng.clamp(-180.0, 180.0);
    }

    /// Picture pixels of a place.
    pub fn to_px(&self, lat: f64, lng: f64, w: f32, h: f32) -> (f32, f32) {
        let (cx, cy) = project(self.lat, self.lng, self.zoom);
        let (x, y) = project(lat, lng, self.zoom);
        ((x - cx) as f32 + w / 2.0, (y - cy) as f32 + h / 2.0)
    }

    /// The place under a picture pixel.
    pub fn at(&self, px: f32, py: f32, w: f32, h: f32) -> (f64, f64) {
        let (cx, cy) = project(self.lat, self.lng, self.zoom);
        unproject(cx + f64::from(px - w / 2.0), cy + f64::from(py - h / 2.0), self.zoom)
    }
}

/// Fill `{z}`, `{x}` and `{y}` in a tile path.
pub fn tile_path(template: &str, z: u32, x: i64, y: i64) -> String {
    template.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
}

fn graticule_step(zoom: f64) -> f64 {
    match zoom {
        z if z < 1.5 => 30.0,
        z if z < 3.0 => 10.0,
        z if z < 5.0 => 5.0,
        z if z < 7.0 => 1.0,
        z if z < 9.0 => 0.5,
        z if z < 12.0 => 0.1,
        _ => 0.01,
    }
}

/// The map: tiles (when a template is given and the pictures exist) or a
/// graticule, then a pin for each place. Pins are groups with id
/// `note:<index>`.
pub fn map_scene(pins: &[Pin], view: &MapView, w: f32, h: f32, o: &MapOpts, tiles_exist: &dyn Fn(&str) -> bool) -> Scene {
    let mut ops: Vec<Op> = Vec::new();
    let mut have_tiles = false;
    if let Some(t) = &o.tiles {
        let tz = (view.zoom.round() as i64).clamp(0, 19) as u32;
        let scale = 2f64.powf(view.zoom - f64::from(tz));
        let (cx, cy) = project(view.lat, view.lng, f64::from(tz));
        let half_w = f64::from(w) / 2.0 / scale;
        let half_h = f64::from(h) / 2.0 / scale;
        let n = 1i64 << tz;
        let (x0, x1) = (((cx - half_w) / TILE).floor() as i64, ((cx + half_w) / TILE).floor() as i64);
        let (y0, y1) = (((cy - half_h) / TILE).floor().max(0.0) as i64, ((cy + half_h) / TILE).floor().min((n - 1) as f64) as i64);
        if (x1 - x0 + 1) * (y1 - y0 + 1) <= 80 {
            for ty in y0..=y1 {
                for tx in x0..=x1 {
                    let path = tile_path(t, tz, tx.rem_euclid(n), ty);
                    if tiles_exist(&path) {
                        let px = ((tx as f64 * TILE - cx) * scale + f64::from(w) / 2.0) as f32;
                        let py = ((ty as f64 * TILE - cy) * scale + f64::from(h) / 2.0) as f32;
                        let size = (TILE * scale) as f32 + 0.5;
                        ops.push(Op::image(&path, px, py, size, size));
                        have_tiles = true;
                    }
                }
            }
        }
    }
    if !have_tiles {
        let step = graticule_step(view.zoom);
        let tl = view.at(0.0, 0.0, w, h);
        let br = view.at(w, h, w, h);
        let (lat_hi, lat_lo) = (tl.0.max(br.0), tl.0.min(br.0));
        let (lng_lo, lng_hi) = (tl.1.min(br.1), tl.1.max(br.1));
        let pal = forge_canvas::Palette::current();
        let grid = Stroke::new(&pal.line, 1.0);
        let strong = Stroke::new(&pal.dim, 1.5);
        let mut lat = (lat_lo / step).floor() * step;
        let mut guard = 0;
        while lat <= lat_hi + step && guard < 200 {
            let (_, y) = view.to_px(lat, view.lng, w, h);
            ops.push(Op::line(0.0, y, w, y, if lat.abs() < 1e-9 { strong.clone() } else { grid.clone() }));
            ops.push(Op::text(4.0, y - 14.0, &format!("{lat:.2}").trim_end_matches('0').trim_end_matches('.').to_string(), 11.0, &pal.dim));
            lat += step;
            guard += 1;
        }
        let mut lng = (lng_lo / step).floor() * step;
        guard = 0;
        while lng <= lng_hi + step && guard < 200 {
            let (x, _) = view.to_px(view.lat, lng, w, h);
            ops.push(Op::line(x, 0.0, x, h, if lng.abs() < 1e-9 { strong.clone() } else { grid.clone() }));
            ops.push(Op::text(x + 4.0, h - 16.0, &format!("{lng:.2}").trim_end_matches('0').trim_end_matches('.').to_string(), 11.0, &pal.dim));
            lng += step;
            guard += 1;
        }
    }
    let color = o.color.clone().unwrap_or_else(|| forge_canvas::Palette::current().accent(3).to_string());
    let label_all = pins.len() <= 40 || view.zoom >= 6.0;
    for p in pins {
        let (x, y) = view.to_px(p.lat, p.lng, w, h);
        if x < -20.0 || y < -40.0 || x > w + 20.0 || y > h + 20.0 {
            continue;
        }
        let mut g = vec![
            Op::path("M0 0 L-7 -14 L7 -14 Z").fill(color.as_str()),
            Op::ellipse(0.0, -19.0, 9.0, 9.0).fill(color.as_str()).stroked(Stroke::new(&forge_canvas::Palette::current().sunken, 1.5)),
            Op::ellipse(0.0, -19.0, 3.5, 3.5).fill(&forge_canvas::Palette::current().sunken),
        ];
        if label_all {
            g.push(Op::text(13.0, -25.0, &p.label, 12.0, &forge_canvas::Palette::current().fg));
        }
        ops.push(Op::group(g).transformed(Transform::translate(x, y)).with_id(&format!("note:{}", p.note)));
    }
    let mut s = Scene::new(w, h).background(&forge_canvas::Palette::current().sunken);
    s.ops = ops;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_in_every_shape() {
        assert_eq!(coord(&Value::List(vec![Value::Num(51.5), Value::Num(-0.12)])), Some((51.5, -0.12)));
        assert_eq!(coord(&Value::Str("10.5, 61.2".into())), Some((10.5, 61.2)));
        let mut m = std::collections::BTreeMap::new();
        m.insert("lat".to_string(), Value::Num(1.0));
        m.insert("lng".to_string(), Value::Num(2.0));
        assert_eq!(coord(&Value::Obj(m)), Some((1.0, 2.0)));
        assert_eq!(coord(&Value::Str("95, 0".into())), None);
        assert_eq!(coord(&Value::Str("hello".into())), None);
    }

    #[test]
    fn projection_round_trips() {
        for (lat, lng, z) in [(10.65, -61.5, 8.0), (-33.9, 151.2, 3.5), (0.0, 0.0, 1.0)] {
            let (x, y) = project(lat, lng, z);
            let (la, lo) = unproject(x, y, z);
            assert!((la - lat).abs() < 1e-6 && (lo - lng).abs() < 1e-6, "{la} {lo}");
        }
        // The equator and the prime meridian are the middle of the world.
        let (x, y) = project(0.0, 0.0, 1.0);
        assert!((x - 256.0).abs() < 1e-6 && (y - 256.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut v = MapView { lat: 10.0, lng: 20.0, zoom: 4.0 };
        let before = v.at(300.0, 200.0, 1200.0, 760.0);
        v.zoom_at(1.0, 300.0, 200.0, 1200.0, 760.0);
        let after = v.at(300.0, 200.0, 1200.0, 760.0);
        assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6);
        assert_eq!(v.zoom, 5.0);
    }

    #[test]
    fn pan_moves_the_view_opposite_the_drag() {
        let mut v = MapView { lat: 0.0, lng: 0.0, zoom: 3.0 };
        v.pan(100.0, 0.0);
        assert!(v.lng < 0.0);
    }

    #[test]
    fn fit_shows_every_pin_and_a_scene_has_hittable_pins() {
        let pins = vec![
            Pin { note: 0, lat: 10.6, lng: -61.5, label: "Port of Spain".into() },
            Pin { note: 1, lat: 51.5, lng: -0.1, label: "London".into() },
        ];
        let o = MapOpts::default();
        let v = MapView::fit(&pins, MAP_W, MAP_H, &o);
        for p in &pins {
            let (x, y) = v.to_px(p.lat, p.lng, MAP_W, MAP_H);
            assert!(x >= 0.0 && x <= MAP_W && y >= 0.0 && y <= MAP_H, "{x} {y}");
        }
        let s = map_scene(&pins, &v, MAP_W, MAP_H, &o, &|_| false);
        let (x, y) = v.to_px(51.5, -0.1, MAP_W, MAP_H);
        assert_eq!(s.hit(x, y - 15.0).as_deref(), Some("note:1"));
    }

    #[test]
    fn tiles_are_used_when_the_pictures_exist() {
        let o = MapOpts { tiles: Some("/t/{z}/{x}/{y}.png".into()), ..MapOpts::default() };
        let v = MapView { lat: 0.0, lng: 0.0, zoom: 1.0 };
        let s = map_scene(&[], &v, 512.0, 512.0, &o, &|p| p.starts_with("/t/1/"));
        assert!(s.to_json().contains("/t/1/"));
        let none = map_scene(&[], &v, 512.0, 512.0, &o, &|_| false);
        assert!(!none.to_json().contains("/t/"));
    }
}
