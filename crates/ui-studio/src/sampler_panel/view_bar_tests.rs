//! Navigation remains UI state; geometry follows the full bar at both display scales.

use super::test_rig::{T, mid};
use serde_json::json;
use transit_model::TrackId;

fn pick(t: &mut T, tr: TrackId, index: usize) {
    let switch = t.panel(tr).bar.switch.unwrap();
    t.click(mid(switch.segments[index]), 1);
    assert_eq!(t.panel(tr).bar.switch.unwrap().selected, index);
}

#[test]
fn view_bar_is_on_every_view_and_switching_changes_no_routing_or_parameters() {
    let (mut t, tr) = T::kit();
    // A screen tall enough that the Slices view's window (bench, panels and sections) leaves the
    // device chain's editor buttons uncovered for the second instance below.
    t.screen = egui::vec2(1400.0, 1400.0);
    t.frames(3);
    t.click(t.pad_at(tr, 1), 1);
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    let bar = t.panel(tr).bar.rect;
    for index in [0, 2, 1] {
        pick(&mut t, tr, index);
        let panel = t.panel(tr);
        assert_eq!(panel.bar.rect, bar, "the same bar on every view");
        assert!(serde_json::to_value(t.app.engine.session()).unwrap() == before, "switching changes no document field");
        assert_eq!(t.undo_len(), undo);
        assert!(panel.footer[3] > panel.footer[1]);
        if index == 1 {
            assert_eq!(panel.selected, 1);
            assert_eq!(panel.pads.len(), 16);
            assert!(panel.placeholder.is_none());
        } else {
            if index == 2 {
                // The Slices view shows its bench (pass G2); the Instrument view keeps the placeholder.
                assert!(panel.placeholder.is_none() && panel.slices.is_some());
            } else {
                assert_eq!(panel.placeholder.unwrap().0, format!("{} view is not built yet.", crate::tokens::sampler::VIEW_BAR_VIEWS[index]));
            }
            assert!(panel.pads.is_empty() && panel.bar.load_kit.is_none());
            t.key("Delete");
            t.key("Backspace");
            assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before, "hidden pads do not take Delete or Backspace");
        }
    }
    pick(&mut t, tr, 2);
    t.key("Escape");
    assert!(t.app.builtin_editor.windows.is_empty());
    t.open_card(0);
    assert_eq!(t.panel(tr).bar.switch.unwrap().selected, 2, "reopening retains the instance's view");
    let second = t.sampler_track();
    t.app.run("mix.sampler_new_patch", json!({"track": second.0})).unwrap();
    t.frames(3);
    t.open_card(0);
    assert_eq!(t.panel(second).bar.switch.unwrap().selected, 1, "another instance starts on Pads");
    assert_eq!(t.panel(tr).bar.switch.unwrap().selected, 2);
}

#[test]
fn legacy_views_keep_the_sample_until_new_kit_is_chosen() {
    let mut t = T::new();
    let tr = t.sampler_track();
    let sample = t.sample("Legacy.wav", 64);
    t.app.run("mix.sampler_load", json!({"track": tr.0, "path": sample})).unwrap();
    t.open_card(0);
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    for index in [0, 2, 1] {
        pick(&mut t, tr, index);
        assert!(t.panel(tr).legacy.is_some());
        assert!(!t.panel(tr).bar.load_enabled);
        assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
        assert_eq!(t.undo_len(), undo);
    }
    pick(&mut t, tr, 2);
    t.click(mid(t.panel(tr).legacy.unwrap().new_kit), 1);
    assert_eq!(t.panel(tr).bar.switch.unwrap().selected, 1);
    assert_eq!(t.panel(tr).pads.len(), 16);
    assert_eq!(t.undo_len(), undo + 1);
}

pub(super) fn hundredths(x: f32) -> i32 {
    (x * 100.0).round() as i32
}

#[test]
fn full_bar_rects_match_the_readme_at_both_scales_and_widths() {
    for scale in [1.0, 1.5] {
        for width in [super::DESIGN_WIDTH, super::MIN_WIDTH] {
            let (mut t, tr, _) = super::width_tests::open_at(width, None);
            t.ctx.set_pixels_per_point(scale);
            t.frames(4);
            let p = t.panel(tr);
            let b = p.bar;
            let switch = b.switch.unwrap();
            let action = b.load_kit.unwrap();
            let expand_rect = b.expand.expect("Expand to rack… is shown");
            let (bar, track, slot, field, label, load, expand) = (
                b.rect.map(hundredths),
                switch.track.map(hundredths),
                b.summary_slot.map(hundredths),
                b.start.field.map(hundredths),
                b.start.label_rect.map(hundredths),
                action.map(hundredths),
                expand_rect.map(hundredths),
            );
            eprintln!(
                "scale {scale}, width {width}: bar {:?}, switch {:?}, segments {:?}, summary {:?}, slot {:?}, label {:?}, field {:?}, load {action:?}, expand {expand_rect:?}, footer {:?}",
                b.rect, switch.track, switch.segments, b.summary_rect, b.summary_slot, b.start.label_rect, b.start.field, p.footer
            );
            assert_eq!(bar[3] - bar[1], 2800);
            assert_eq!(track[0], bar[0]);
            assert_eq!(slot[0] - track[2], 1200);
            assert_eq!(label[0] - slot[2], 1200);
            assert_eq!(field[0] - label[2], 600);
            assert_eq!(load[0] - field[2], 1200);
            assert_eq!(expand[0] - load[2], 1200);
            assert_eq!(expand[2], bar[2]);
            assert_eq!(field[2] - field[0], 4800);
            for control in [track, field, load, expand] {
                assert_eq!(control[3] - control[1], 2400);
                assert_eq!(control[1] - bar[1], 200);
                assert_eq!(bar[3] - control[3], 200);
            }
            assert!(hundredths(b.summary_rect[2]) <= slot[2], "the complete summary fits");
            assert_eq!(hundredths(b.summary_rect[0]), slot[0]);
            assert_eq!(switch.selected, 1);
            assert_eq!(switch.inks, vec![crate::theme::TOKENS.text_faint, crate::theme::TOKENS.text, crate::theme::TOKENS.text_faint]);
        }
    }
}

#[test]
fn expand_to_rack_is_shown_unavailable_at_the_unused_control_alpha() {
    let (mut t, tr) = T::kit();
    let expand = t.panel(tr).bar.expand.expect("Expand to rack… is shown");
    assert_eq!(t.panel(tr).bar.opacity, [1.0, crate::tokens::sampler::UNUSED_CONTROL_ALPHA], "Load kit available, Expand to rack faded");
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    t.click(mid(expand), 1);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before, "it takes no input");
    assert_eq!(t.undo_len(), undo);
    assert!(t.app.dialog.is_none() && t.app.builtin_editor.devices.sampler.views.values().all(|v| v.kit_preview.is_none()));
    // A legacy patch's Load kit is unavailable the same way.
    let mut t = T::new();
    let tr = t.sampler_track();
    let sample = t.sample("Legacy.wav", 64);
    t.app.run("mix.sampler_load", json!({"track": tr.0, "path": sample})).unwrap();
    t.open_card(0);
    assert!(!t.panel(tr).bar.load_enabled);
    assert_eq!(t.panel(tr).bar.opacity, [crate::tokens::sampler::UNUSED_CONTROL_ALPHA; 2]);
}

#[test]
fn the_longest_summary_fits_at_the_minimum_width() {
    for scale in [1.0, 1.5] {
        let (mut t, tr, _) = super::width_tests::open_at(super::MIN_WIDTH, None);
        for pad in 1..=16u8 {
            if pad > 3 {
                let p = t.sample(&format!("Hit {pad}.wav"), 480);
                t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": pad, "pad": pad, "path": p.to_string_lossy()}));
            }
            t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": pad, "choke": pad}));
        }
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let b = t.panel(tr).bar;
        assert_eq!(b.summary, "Bank A · 16 of 16 loaded · 16 choke groups");
        assert_eq!(hundredths(b.summary_rect[0]), hundredths(b.summary_slot[0]));
        assert!(hundredths(b.summary_rect[2]) <= hundredths(b.summary_slot[2]), "{scale}: {:?} in {:?}", b.summary_rect, b.summary_slot);
        assert_eq!(hundredths(b.expand.unwrap()[2]), hundredths(b.rect[2]));
    }
}
