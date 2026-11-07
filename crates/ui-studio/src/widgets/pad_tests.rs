//! The pad (README, Components > Pad): both sizes, every state the README names, each against a
//! negative control, and the hit's fall and the cut's 200 ms driven by fixed time steps.

use super::pad::{self, Pad, Size};
use super::test_support::{Rig, inside, mid, size};
use crate::theme::{DARK, LIGHT};

fn base(size: Size) -> Pad<'static> {
    Pad {
        size,
        id: "A6",
        name: "OH Hat",
        empty: false,
        choke: 0,
        tag: None,
        mute: false,
        solo: false,
        excluded: false,
        selected: false,
        peer: false,
        fill: 0.0,
        choked: false,
    }
}

fn draw(rig: &mut Rig, p: Pad<'static>) -> pad::PadLayout {
    rig.idle(|ui| pad::show(ui, egui::Id::new("pad"), egui::pos2(40.0, 40.0), &p).layout)
}

#[test]
fn both_sizes_are_square_with_the_id_top_left_and_the_name_bottom_left() {
    let mut rig = Rig::new();
    for (s, side) in [(Size::Grid, 56.0), (Size::Focus, 80.0)] {
        let l = draw(&mut rig, base(s));
        assert_eq!(size(l.rect), (side, side), "{s:?}");
        assert_eq!((l.fill, l.border, l.border_width, l.dashed), (DARK.surface_raised, DARK.line, 1.0, false));
        assert!(inside(l.id_rect, l.rect) && l.id_rect[0] - l.rect[0] <= 6.0 && l.id_rect[1] - l.rect[1] <= 7.0, "{s:?} id top-left");
        assert_eq!(l.id, "A6");
        assert!(inside(l.name_rect, l.rect) && l.name_rect[3] > l.rect[3] - 7.0, "{s:?} name at the bottom");
        assert_eq!((l.badge, l.tag, l.chip, l.hit, l.peer, l.drop_label), (None, None, None, None, None, None), "idle");
        assert_eq!(l.opacity, 1.0);
    }
    // The id is 9 at grid size and 10 when focused: the focused id is taller.
    let g = draw(&mut rig, base(Size::Grid));
    let f = draw(&mut rig, base(Size::Focus));
    assert!(size(f.id_rect).1 > size(g.id_rect).1, "negative control: the sizes differ in type");
}

#[test]
fn the_choke_badge_shows_the_group_and_inverts_when_choked() {
    let mut rig = Rig::new();
    let l = draw(&mut rig, Pad { choke: 1, ..base(Size::Grid) });
    let b = l.badge.expect("a grouped pad has a badge");
    assert!(size(b).0 >= 16.0 && size(b).1 == 14.0, "at least 16 by 14: {b:?}");
    assert_eq!(b[2], l.rect[2] - 4.0, "top-right");
    assert_eq!((l.badge_text.as_str(), l.badge_fill, l.badge_ink), ("1", egui::Color32::TRANSPARENT, DARK.text_muted));
    let c = draw(&mut rig, Pad { choke: 1, choked: true, ..base(Size::Grid) });
    assert_eq!((c.badge_fill, c.badge_ink), (DARK.text_muted, DARK.surface_panel), "inverted");
    assert!(c.choked);
    // Negative controls: Off has no badge, and "choked" without a group shows nothing.
    assert_eq!(draw(&mut rig, base(Size::Grid)).badge, None);
    assert!(!draw(&mut rig, Pad { choked: true, ..base(Size::Grid) }).choked);
    let wide = draw(&mut rig, Pad { choke: 16, ..base(Size::Focus) });
    assert!(size(wide.badge.unwrap()).0 >= 16.0);
}

#[test]
fn the_color_tag_shows_only_once_a_group_has_a_color() {
    let mut rig = Rig::new();
    let red = crate::tokens::palette::DARK_BAR[0];
    let l = draw(&mut rig, Pad { choke: 2, tag: Some(red), ..base(Size::Grid) });
    let g = l.tag.expect("a colored group draws its tag");
    assert_eq!((size(g).1, l.tag_color), (3.0, red));
    assert_eq!((g[1], g[0]), (l.rect[1] + 1.0, l.rect[0] + 1.0), "the top edge, inside the border");
    // Negative controls: a group without a color, or a color without a group, draws no tag.
    assert_eq!(draw(&mut rig, Pad { choke: 2, ..base(Size::Grid) }).tag, None);
    assert_eq!(draw(&mut rig, Pad { tag: Some(red), ..base(Size::Grid) }).tag, None);
}

#[test]
fn mute_and_solo_chips_and_the_forty_percent_dim() {
    let mut rig = Rig::new();
    let m = draw(&mut rig, Pad { mute: true, ..base(Size::Grid) });
    assert_eq!((m.chip_letter, m.chip_fill, m.chip_ink, m.opacity), (Some('M'), DARK.text, DARK.bg_window, 0.4));
    assert_eq!(size(m.chip.unwrap()), (14.0, 14.0));
    let s = draw(&mut rig, Pad { solo: true, ..base(Size::Grid) });
    assert_eq!((s.chip_letter, s.chip_fill, s.chip_ink, s.opacity), (Some('S'), DARK.signal, DARK.surface_inset, 1.0));
    let x = draw(&mut rig, Pad { excluded: true, ..base(Size::Grid) });
    assert_eq!((x.opacity, x.chip), (0.4, None), "solo-excluded dims without a chip");
    // Negative controls: a plain pad is at full strength with no chip; an empty pad never dims.
    let plain = draw(&mut rig, base(Size::Grid));
    assert_eq!((plain.opacity, plain.chip), (1.0, None));
    assert_eq!(draw(&mut rig, Pad { empty: true, excluded: true, ..base(Size::Grid) }).opacity, 1.0);
    // The chip sits bottom-right, clear of the name.
    let c = m.chip.unwrap();
    assert!(c[0] >= m.name_rect[2] && c[3] == m.rect[3] - 4.0);
}

#[test]
fn selected_and_peer_outlines() {
    let mut rig = Rig::new();
    let s = draw(&mut rig, Pad { selected: true, ..base(Size::Grid) });
    assert_eq!((s.border, s.border_width), (DARK.signal, 2.0));
    let p = draw(&mut rig, Pad { peer: true, ..base(Size::Grid) });
    let o = p.peer.expect("a peer is outlined");
    assert_eq!(o, [p.rect[0] + 3.0, p.rect[1] + 3.0, p.rect[2] - 3.0, p.rect[3] - 3.0], "inset 3");
    // Negative controls: unselected has the 1 px line; a non-peer has no outline.
    assert_eq!(draw(&mut rig, base(Size::Grid)).border_width, 1.0);
    assert_eq!(draw(&mut rig, base(Size::Grid)).peer, None);
}

#[test]
fn an_empty_pad_is_dashed_and_reads_drop() {
    let mut rig = Rig::new();
    let (l, shapes) = rig.shapes(|ui| {
        pad::show(ui, egui::Id::new("pad"), egui::pos2(40.0, 40.0), &Pad { empty: true, choke: 3, mute: true, ..base(Size::Grid) }).layout
    });
    assert!(l.dashed && l.drop_label.is_some());
    assert_eq!((l.border, l.fill, l.name.as_str()), (DARK.line_strong, egui::Color32::TRANSPARENT, ""));
    assert_eq!((l.badge, l.chip, l.hit), (None, None, None), "an empty pad shows no group, chip or hit");
    let texts = painted(&shapes);
    assert!(texts.contains(&"+ Drop".to_string()), "{texts:?}");
    // The selected empty pad keeps the dashed edge (the board).
    assert!(draw(&mut rig, Pad { empty: true, selected: true, ..base(Size::Grid) }).dashed);
    // Negative control: a filled pad is not dashed and has no label.
    let f = draw(&mut rig, base(Size::Grid));
    assert!(!f.dashed && f.drop_label.is_none());
}

fn painted(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(s: &egui::Shape, out: &mut Vec<String>) {
        match s {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|s| walk(&s.shape, &mut out));
    out
}

#[test]
fn the_hit_rises_with_velocity_and_falls_over_250_ms_at_a_fixed_step() {
    // Fixed 1/60 s steps: the fill starts at the velocity's share and reaches nothing at 250 ms.
    // Ages come from the frame clock (f64 seconds), as the panel reads them.
    let age = |i: u32| (f64::from(i) * 1000.0 / 60.0) as f32;
    let heights: Vec<f32> = (0..20).map(|i| pad::hit_fill(127, age(i))).collect();
    assert_eq!(heights[0], 1.0);
    assert!(heights.windows(2).all(|w| w[1] <= w[0]), "it only falls: {heights:?}");
    assert!(heights[14] > 0.0 && heights[15] == 0.0, "gone at 250 ms (frame 15): {heights:?}");
    assert!((pad::hit_fill(64, 0.0) - 64.0 / 127.0).abs() < 1e-6, "height is the velocity");
    let mut rig = Rig::new();
    let l = draw(&mut rig, Pad { fill: pad::hit_fill(100, 0.0), ..base(Size::Grid) });
    let h = l.hit.expect("a hit draws its fill");
    assert_eq!(h[3], l.rect[3] - 1.0, "from the bottom, inside the edge");
    assert!((size(h).1 - 54.0 * 100.0 / 127.0).abs() < 0.02, "{h:?}");
    assert_eq!(l.hit_color, DARK.signal.gamma_multiply(0.26));
    // Negative controls: at rest there is no fill, and a later frame has a lower one.
    assert_eq!(draw(&mut rig, base(Size::Grid)).hit, None);
    let later = draw(&mut rig, Pad { fill: pad::hit_fill(100, age(6)), ..base(Size::Grid) });
    assert!(size(later.hit.unwrap()).1 < size(h).1);
}

#[test]
fn a_cut_drops_the_fill_at_once_and_inverts_the_badge_for_200_ms() {
    let age = |i: u32| (f64::from(i) * 1000.0 / 60.0) as f32;
    // Hit 5 frames ago, cut 2 frames ago: the fill is gone.
    assert_eq!(pad::live_fill(Some((127, age(5))), Some(age(2))), 0.0);
    // Negative control: a cut before the hit (an older one) leaves the new hit's fill.
    assert!(pad::live_fill(Some((127, age(2))), Some(age(5))) > 0.0);
    assert_eq!(pad::live_fill(None, None), 0.0);
    // 200 ms at 1/60 s: 12 frames inverted, the 13th (216.7 ms) back.
    let shown: Vec<bool> = (0..14).map(|i| pad::choked(age(i))).collect();
    assert!(shown[..12].iter().all(|x| *x) && !shown[12] && !shown[13], "{shown:?}");
    assert!(!pad::choked(-1.0), "a future cut is not shown");
}

#[test]
fn a_click_on_the_pad_is_reported() {
    let mut rig = Rig::new();
    let l = draw(&mut rig, base(Size::Grid));
    let clicked = rig.click(mid(l.rect), |ui| pad::show(ui, egui::Id::new("pad"), egui::pos2(40.0, 40.0), &base(Size::Grid)).response.clicked());
    assert!(clicked);
    let outside = rig.click(egui::pos2(l.rect[2] + 10.0, l.rect[1]), |ui| {
        pad::show(ui, egui::Id::new("pad"), egui::pos2(40.0, 40.0), &base(Size::Grid)).response.clicked()
    });
    assert!(!outside, "negative control: a click beside it is not its");
}

#[test]
fn the_light_theme_draws_the_same_roles() {
    let mut rig = Rig::themed(egui::Theme::Light);
    let l = rig.idle(|ui| {
        crate::theme::set_current(egui::Theme::Light);
        pad::show(ui, egui::Id::new("pad"), egui::pos2(40.0, 40.0), &Pad { selected: true, mute: true, ..base(Size::Focus) }).layout
    });
    assert_eq!((l.fill, l.border, l.chip_fill, l.chip_ink), (LIGHT.surface_raised, LIGHT.signal, LIGHT.text, LIGHT.bg_window));
}

#[test]
fn the_kits_numbers_are_the_tokens() {
    let m = |k: &str| crate::tokens::pad::METRICS.iter().find(|(p, _)| *p == k).unwrap_or_else(|| panic!("{k}")).1;
    let pairs = [
        (pad::GRID, "sizes/grid"),
        (pad::FOCUS, "sizes/focus"),
        (pad::GAP, "gap"),
        (pad::RADIUS, "radius"),
        (pad::HIT_ALPHA, "hit/alpha"),
        (pad::HIT_EDGE, "hit/edge"),
        (pad::HIT_DECAY_MS, "hit/decayMs"),
        (pad::SELECTED_WIDTH, "selected/width"),
        (pad::DIM, "muted/opacity"),
        (pad::DIM, "soloExcluded/opacity"),
        (pad::TAG_HEIGHT, "tag/height"),
        (pad::BADGE_MIN_W, "chokeBadge/minWidth"),
        (pad::BADGE_H, "chokeBadge/height"),
        (pad::BADGE_RADIUS, "chokeBadge/radius"),
        (pad::CHOKED_MS, "chokeBadge/chokedMs"),
        (pad::PEER_INSET, "peer/inset"),
        (pad::PEER_RADIUS, "peer/radius"),
        (pad::BANK_DOT, "bankPeerDot/size"),
        (pad::CHIP, "chip/size"),
        (pad::CHIP_RADIUS, "chip/radius"),
    ];
    for (v, k) in pairs {
        assert_eq!(v, m(k), "{k}");
    }
    assert_ne!(pad::GRID, m("sizes/focus"), "negative control: the lookup tells paths apart");
}
