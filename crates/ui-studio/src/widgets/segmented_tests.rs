//! The segmented control: segments tile the inset track with 2 px padding, the selected one reads
//! `surface.selected` and `text`, the rest `text.faint`, and a click picks another segment.

use super::segmented;
use super::test_support::{Rig, inside, mid};
use crate::theme::DARK;

#[test]
fn segments_tile_the_track_inside_its_padding() {
    let mut rig = Rig::new();
    for width in [None, Some(150.0)] {
        let l = rig.idle(|ui| segmented::show(ui, egui::Id::new("f"), &["LP", "BP", "HP"], 1, width).layout);
        assert_eq!(l.track[3] - l.track[1], 24.0);
        if let Some(w) = width {
            assert_eq!(l.track[2] - l.track[0], w);
            let ws: Vec<f32> = l.segments.iter().map(|s| s[2] - s[0]).collect();
            assert!(ws.iter().all(|w| (w - ws[0]).abs() < 0.02), "even: {ws:?}");
        }
        assert_eq!(l.segments[0][0], l.track[0] + 2.0, "2 px padding");
        assert!((l.segments[2][2] - (l.track[2] - 2.0)).abs() < 0.02);
        for pair in l.segments.windows(2) {
            assert!((pair[0][2] - pair[1][0]).abs() < 0.02, "segments abut: {pair:?}");
        }
        for s in &l.segments {
            assert!(inside(*s, l.track));
        }
        assert_eq!(l.inks, vec![DARK.text_faint, DARK.text, DARK.text_faint]);
        assert_eq!(l.selected_fill, DARK.surface_selected);
    }
}

#[test]
fn a_click_picks_another_segment_and_the_selected_one_reports_nothing() {
    let mut rig = Rig::new();
    let opts = ["LP", "BP", "HP"];
    let l = rig.idle(|ui| segmented::show(ui, egui::Id::new("f"), &opts, 1, None).layout);
    assert_eq!(rig.click(mid(l.segments[2]), |ui| segmented::show(ui, egui::Id::new("f"), &opts, 1, None)).picked, Some(2));
    assert_eq!(rig.click(mid(l.segments[1]), |ui| segmented::show(ui, egui::Id::new("f"), &opts, 1, None)).picked, None);
}
