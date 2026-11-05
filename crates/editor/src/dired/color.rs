// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

/// Luminance adjustment. Only the lightness moves, on an exponential
/// ramp, so a shade is a paler or stronger version of the colour it starts
/// from rather than a different colour. Oklab is what makes the ramp hold
/// up across hues: scaling RGB channels would send yellow through orange.
pub(super) fn shade(base: u32, x: f64) -> u32 {
    let (l, m, s) = to_lms(base);
    let (_, a, b) = to_oklab(l.cbrt(), m.cbrt(), s.cbrt());
    // Oklab's L runs from 0 for black to 1 for white.
    let lightness =
        (MIN_LUMINANCE + (1.0 - MIN_LUMINANCE) * (-4.0 * (1.0 - x)).exp()).clamp(0.0, 1.0);
    let (r, g, b) = from_oklab(lightness, a, b);
    pack(r, g, b)
}
/// A packed 0xRRGGBB colour as terminal colours are: the eight ANSI
/// colours and bright green are full-strength, rather
/// than whatever a terminal happens to have made of them.
pub(super) fn base_rgb(face: DiredFace) -> u32 {
    match face {
        DiredFace::Size | DiredFace::GitNew => 0x00ff00,
        DiredFace::SizeMega => 0xffff00,
        DiredFace::SizeGiga | DiredFace::Compressed => 0xff0000,
        DiredFace::SizeHuge | DiredFace::Image => 0x800080,
        DiredFace::Date => 0x0000ff,
        _ => 0xffffff,
    }
}
pub(super) fn channels(base: u32) -> (f64, f64, f64) {
    let c = |shift: u32| f64::from(((base >> shift) & 0xff) as u8) / 255.0;
    (c(16), c(8), c(0))
}
pub(super) fn pack(r: f64, g: f64, b: f64) -> u32 {
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (byte(r) << 16) | (byte(g) << 8) | byte(b)
}
/// An sRGB channel to linear, and back.
pub(super) fn to_linear(v: f64) -> f64 {
    match v <= 0.040_45 {
        true => v / 12.92,
        false => ((v + 0.055) / 1.055).powf(2.4),
    }
}
pub(super) fn from_linear(v: f64) -> f64 {
    match v <= 0.003_130_8 {
        true => v * 12.92,
        false => 1.055 * v.powf(1.0 / 2.4) - 0.055,
    }
}
/// A packed colour's linear RGB, as LMS.
#[rustfmt::skip]
pub(super) fn to_lms(base: u32) -> (f64, f64, f64) {
    let (r, g, b) = channels(base);
    let (r, g, b) = (to_linear(r), to_linear(g), to_linear(b));
    (
        0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b,
        0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b,
        0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b,
    )
}
/// The cube roots of the LMS channels, as Oklab.
#[rustfmt::skip]
pub(super) fn to_oklab(l: f64, m: f64, s: f64) -> (f64, f64, f64) {
    (
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    )
}
/// Oklab back to a packed colour.
#[rustfmt::skip]
pub(super) fn from_oklab(lightness: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let (l, m, s) = (
        (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3),
        (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3),
        (lightness - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3),
    );
    let (r, g, b) = (
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
    );
    (from_linear(r), from_linear(g), from_linear(b))
}
/// The face a size is drawn in, by magnitude.
pub(super) fn size_face(bytes: u64) -> DiredFace {
    match bytes {
        _ if bytes >= 1_000_000_000_000 => DiredFace::SizeHuge,
        _ if bytes >= 1_000_000_000 => DiredFace::SizeGiga,
        _ if bytes >= 1_000_000 => DiredFace::SizeMega,
        _ => DiredFace::Size,
    }
}

// ------------------------------------------------------------- decorating
