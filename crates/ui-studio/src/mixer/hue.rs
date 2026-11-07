//! The bridge from a track's free RGB color to the content palette (#326): until tracks carry a
//! palette name in the model, a strip's color bar takes the palette hue nearest the track's RGB in
//! OKLCH hue angle, or Graphite when the RGB is too grey to have a hue. The sixteen hues come from
//! `palette.oklch` in tokens.json (`hueStart` 11, `hueStep` 22.5). When tracks store palette
//! names this module goes away and the bar reads the name.

use crate::tokens::palette;

/// Below this OKLCH chroma a color reads as grey and maps to Graphite. The palette's calmest
/// value (a light body) has 0.06; half of it keeps every palette swatch and every saturated track
/// color on a hue, and the UI greys (chroma under 0.02) on Graphite.
pub const CHROMA_FLOOR: f32 = 0.03;

/// The index of Graphite in the palette.
pub const GRAPHITE: usize = 16;

fn oklch(name: &str) -> f32 {
    palette::OKLCH.iter().find(|(k, _)| *k == name).map_or(0.0, |(_, v)| *v)
}

/// The sixteen palette hues in degrees, from tokens.json.
pub fn hues() -> [f32; 16] {
    let (start, step) = (oklch("hueStart"), oklch("hueStep"));
    std::array::from_fn(|i| start + step * i as f32)
}

/// OKLCH chroma and hue (degrees, 0..360) of an sRGB color.
pub fn chroma_hue(rgb: [u8; 3]) -> (f32, f32) {
    let lin = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    (a.hypot(bb), bb.atan2(a).to_degrees().rem_euclid(360.0))
}

/// The palette index (0..16 a hue, [`GRAPHITE`]) for `rgb` against the hue table `hues`.
pub fn nearest(rgb: [u8; 3], hues: &[f32; 16]) -> usize {
    let (chroma, hue) = chroma_hue(rgb);
    if chroma < CHROMA_FLOOR {
        return GRAPHITE;
    }
    let dist = |h: f32| {
        let d = (hue - h).rem_euclid(360.0);
        d.min(360.0 - d)
    };
    (0..16).min_by(|&i, &j| dist(hues[i]).total_cmp(&dist(hues[j]))).unwrap_or(GRAPHITE)
}

/// The color bar's fill for a track's RGB: the nearest hue's `bar` value in the current theme.
pub fn bar_for(rgb: [u8; 3]) -> egui::Color32 {
    let i = nearest(rgb, &hues());
    if crate::theme::current() == egui::Theme::Dark { palette::DARK_BAR[i] } else { palette::LIGHT_BAR[i] }
}
