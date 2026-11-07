//! The device window policy (#326, job 326-X; the maintainer's decision of 2026-11-07), shared by
//! every device panel that migrates to the design language's device window: the Macro Oscillator
//! first, the Compressor and the other panels as they follow. One implementation, plain functions
//! over data; each panel supplies only what is its own (its design and minimum widths, its
//! content's height, its footer).
//!
//! - **Content-sized and fixed** (Ableton's device windows, Logic's plug-in windows): the panel's
//!   design width, less a margin a side on a narrower screen and never below the panel's minimum;
//!   the title bar, the content and the footer high (with the frame's edge), at most the screen.
//!   The caller recomputes it every frame, so a section that opens or closes re-fits the window
//!   that frame. No resize handle ([`configure`]: a fixed size, never a default size).
//! - **Anchored at its top left:** when the bottom would leave the screen the window moves up
//!   ([`anchored`]); when it still does not fit, the caller's body scrolls.
//! - **Position memory:** per instance for the session ([`Positions`], keyed by the insert's uid,
//!   which no restart keeps) and per device across launches ([`DevicePositions`],
//!   `UiState::device_windows` in `ui.json`, written when a window closes). Only the top left is
//!   kept; an entry written before 326-X carries a size too, which [`read_device_positions`] reads
//!   past (a tolerant reader, no schema bump).

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use std::collections::{BTreeMap, HashMap};

/// Room kept between the window and the screen's sides when the screen is narrower than the
/// panel's design width.
pub const SCREEN_MARGIN: f32 = crate::tokens::SPACE[5];
/// Each further window opens this much down and to the right of the one before.
pub const CASCADE: f32 = crate::tokens::SPACE[4];

/// Each instance's last position (the window's top left) by insert uid, for this launch.
pub type Positions = HashMap<u64, Pos2>;
/// Each device's last closed position by device (plug-in) id, across launches (`ui.json`).
pub type DevicePositions = BTreeMap<String, [f32; 2]>;

/// What a panel tells the policy about itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    /// The width the panel is designed at (the board's).
    pub design: f32,
    /// The narrowest its content lays out.
    pub min: f32,
}

/// The frame's edge (margins and stroke) around the content, both sides summed.
pub fn edge() -> Vec2 {
    crate::theme::device_panel_frame().total_margin().sum()
}

/// The window's width on `screen`: the design width, less a margin a side on a narrower screen,
/// never below the minimum.
pub fn width(widths: Widths, screen: Rect) -> f32 {
    widths.design.min(screen.width() - 2.0 * SCREEN_MARGIN).max(widths.min)
}

/// The window's size on `screen` for content `content_h` tall between a title bar `title_h` and a
/// footer `footer_h` tall: [`width`] wide, the three and the frame's edge high, at most the screen.
pub fn size(widths: Widths, screen: Rect, title_h: f32, content_h: f32, footer_h: f32) -> Vec2 {
    vec2(width(widths, screen), (title_h + content_h + footer_h + edge().y).min(screen.height()))
}

/// The size of a window collapsed to its title bar `title_h` high (326-V): [`width`] wide, the bar
/// and the frame's edge high.
pub fn collapsed_size(widths: Widths, screen: Rect, title_h: f32) -> Vec2 {
    vec2(width(widths, screen), title_h + edge().y)
}

/// `rect` moved and shrunk to lie on `screen`.
pub fn on_screen(rect: Rect, screen: Rect) -> Rect {
    let size = rect.size().min(screen.size());
    let x = rect.min.x.clamp(screen.min.x, screen.max.x - size.x);
    let y = rect.min.y.clamp(screen.min.y, screen.max.y - size.y);
    Rect::from_min_size(pos2(x, y), size)
}

/// Where a window of `size` whose top left is `pos` sits this frame: kept, unless its bottom (or
/// its side) would leave `screen`, which moves it up (or across) onto the screen.
pub fn anchored(pos: Pos2, size: Vec2, screen: Rect) -> Pos2 {
    on_screen(Rect::from_min_size(pos, size), screen).min
}

/// `fallback` moved 24 px down and right while its corner sits on an open window's (at most a
/// screen's worth of steps).
pub fn cascaded(fallback: Rect, open: &[Rect]) -> Rect {
    let mut r = fallback;
    for _ in 0..64 {
        if !open.iter().any(|o| (o.min - r.min).length() < 1.0) {
            break;
        }
        r = r.translate(vec2(CASCADE, CASCADE));
    }
    r
}

/// Where a window of `size` opens (its top left): the instance's own remembered position, else
/// the device's (cascaded off the `open` windows), else centered across the screen a margin from
/// its top, cascaded by `n`; on screen either way.
pub fn opening_pos(own: Option<Pos2>, device: Option<[f32; 2]>, open: &[Rect], screen: Rect, size: Vec2, n: usize) -> Pos2 {
    let r = match (own, device) {
        (Some(o), _) => Rect::from_min_size(o, size),
        (None, Some([x, y])) => cascaded(Rect::from_min_size(pos2(x, y), size), open),
        (None, None) => {
            let off = CASCADE * n as f32;
            Rect::from_min_size(pos2(screen.center().x - size.x / 2.0 + off, screen.min.y + SCREEN_MARGIN + off), size)
        }
    };
    anchored(r.min, r.size(), screen)
}

/// The device window's builder options: at `pos` (the caller moves it from its own bar), exactly
/// `size`, no resize handle.
pub fn configure(win: crate::title_bar::Window<'_>, pos: Pos2, size: Vec2) -> crate::title_bar::Window<'_> {
    win.current_pos(pos).movable(false).resizable(false).fixed_size(size)
}

/// Reads `UiState::device_windows`: each entry's first two numbers, the top left. An entry from
/// before 326-X is a rect (`[min x, min y, max x, max y]`); its size is read past. An entry with
/// fewer than two numbers is dropped.
pub fn read_device_positions<'de, D: serde::Deserializer<'de>>(d: D) -> Result<DevicePositions, D::Error> {
    let raw: BTreeMap<String, Vec<f32>> = serde::Deserialize::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|(k, v)| (v.len() >= 2).then(|| (k, [v[0], v[1]]))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { min: Pos2::ZERO, max: Pos2 { x: 1400.0, y: 900.0 } };
    const W: Widths = Widths { design: 1440.0, min: 1200.0 };

    #[test]
    fn the_size_is_the_design_width_on_screen_and_the_content_height_at_most_the_screen() {
        let e = edge();
        // A narrower screen than the design: a margin a side.
        let s = size(W, SCREEN, 52.0, 600.0, 32.0);
        assert_eq!(s, vec2(1400.0 - 2.0 * SCREEN_MARGIN, 52.0 + 600.0 + 32.0 + e.y));
        // A wide screen: the design width. Negative control: never wider.
        let wide = Rect::from_min_size(Pos2::ZERO, vec2(2560.0, 1440.0));
        assert_eq!(size(W, wide, 52.0, 600.0, 32.0).x, 1440.0);
        // A screen narrower than the minimum keeps the minimum.
        let narrow = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 900.0));
        assert_eq!(width(W, narrow), 1200.0);
        // Content taller than the screen: the screen's height (the body scrolls).
        assert_eq!(size(W, SCREEN, 52.0, 2000.0, 32.0).y, 900.0);
        // Collapsed: the bar and the edge only, at the same width.
        assert_eq!(collapsed_size(W, SCREEN, 52.0), vec2(size(W, SCREEN, 52.0, 600.0, 32.0).x, 52.0 + e.y));
        // Content grows by d: the window grows by exactly d.
        let (a, b) = (size(W, SCREEN, 52.0, 400.0, 32.0), size(W, SCREEN, 52.0, 523.0, 32.0));
        assert_eq!(b.y - a.y, 123.0);
    }

    #[test]
    fn the_window_keeps_its_top_left_and_moves_up_when_its_bottom_would_leave_the_screen() {
        let at = pos2(100.0, 200.0);
        assert_eq!(anchored(at, vec2(1200.0, 600.0), SCREEN), at, "fits: kept");
        assert_eq!(anchored(at, vec2(1200.0, 800.0), SCREEN), pos2(100.0, 100.0), "moved up by the overflow");
        assert_eq!(anchored(at, vec2(1200.0, 1200.0), SCREEN), pos2(100.0, 0.0), "taller than the screen: at its top");
        assert_eq!(anchored(pos2(500.0, 0.0), vec2(1200.0, 400.0), SCREEN), pos2(200.0, 0.0), "and across");
    }

    #[test]
    fn a_window_opens_at_its_own_position_else_the_devices_cascaded_else_centered() {
        let s = vec2(1200.0, 600.0);
        assert_eq!(opening_pos(Some(pos2(40.0, 50.0)), Some([300.0, 300.0]), &[], SCREEN, s, 0), pos2(40.0, 50.0));
        let open = [Rect::from_min_size(pos2(60.0, 70.0), s)];
        assert_eq!(opening_pos(None, Some([60.0, 70.0]), &open, SCREEN, s, 0), pos2(84.0, 94.0), "cascaded off the open one");
        assert_eq!(opening_pos(None, None, &[], SCREEN, s, 1), pos2(100.0 + CASCADE, SCREEN_MARGIN + CASCADE));
        // Negative control: a position off the screen comes back onto it.
        assert_eq!(opening_pos(Some(pos2(1300.0, 850.0)), None, &[], SCREEN, s, 0), pos2(200.0, 300.0));
        let r = Rect::from_min_size(pos2(32.0, 32.0), vec2(100.0, 100.0));
        assert_eq!(cascaded(r, &[]).min, r.min);
        assert_eq!(cascaded(r, &[r, r.translate(vec2(24.0, 24.0))]).min, pos2(80.0, 80.0));
    }

    #[test]
    fn the_device_positions_read_an_old_rect_entry_as_its_top_left() {
        #[derive(serde::Deserialize)]
        struct S {
            #[serde(deserialize_with = "read_device_positions")]
            m: DevicePositions,
        }
        let s: S = serde_json::from_value(serde_json::json!({"m": {"old": [100.0, 80.0, 1200.0, 700.0], "new": [5.0, 6.0], "bad": [1.0]}})).unwrap();
        assert_eq!(s.m.get("old"), Some(&[100.0, 80.0]), "the size is read past");
        assert_eq!(s.m.get("new"), Some(&[5.0, 6.0]));
        assert!(!s.m.contains_key("bad"), "negative control: one number is no position");
        // What is written is the new shape.
        assert_eq!(serde_json::to_value(&s.m).unwrap(), serde_json::json!({"new": [5.0, 6.0], "old": [100.0, 80.0]}));
    }
}
