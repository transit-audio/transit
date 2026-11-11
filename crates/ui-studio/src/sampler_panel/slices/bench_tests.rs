//! The Slices view's bench through the panel's rig: what it shows comes from the state query, the
//! selection, zoom and scroll are UI state, and Space auditions the selected slice.

use super::super::test_rig::{T, mid};
use super::bench::{self, BenchLayout};
use crate::theme::TOKENS;
use crate::tokens::{SPACE, chrome, layout, sampler as tk};
use serde_json::json;
use transit_midi::note_name;
use transit_model::TrackId;

/// A kit with a one-second loop at 44.1 kHz (the session runs at 48 kHz), Manual markers at a
/// quarter, a half and three quarters, the four slices on pads A1 to A4, the Slices view showing.
fn sliced() -> (T, TrackId) {
    let (mut t, tr) = T::open();
    let path = t.sample_at("Loop.wav", 44_100, 44_100);
    t.run("mix.sampler_slice_loop", json!({"track": tr.0, "path": path.to_string_lossy()}));
    for frame in [11_025, 22_050, 33_075] {
        t.run("mix.sampler_slice_marker", json!({"track": tr.0, "op": "add", "frame": frame, "snap": false}));
    }
    t.run("mix.sampler_slices_to_pads", json!({"track": tr.0}));
    show_slices(&mut t, tr);
    (t, tr)
}

fn show_slices(t: &mut T, tr: TrackId) {
    let switch = t.panel(tr).bar.switch.unwrap();
    t.click(mid(switch.segments[2]), 1);
}

fn bench(t: &T, tr: TrackId) -> BenchLayout {
    t.panel(tr).slices.expect("the Slices view's bench")
}

fn doc(t: &T) -> serde_json::Value {
    serde_json::to_value(t.app.engine.session()).unwrap()
}

fn h(x: f32) -> i32 {
    (x * 100.0).round() as i32
}

#[test]
fn the_bench_shows_the_state_querys_slices_at_the_sources_own_rate() {
    let (mut t, tr) = sliced();
    let b = bench(&t, tr);
    assert!(t.panel(tr).placeholder.is_none());
    assert_eq!(b.count, "4 of 64 slices");
    assert_eq!(b.flags.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(), ["S 1", "2", "3", "4"]);
    assert_eq!(b.flags.iter().map(|f| f.frame).collect::<Vec<_>>(), [0, 11_025, 22_050, 33_075]);
    assert!(b.end.is_some());
    assert_eq!(b.hint, bench::HINT_MANUAL);
    let mc = t.app.ui.middle_c;
    assert_eq!(b.selection, format!("Slice 1 · 0.000–0.250 s · pad A1 · {}", note_name(36, mc)));
    // The time ruler counts seconds from S at 44.1 kHz: 0.0, 0.5 and 1.0 s at 1×.
    assert_eq!(b.ticks.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["0.0 s", "0.5 s"]);
    // The bar's summary: the file, its length and its native rate.
    assert_eq!(t.panel(tr).bar.summary, "Loop.wav · 1.00 s · 44.1 kHz");
    // The Middle C setting names the key through the one namer.
    t.app.ui.middle_c = transit_midi::MiddleC::C4;
    t.frames(2);
    assert!(bench(&t, tr).selection.ends_with(&note_name(36, transit_midi::MiddleC::C4)));
}

#[test]
fn a_click_on_a_cell_selects_the_slice_and_changes_no_document() {
    let (mut t, tr) = sliced();
    let before = doc(&t);
    let undo = t.undo_len();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 2).unwrap().1;
    t.click(mid(cell), 1);
    let b = bench(&t, tr);
    let mc = t.app.ui.middle_c;
    assert_eq!(b.selection, format!("Slice 3 · 0.500–0.750 s · pad A3 · {}", note_name(38, mc)));
    let tint = |i: usize| b.bands.iter().find(|x| x.0 == i).unwrap().2;
    assert_eq!(tint(2), Some(TOKENS.signal.gamma_multiply(tk::BENCH_SELECTED_ALPHA)));
    assert_eq!(tint(1), Some(TOKENS.text.gamma_multiply(tk::BENCH_ALT_SLICE_ALPHA)), "alternate slices read as cells");
    assert_eq!(tint(0), None);
    let flag = &b.flags[2];
    assert_eq!((flag.fill, flag.ink, flag.edge, flag.line_color), (TOKENS.signal, TOKENS.surface_inset, TOKENS.signal, TOKENS.signal));
    assert!(b.selected_columns > 0 && b.selected_columns < b.columns, "the selected slice's waveform in signal");
    assert_eq!(doc(&t), before, "selection is UI state");
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn zoom_and_scroll_keep_the_selected_slice_centered_and_change_no_document() {
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 1).unwrap().1;
    t.click(mid(cell), 1);
    let before = doc(&t);
    let undo = t.undo_len();
    let zoom = bench(&t, tr).zoom.unwrap();
    t.click(mid(zoom.segments[3]), 1);
    let b = bench(&t, tr);
    assert_eq!(b.zoom.as_ref().unwrap().selected, 3);
    assert_eq!(b.view.1 - b.view.0, 44_100.0 / 8.0);
    assert_eq!((b.view.0 + b.view.1) / 2.0, (11_025.0 + 22_050.0) / 2.0, "8× centers slice 2");
    let (track, thumb) = b.scroll.expect("past 1× the scroll bar shows");
    assert_eq!(h(track[3]), h(b.well[3] - chrome::FIELD_EDGE_WIDTH - chrome::SCROLL_INSET), "inside the well's foot");
    assert_eq!(h(thumb[3] - thumb[1]), h(chrome::SCROLL_REST));
    let start = b.view.0;
    t.drag(mid(thumb), (mid(thumb).0 + 100.0, mid(thumb).1), false);
    let b = bench(&t, tr);
    assert!(b.view.0 > start, "the drag scrolls: {start} to {}", b.view.0);
    assert_eq!(b.flags.iter().map(|f| f.frame).collect::<Vec<_>>(), bench_frames(&t, tr), "stored positions never change");
    assert_eq!(doc(&t), before, "zoom and scroll are UI state");
    assert_eq!(t.undo_len(), undo);
    t.click(mid(b.zoom.unwrap().segments[0]), 1);
    assert!(bench(&t, tr).scroll.is_none(), "1× has no scroll bar");
}

/// The visible markers' frames as the state reports them.
fn bench_frames(t: &T, tr: TrackId) -> Vec<u64> {
    let b = bench(t, tr);
    let all = [0u64, 11_025, 22_050, 33_075];
    all.into_iter().filter(|f| (*f as f64) >= b.view.0 - 1.0 && (*f as f64) <= b.view.1).filter(|f| b.flags.iter().any(|x| x.frame == *f)).collect()
}

fn key_event(pressed: bool) -> egui::Event {
    egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
}

#[test]
fn space_auditions_the_selected_slice_while_held_and_does_not_play_the_song() {
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 3).unwrap().1;
    t.click(mid(cell), 1);
    let before = doc(&t);
    t.input(vec![key_event(true)]);
    let v = t.app.builtin_editor.devices.sampler.views.values().next().unwrap().clone();
    let velocity = t.app.ui.keys.velocity.clamp(1, 127);
    assert_eq!(v.last_audition, Some((39, velocity)), "slice 4's pad note at the piano roll's velocity");
    assert_eq!(v.held, Some(39));
    assert!(!t.app.is_playing(), "Space auditions instead of playing");
    t.input(vec![key_event(false)]);
    assert_eq!(t.app.builtin_editor.devices.sampler.views.values().next().unwrap().held, None, "the release ends it");
    assert_eq!(doc(&t), before);
    let rows = crate::shortcuts::filtered("Audition slice");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, crate::shortcuts::Scope::Sampler);
    assert_eq!(rows[0].1.len(), 2, "Space and Enter");
}

#[test]
fn auto_markers_are_quiet_manual_markers_have_handles_and_snap_is_unavailable_in_auto() {
    let (mut t, tr) = sliced();
    let b = bench(&t, tr);
    let snap = b.snap.as_ref().unwrap();
    assert!(snap.available && snap.on, "Manual: snap on by default and available");
    let m = &b.flags[1];
    assert_eq!((m.fill, m.ink, m.edge, m.line_color), (TOKENS.surface_selected, TOKENS.text, TOKENS.line_strong, TOKENS.text_muted));
    assert!(b.flags[0].handle.is_none(), "S has no handle");
    assert!(b.flags[1..].iter().all(|f| f.handle.is_some()));
    t.run("mix.sampler_slice_mode", json!({"track": tr.0, "mode": "auto"}));
    let b = bench(&t, tr);
    assert_eq!(b.mode.as_ref().unwrap().selected, 0);
    assert_eq!(b.hint, bench::HINT_AUTO);
    assert!(!b.snap.as_ref().unwrap().available, "SNAP shapes nothing in Auto");
    assert_eq!(b.count, "1 of 64 slices");
    assert!(b.flags.iter().all(|f| f.handle.is_none()), "Auto markers are read-only");
    let e = b.end.unwrap();
    assert_eq!((e.fill, e.ink), (TOKENS.surface_selected, TOKENS.text_muted));
    assert_eq!(h(e.line[2] - e.line[0]), h(tk::BENCH_END_WIDTH));
    assert_eq!(e.flag[2], e.line[0], "the E flag stands against its line");
}

#[test]
fn every_bench_color_name_is_a_token() {
    for name in tk::BENCH_AUTO_FLAG
        .iter()
        .chain(&tk::BENCH_MANUAL_FLAG)
        .chain(&tk::BENCH_SELECTED_FLAG)
        .chain(&tk::BENCH_END_FLAG)
        .chain(&[tk::BENCH_SELECTED_LINE, tk::BENCH_HANDLE_LINE, tk::BENCH_HANDLE_FILL, tk::BENCH_MIDLINE, tk::TAG_INK, tk::TAG_EDGE])
        .chain(&[tk::BENCH_END, tk::BENCH_AUTO_LINE, tk::BENCH_MANUAL_LINE, tk::BENCH_WAVE, tk::BENCH_FLAG_STRIP_LINE])
    {
        assert_ne!(bench::named(name), egui::Color32::MAGENTA, "{name}");
    }
}

#[test]
fn the_state_is_read_once_per_document_change() {
    let (mut t, tr) = sliced();
    let reads = |t: &T| t.app.builtin_editor.devices.sampler.views.values().next().unwrap().slices.snapshot.reads;
    let r0 = reads(&t);
    t.frames(30);
    assert_eq!(reads(&t), r0, "idle frames read nothing");
    t.run("mix.sampler_slice_mode", json!({"track": tr.0, "mode": "auto"}));
    assert_eq!(reads(&t), r0 + 1, "one read for one change");
}

#[test]
fn bench_rects_match_the_readme_and_board_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = sliced();
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let p = t.panel(tr);
        let b = p.slices.clone().unwrap();
        eprintln!(
            "scale {scale}: panel {:?} well {:?} strip {:?} ruler {:?} hint {:?} flags {:?}",
            b.panel,
            b.well,
            b.strip,
            b.ruler,
            b.hint_rect,
            b.flags.iter().map(|f| (f.flag, f.handle)).collect::<Vec<_>>()
        );
        let (pad_v, pad_h) = (layout::PANEL_PADDING[0], layout::PANEL_PADDING[1]);
        // The panel under the view bar, one panel gap down, across the body.
        assert_eq!(h(b.panel[1] - p.bar.rect[3]), h(layout::PANEL_GAP));
        assert_eq!((h(b.panel[0]), h(b.panel[2])), (h(p.bar.rect[0]), h(p.bar.rect[2])));
        // The header: 24 px controls centered on one row, items 12 apart.
        let mode = b.mode.clone().unwrap().track;
        let zoom = b.zoom.clone().unwrap().track;
        let snap = b.snap.clone().unwrap().rect;
        let top = b.panel[1] + pad_v;
        for c in [mode, zoom, snap] {
            assert_eq!((h(c[1]), h(c[3])), (h(top), h(top + layout::DENSE_CONTROL_HEIGHT)), "{c:?}");
        }
        assert_eq!(h(b.label[0]), h(b.panel[0] + pad_h));
        assert_eq!(h(mode[0] - b.label[2]), h(SPACE[2]));
        assert_eq!(h(b.count_rect[0] - mode[2]), h(SPACE[2]));
        assert_eq!(h(snap[2]), h(b.panel[2] - pad_h));
        assert_eq!(h(snap[0] - zoom[2]), h(SPACE[2]));
        assert_eq!(h(zoom[0] - b.zoom_label[2]), h(SPACE[2]));
        // The well: 8 under the header, 188 high, across the panel's inside.
        assert_eq!(h(b.well[1] - (top + layout::DENSE_CONTROL_HEIGHT)), h(SPACE[1]));
        assert_eq!(h(b.well[3] - b.well[1]), h(tk::BENCH_WELL_HEIGHT));
        assert_eq!((h(b.well[0]), h(b.well[2])), (h(b.panel[0] + pad_h), h(b.panel[2] - pad_h)));
        // The 18 px flag strip inside the well's edge; flags 16 high, 1 down, at least 18 wide,
        // each starting on its line.
        assert_eq!((h(b.strip[1] - b.well[1]), h(b.strip[3] - b.strip[1])), (h(chrome::FIELD_EDGE_WIDTH), h(tk::BENCH_FLAG_STRIP)));
        for f in &b.flags {
            assert_eq!(h(f.flag[1] - b.strip[1]), h(tk::BENCH_FLAG_INSET));
            assert_eq!(h(f.flag[3] - f.flag[1]), h(tk::BENCH_FLAG_HEIGHT));
            assert!(h(f.flag[2] - f.flag[0]) >= h(tk::BENCH_FLAG_MIN_WIDTH));
            assert_eq!(h(f.flag[0]), h(f.line[0]));
            assert_eq!(h(f.line[2] - f.line[0]), 100);
            if let Some(hd) = f.handle {
                assert_eq!((h(hd[2] - hd[0]), h(hd[3] - hd[1])), (h(tk::BENCH_HANDLE), h(tk::BENCH_HANDLE)));
                assert_eq!(h(b.well[3] - chrome::FIELD_EDGE_WIDTH - hd[3]), h(tk::BENCH_HANDLE_INSET));
            }
        }
        // The end line at the loop's end, the well's inside right edge at 1×.
        assert_eq!(h(b.end.as_ref().unwrap().line[2]), h(b.well[2] - chrome::FIELD_EDGE_WIDTH));
        // The ruler 8 under the well, 12 high; the hint row 8 under it; the panel's padding last.
        assert_eq!((h(b.ruler[1] - b.well[3]), h(b.ruler[3] - b.ruler[1])), (h(SPACE[1]), h(tk::BENCH_RULER_HEIGHT)));
        assert_eq!(h(b.ruler[0]), h(b.well[0]));
        assert!(h(b.hint_rect[1]) >= h(b.ruler[3] + SPACE[1]));
        assert_eq!(h(b.selection_rect[2]), h(b.panel[2] - pad_h), "the selection line right-aligned");
        assert_eq!(h(b.panel[3] - b.panel[1]), h(bench::height(&super::super::measurer(&t.ctx))));
    }
}
