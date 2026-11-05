// SPDX-License-Identifier: MIT OR Apache-2.0
//! Colours: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()` and names.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0.0 to 1.0.
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 1.0,
    };
    pub const TRANSPARENT: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 0.0,
    };

    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self {
            a: (self.a * a).clamp(0.0, 1.0),
            ..self
        }
    }

    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim();
        if let Some(h) = s.strip_prefix('#') {
            return Self::hex(h);
        }
        let lower = s.to_ascii_lowercase();
        if let Some(args) = lower
            .strip_prefix("rgba(")
            .or_else(|| lower.strip_prefix("rgb("))
            .and_then(|r| r.strip_suffix(')'))
        {
            let parts: Vec<&str> = args.split(',').map(str::trim).collect();
            if parts.len() < 3 {
                return None;
            }
            let c = |i: usize| {
                parts[i]
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 255.0) as u8)
            };
            let a = parts
                .get(3)
                .and_then(|a| a.parse::<f32>().ok())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            return Some(Color {
                r: c(0)?,
                g: c(1)?,
                b: c(2)?,
                a,
            });
        }
        let (r, g, b) = match lower.as_str() {
            "transparent" | "none" => return Some(Color::TRANSPARENT),
            "black" => (0, 0, 0),
            "white" => (255, 255, 255),
            "red" => (230, 60, 60),
            "orange" => (240, 140, 40),
            "yellow" => (235, 200, 50),
            "green" => (70, 180, 90),
            "cyan" => (50, 190, 200),
            "blue" => (60, 120, 230),
            "purple" => (150, 90, 220),
            "pink" => (230, 100, 170),
            "gray" | "grey" => (128, 128, 128),
            _ => return None,
        };
        Some(Color::new(r, g, b))
    }

    fn hex(h: &str) -> Option<Color> {
        let v = |i: usize, n: usize| u8::from_str_radix(h.get(i..i + n)?, 16).ok();
        match h.len() {
            3 => Some(Color::new(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
            6 => Some(Color::new(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
            8 => Some(Color {
                r: v(0, 2)?,
                g: v(2, 2)?,
                b: v(4, 2)?,
                a: f32::from(v(6, 2)?) / 255.0,
            }),
            _ => None,
        }
    }

    /// `#rrggbbaa`: what the frontends that draw natively read.
    pub fn hex_rgba(&self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            self.r,
            self.g,
            self.b,
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8
        )
    }

    /// `#rrggbb`, with `fill-opacity` kept apart by the caller.
    pub fn hex_rgb(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse() {
        assert_eq!(Color::parse("#fff"), Some(Color::new(255, 255, 255)));
        assert_eq!(Color::parse("#1a2b3c"), Some(Color::new(0x1a, 0x2b, 0x3c)));
        assert_eq!(
            Color::parse("#00000080").unwrap().a.round(),
            1.0_f32.min(0.5_f32.round())
        );
        assert_eq!(Color::parse("rgba(10, 20, 30, 0.5)").unwrap().a, 0.5);
        assert_eq!(Color::parse("rgb(1,2,3)"), Some(Color::new(1, 2, 3)));
        assert_eq!(Color::parse("blue").unwrap().b, 230);
        assert!(Color::parse("#12").is_none());
        assert!(Color::parse("nope").is_none());
    }
}
