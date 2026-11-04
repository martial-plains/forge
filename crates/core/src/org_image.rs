// SPDX-License-Identifier: MIT OR Apache-2.0
//! How wide and where to draw an Org image, and its caption: the display
//! rules for `org-image-actual-width`, `org-image-max-width`,
//! `org-image-align`, the `#+ATTR_*` and `#+CAPTION`/`#+NAME` affiliated
//! keywords, and the `ORG-IMAGE-ACTUAL-WIDTH` subtree property.
//!
//! Nothing here touches pixels; it works out what a frontend should do,
//! and `display.rs` passes the result along.

use crate::Buffer;
use crate::org_display::{ActualWidth, AttrWidth, ImageAlign, MaxWidth};
use std::path::PathBuf;

/// An affiliated keyword (`#+KEY: value`) directly above a link: the
/// nearest one per key wins.
#[derive(Clone, Debug, Default)]
pub struct Attrs {
    pub caption: Option<String>,
    pub name: Option<String>,
    /// `#+ATTR_ORG` values, then the others (`#+ATTR_HTML`, ...) under it.
    attrs: Vec<(String, String)>,
    /// Keys seen, most recent first, so `#+ATTR_ORG` can override.
    order: Vec<String>,
}

impl Attrs {
    /// The value of attribute `key` (`:width`, `:align`, ...). `ATTR_ORG`
    /// wins over the other `#+ATTR_*` keys.
    pub fn attr(&self, key: &str) -> Option<&str> {
        let want = key.trim_start_matches(':').to_ascii_lowercase();
        // An ATTR_ORG line wins over the rest.
        for (k, v) in self.attrs.iter().rev() {
            if k == "ATTR_ORG" {
                if let Some(val) = parse_attr_values(v).get(&want) {
                    return Some(val);
                }
            }
        }
        for (k, v) in self.attrs.iter().rev() {
            if k != "ATTR_ORG"
                && let Some(val) = parse_attr_values(v).get(&want)
            {
                return Some(val);
            }
        }
        None
    }

    pub fn has_any(&self) -> bool {
        !self.attrs.is_empty()
    }
}

/// His `:width 300px :align center` into pairs.
fn parse_attr_values(v: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut it = v.split_whitespace();
    while let Some(tok) = it.next() {
        if let Some(key) = tok.strip_prefix(':') {
            let val = it.next().unwrap_or("").to_string();
            out.push((key.to_ascii_lowercase(), val));
        }
    }
    out
}

fn lookup<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Read `#+KEY: value` lines immediately above `line` (the link's line),
/// stopping at the first non-affiliated, non-blank line or a blank line
/// that already ended the run.
fn affiliated(buf: &Buffer, line: usize) -> Attrs {
    let mut a = Attrs::default();
    let mut seen_blank = false;
    let mut li = line;
    while li > 0 {
        li -= 1;
        let text = buf.line(li);
        let t = text.trim();
        if t.is_empty() {
            if seen_blank {
                break;
            }
            seen_blank = true;
            continue;
        }
        if let Some(rest) = strip_keyword(t) {
            let (key, value) = rest;
            match key.to_ascii_uppercase().as_str() {
                "CAPTION" => {
                    if a.caption.is_none() {
                        a.caption = Some(value.trim().to_string());
                    }
                }
                "NAME" => {
                    if a.name.is_none() {
                        a.name = Some(value.trim().to_string());
                    }
                }
                k if k.starts_with("ATTR_") => {
                    if !a.order.contains(&k.to_string()) {
                        a.order.push(k.to_string());
                    }
                    a.attrs.push((k.to_string(), value.trim().to_string()));
                }
                _ => break,
            }
            continue;
        }
        // Any other text ends the run of affiliated keywords.
        break;
    }
    a
}

/// `#+KEY: value` -> (`KEY`, `value`), case-insensitively.
fn strip_keyword(t: &str) -> Option<(&str, &str)> {
    let rest = t.strip_prefix("#+")?;
    let colon = rest.find(':')?;
    let key = rest[..colon].trim_end();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key, &rest[colon + 1..]))
}

/// The width `#+ATTR_*` asks for, if any (`:width 300px`, `50%`, `80`).
fn attr_width(a: &Attrs) -> Option<AttrWidth> {
    let v = a.attr("width")?;
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse().ok().map(AttrWidth::Percent);
    }
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: usize = digits.parse().ok()?;
    let unit = v[digits.len()..].trim().to_ascii_lowercase();
    match unit.as_str() {
        "" | "px" => Some(AttrWidth::Pixels(n)),
        "em" | "ex" | "ch" | "rem" => Some(AttrWidth::Columns(n)),
        _ => None,
    }
}

/// The alignment `#+ATTR_*` asks for (`:align left/center/right`), or the
/// `:center` flag some backends use.
fn attr_align(a: &Attrs) -> Option<ImageAlign> {
    if a.attr("align")
        .and_then(|v| ImageAlign::from_name(&v.to_ascii_lowercase()))
        .is_some()
    {
        return a
            .attr("align")
            .and_then(|v| ImageAlign::from_name(&v.to_ascii_lowercase()));
    }
    // `:center t/yes` and friends.
    if let Some(v) = a.attr("center")
        && matches!(
            v.to_ascii_lowercase().as_str(),
            "t" | "true" | "yes" | "y" | ""
        )
    {
        return Some(ImageAlign::Center);
    }
    None
}

/// The width of an image in pixels (`image_size`), or None.
fn natural_width(path: &PathBuf) -> Option<usize> {
    crate::org_display::image_size(path).map(|(w, _)| w as usize)
}

/// A standalone image on `line`: its path, the width and alignment to draw
/// with, and its caption. `avail` is the window width in columns, used for
/// percentage and text-unit widths and the `window` cap.
pub struct OrgImage {
    pub path: PathBuf,
    /// Target width: pixels or columns. `None` means "use the natural
    /// width, capped by max_width".
    pub px: Option<usize>,
    pub cols: Option<usize>,
    pub align: ImageAlign,
    pub caption: Option<String>,
}

/// Work out how to draw the image linked on `line`, or None when the line
/// has no standalone image link.
pub fn image_at(buf: &Buffer, line: usize, avail: usize) -> Option<OrgImage> {
    let path = buf.org_line_image(line)?;
    let a = affiliated(buf, line);
    let natural = natural_width(&path);

    // org-image-actual-width: the buffer setting, or the subtree property.
    let setting = buf
        .org_heading_at(line)
        .and_then(|h| buf.org_get_property(h, "ORG-IMAGE-ACTUAL-WIDTH", true))
        .and_then(|v| parse_actual_width(&v))
        .unwrap_or(buf.org_display.image_actual_width);

    let aw = attr_width(&a);
    // Pixels the setting asks for, when it pins a number.
    let set_px = match setting {
        ActualWidth::Actual => None,
        ActualWidth::Pixels(n) => Some(n),
        ActualWidth::PixelsOrActual(n) => match aw {
            Some(AttrWidth::Pixels(p)) => Some(p),
            Some(AttrWidth::Columns(c)) => Some(c),
            Some(AttrWidth::Percent(p)) => Some(avail.saturating_mul(p) / 100),
            None => Some(n),
        },
    };

    let mut px = set_px;
    let mut cols = None;
    if px.is_none() {
        match aw {
            Some(AttrWidth::Pixels(p)) => px = Some(p),
            Some(AttrWidth::Columns(c)) => cols = Some(c),
            Some(AttrWidth::Percent(p)) => cols = Some(avail.saturating_mul(p) / 100),
            None => {}
        }
    }
    // With actual width (the default), cap by org-image-max-width.
    if setting == ActualWidth::Actual
        && let Some(nw) = natural
        && px.is_none()
        && cols.is_none()
    {
        match buf.org_display.image_max_width {
            MaxWidth::FillColumn => {
                // fill-column characters; a character is ~10px wide.
                let cap = buf.fill_column.saturating_mul(10);
                if nw > cap {
                    px = Some(cap);
                }
            }
            MaxWidth::Window => {
                let cap = avail.saturating_mul(10);
                if nw > cap {
                    px = Some(cap);
                }
            }
            MaxWidth::Pixels(n) => {
                if nw > n {
                    px = Some(n);
                }
            }
            MaxWidth::None => {}
        }
    }

    let align = attr_align(&a).unwrap_or(buf.org_display.image_align);
    Some(OrgImage {
        path,
        px,
        cols,
        align,
        caption: a.caption,
    })
}

fn parse_actual_width(v: &str) -> Option<ActualWidth> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    if v.eq_ignore_ascii_case("nil") {
        return Some(ActualWidth::PixelsOrActual(0));
    }
    if v.eq_ignore_ascii_case("t") {
        return Some(ActualWidth::Actual);
    }
    // "300" or "(300)".
    let inner = v.trim_matches(|c| c == '(' || c == ')');
    if let Some(n) = inner.split_whitespace().next().and_then(|n| n.parse().ok()) {
        return Some(if inner.split_whitespace().count() > 1 {
            ActualWidth::PixelsOrActual(n)
        } else {
            ActualWidth::Pixels(n)
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(t: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(t);
        b.set_major_mode(crate::org::MODE);
        b
    }

    #[test]
    fn reads_affiliated_caption_name_and_attrs() {
        let dir = std::env::temp_dir().join(format!("forge-imga-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 40x20 PNG header.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&40u32.to_be_bytes());
        png.extend_from_slice(&20u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        std::fs::write(dir.join("p.png"), &png).unwrap();
        let mut b = org(
            "#+CAPTION: A cat\n#+NAME: fig:cat\n#+ATTR_HTML: :width 300px :align right\n#+ATTR_ORG: :align center\n[[file:p.png]]\n",
        );
        b.set_visited_file(dir.join("t.org"));
        b.org_display.link_preview = true;
        let a = affiliated(&b, 4);
        assert_eq!(a.caption.as_deref(), Some("A cat"));
        assert_eq!(a.name.as_deref(), Some("fig:cat"));
        assert_eq!(a.attr("align"), Some("center"), "ATTR_ORG wins");
        assert_eq!(attr_width(&a), Some(AttrWidth::Pixels(300)));
        assert_eq!(attr_align(&a), Some(ImageAlign::Center));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn width_from_attr_percent_and_units() {
        let a = Attrs {
            attrs: vec![("ATTR_HTML".into(), ":width 50%".into())],
            ..Default::default()
        };
        assert_eq!(attr_width(&a), Some(AttrWidth::Percent(50)));
        let a = Attrs {
            attrs: vec![("ATTR_HTML".into(), ":width 12em".into())],
            ..Default::default()
        };
        assert_eq!(attr_width(&a), Some(AttrWidth::Columns(12)));
    }

    #[test]
    fn parses_actual_width_forms() {
        assert_eq!(parse_actual_width("t"), Some(ActualWidth::Actual));
        assert_eq!(parse_actual_width("300"), Some(ActualWidth::Pixels(300)));
        assert_eq!(
            parse_actual_width("(300)"),
            Some(ActualWidth::Pixels(300))
        );
        assert_eq!(
            parse_actual_width("(300 400)"),
            Some(ActualWidth::PixelsOrActual(300))
        );
    }
}
