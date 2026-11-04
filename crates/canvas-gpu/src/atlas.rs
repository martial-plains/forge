// SPDX-License-Identifier: MIT OR Apache-2.0
//! A texture of glyph coverage, packed in rows. Glyphs are drawn once per
//! (character, size, weight) and reused.

use std::collections::HashMap;

use forge_canvas::raster;

pub const SIZE: u32 = 2048;

#[derive(Debug, Clone, Copy)]
pub struct Slot {
    /// Texel rectangle in the atlas.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Where the bitmap sits relative to the pen and baseline.
    pub xmin: i32,
    pub ymin: i32,
    pub advance: f32,
}

pub struct Atlas {
    pub pixels: Vec<u8>,
    slots: HashMap<(char, u16, bool), Option<Slot>>,
    row_y: u32,
    row_h: u32,
    x: u32,
    /// Pixels changed since the last upload.
    pub dirty: bool,
    /// Incremented when the atlas is emptied to make room.
    pub generation: u32,
}

impl Default for Atlas {
    fn default() -> Self {
        Atlas { pixels: vec![0; (SIZE * SIZE) as usize], slots: HashMap::new(), row_y: 0, row_h: 0, x: 0, dirty: true, generation: 0 }
    }
}

pub enum Lookup {
    Slot(Slot),
    /// No glyph (a space, or no font has it).
    Blank,
    /// The atlas is full; clear it and start the frame again.
    Full,
}

impl Atlas {
    pub fn clear(&mut self) {
        self.pixels.fill(0);
        self.slots.clear();
        self.row_y = 0;
        self.row_h = 0;
        self.x = 0;
        self.dirty = true;
        self.generation += 1;
    }

    pub fn glyph(&mut self, c: char, px: u16, bold: bool) -> Lookup {
        if let Some(s) = self.slots.get(&(c, px, bold)) {
            return s.map_or(Lookup::Blank, Lookup::Slot);
        }
        let Some(g) = raster::glyph(c, f32::from(px), bold) else {
            self.slots.insert((c, px, bold), None);
            return Lookup::Blank;
        };
        if g.width == 0 || g.height == 0 {
            // Spaces still advance.
            let slot = Slot { x: 0, y: 0, w: 0, h: 0, xmin: g.xmin, ymin: g.ymin, advance: g.advance };
            self.slots.insert((c, px, bold), Some(slot));
            return Lookup::Slot(slot);
        }
        let (w, h) = (g.width as u32 + 1, g.height as u32 + 1);
        if self.x + w > SIZE {
            self.row_y += self.row_h;
            self.row_h = 0;
            self.x = 0;
        }
        if self.row_y + h > SIZE || w > SIZE {
            return Lookup::Full;
        }
        for row in 0..g.height {
            let dst = ((self.row_y + row as u32) * SIZE + self.x) as usize;
            self.pixels[dst..dst + g.width].copy_from_slice(&g.bitmap[row * g.width..(row + 1) * g.width]);
        }
        let slot = Slot { x: self.x, y: self.row_y, w: g.width as u32, h: g.height as u32, xmin: g.xmin, ymin: g.ymin, advance: g.advance };
        self.x += w;
        self.row_h = self.row_h.max(h);
        self.slots.insert((c, px, bold), Some(slot));
        self.dirty = true;
        Lookup::Slot(slot)
    }
}
