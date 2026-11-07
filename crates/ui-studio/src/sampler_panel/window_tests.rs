//! The Sampler's floating window: the shared device window rule (the A bar, the footer, the
//! content's width), the remembered position, Escape and Window > Close All Editors, both token
//! sets, and the user-only preset browser.

use super::test_rig::{T, mid};
use serde_json::json;

fn size(w: [f32; 4]) -> (f32, f32) {
    (w[2] - w[0], w[3] - w[1])
}

#[test]
fn the_window_is_the_device_window_with_the_shared_bar_and_a_footer() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let w = p.window;
    assert_eq!((p.title.bar[0], p.title.bar[1], p.title.bar[2]), (w[0] + 1.0, w[1] + 1.0, w[2] - 1.0));
    assert_eq!(p.title.device, "Sampler");
    assert_eq!(size(w).0, super::DESIGN_WIDTH, "the content's width on a screen wider than it");
    assert_eq!(p.footer[3], w[3] - 1.0);
    assert!(p.load.starts_with("Audio load "));
    assert!(t.window(tr).credit.is_empty(), "transit's own: no credit");
    // The instrument has no bypass: the bar ghosts it.
    assert!(!p.title.bypass_available);
    // The panels: the hero row beside each other, the sections row under them.
    let (pads, strip, play, amp) = (p.panel("pads").unwrap(), p.panel("strip").unwrap(), p.panel("playback").unwrap(), p.panel("amp").unwrap());
    assert_eq!(strip[0] - pads[2], crate::tokens::layout::PANEL_GAP);
    assert_eq!((pads[1], pads[3]), (strip[1], strip[3]), "one hero row");
    assert_eq!(play[1] - pads[3], crate::tokens::layout::PANEL_GAP);
    assert_eq!(amp[0] - play[2], crate::tokens::layout::PANEL_GAP);
    assert_eq!((play[0], amp[2]), (pads[0], strip[2]), "the rows share their edges");
    // The window holds its content exactly: the footer right under the sections row.
    assert_eq!(p.footer[1], play[3] + crate::tokens::layout::WINDOW_PADDING);
    // Negative control: a Macro Oscillator still opens its own panel, not this one.
    let mut m = T::new();
    let r = m.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let mt = transit_model::TrackId(r["tracks"][0].as_u64().unwrap());
    m.app.focus(mt);
    m.frames(4);
    m.open_card(0);
    let mw = m.window(mt);
    assert!(mw.sampler.is_none() && mw.macro_osc.is_some());
}

#[test]
fn the_position_is_remembered_and_escape_and_close_all_close_it() {
    let (mut t, tr) = T::open();
    let w = t.panel(tr).window;
    // Drag the bar 40 right and 30 down; the window follows and keeps its size.
    let bar = (w[0] + 60.0, w[1] + 26.0);
    t.drag(bar, (bar.0 + 40.0, bar.1 + 30.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0], moved[1]), (w[0] + 40.0, w[1] + 30.0));
    assert_eq!(size(moved), size(w));
    // Escape closes the focused editor.
    t.click(mid(t.panel(tr).footer), 1);
    t.key("Escape");
    assert!(t.app.builtin_editor.windows.is_empty(), "closed");
    // Reopened, it is where it was left.
    t.open_card(0);
    let back = t.panel(tr).window;
    assert_eq!((back[0], back[1]), (moved[0], moved[1]));
    // Window > Close All Editors.
    t.run("window.close_editors", json!({}));
    assert!(t.app.builtin_editor.windows.is_empty());
    // Negative control: a click elsewhere hands Escape back; the window stays.
    t.open_card(0);
    t.click((5.0, 890.0), 1);
    t.key("Escape");
    assert!(!t.app.builtin_editor.windows.is_empty());
}

#[test]
fn both_token_sets_draw_the_pads() {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let (mut t, tr) = T::kit();
        crate::theme::apply_pinned(&t.ctx, theme);
        t.frames(3);
        let tokens = if theme == egui::Theme::Light { &crate::theme::LIGHT } else { &crate::theme::DARK };
        let p = t.panel(tr);
        assert_eq!(p.pad(1).unwrap().pad.fill, tokens.surface_raised, "{theme:?}");
        assert_eq!(p.pad(0).unwrap().pad.border, tokens.signal, "the selected pad, {theme:?}");
        assert_eq!(p.strip.segments[0].ink, tokens.text, "{theme:?}");
    }
    assert_ne!(crate::theme::DARK.surface_raised, crate::theme::LIGHT.surface_raised, "negative control: the sets differ");
}

#[test]
fn the_preset_browser_lists_user_presets_only() {
    // ADR 0041: no sample kits or sample-dependent factory presets ship.
    assert!(crate::presets::factory_for("sampler").is_empty());
    let (t, tr) = T::open();
    let title = t.panel(tr).title;
    assert!(title.preset_group != "Factory", "{title:?}");
    // Negative control: the Compressor's browser has factory presets.
    assert!(!crate::presets::factory_for("compressor").is_empty());
}
