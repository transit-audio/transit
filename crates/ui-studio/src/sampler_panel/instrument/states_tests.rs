//! The Instrument view's states through the panel's rig (README, Sampler views, States): empty
//! with its drop target, Load… and a drop, a refused file, loading, missing media, the legacy
//! one-sample patch, and over capacity (every zone in use, a ninth alternative).

use super::super::slices::bench;
use super::super::test_rig::{T, mid};
use super::key_map;
use super::rig::*;
use crate::tokens::{SPACE, layout as space, sampler as tk};
use serde_json::json;
use transit_model::TrackId;

/// A new kit with no zone, the Instrument view showing.
fn empty() -> (T, TrackId) {
    let (mut t, tr) = T::open();
    show_instrument(&mut t, tr);
    (t, tr)
}

/// The engine's own words for a refused command (the note row's text).
fn refusal(t: &mut T, id: &str, p: serde_json::Value) -> String {
    let e = t.app.engine.execute(id, &p).expect_err("the engine refuses");
    super::super::slices::edit::words(&e)
}

#[test]
fn states_render_empty_with_the_drop_target_and_the_cards_ghosted() {
    let (t, tr) = empty();
    let l = layout(&t, tr);
    let (well, load) = l.key_map.empty.expect("the drop target");
    assert_eq!(well, l.key_map.well, "the key map's well is the drop target");
    assert!(load[0] > well[0] && load[2] < well[2] && load[1] > well[1] && load[3] < well[3], "Load… inside it");
    assert!(l.key_map.zones.is_empty() && l.key_map.octave_lines.is_empty(), "no zone, no grid lines");
    assert!(!l.key_map.keys.is_empty(), "the keyboard stays");
    assert!(l.zone.is_none() && l.group_panel.is_none());
    assert_eq!(l.cards_opacity, tk::UNUSED_CONTROL_ALPHA, "the cards ghosted");
    // The sections at 40 percent: their knobs take no input.
    assert!(!t.panel(tr).knobs.is_empty());
    assert!(t.panel(tr).knobs.iter().all(|k| k.knob.state == crate::widgets::knob::State::Disabled), "sections inert with no zone");
}

#[test]
fn the_drop_target_rects_are_exact_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = empty();
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let l = layout(&t, tr);
        let (well, load) = l.key_map.empty.unwrap();
        let (cx, cy) = mid(well);
        // Load… at the dense height, centered on the well's middle; the words then the button a
        // `space` 12 apart, centered as one across the well.
        let p = egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
        let words = crate::widgets::role_size(&p, &crate::tokens::LABEL_CONTROL, key_map::DROP).x;
        let bw = bench::button_w(&p, bench::LOAD);
        let x = cx - (words + SPACE[2] + bw) / 2.0;
        let want = [x + words + SPACE[2], cy - space::DENSE_CONTROL_HEIGHT / 2.0, x + words + SPACE[2] + bw, cy + space::DENSE_CONTROL_HEIGHT / 2.0];
        assert_eq!(load.map(h), want.map(h), "scale {scale}");
        eprintln!("scale {scale}: well {well:?} load {load:?}");
    }
}

#[test]
fn load_on_the_empty_view_loads_a_sample_into_zone_1_in_one_step() {
    let (mut t, tr) = empty();
    let path = t.sample("First.wav", 4_800);
    let p = path.to_string_lossy().to_string();
    t.app.services.pick_open = Some(Box::new(move |purpose, _| {
        assert_eq!(purpose, "mix.sampler_zone_source");
        Some(p.clone())
    }));
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let load = layout(&t, tr).key_map.empty.unwrap().1;
    t.click(mid(load), 1);
    assert_eq!(t.undo_len(), undo + 1);
    let s = sent(&t, log);
    assert!(s.len() == 1 && s[0].starts_with("mix.sampler_zone_source {\"path\":") && s[0].ends_with(",\"zone\":1}"), "{s:?}");
    assert!(t.patch(tr).zone(0).is_some(), "zone 1 holds the sample");
    assert_eq!(view(&t).selected, Some(1));
    let l = layout(&t, tr);
    assert!(l.key_map.empty.is_none() && l.key_map.zone(1).is_some() && l.zone.is_some());
    assert_eq!(l.cards_opacity, 1.0);
}

#[test]
fn a_file_dropped_on_the_empty_view_loads_and_a_refused_one_is_the_note() {
    let (mut t, tr) = empty();
    let well = layout(&t, tr).key_map.well;
    // Refused file: the engine's words in the note row, nothing loaded.
    let bad = t.dir.join("not audio.wav");
    std::fs::write(&bad, b"not a wav").unwrap();
    let undo = t.undo_len();
    t.drop_file(&bad, mid(well));
    let note = layout(&t, tr).note.expect("the refusal in the note row").0;
    assert!(!note.is_empty());
    assert!(t.patch(tr).zones.is_empty() && t.undo_len() == undo, "nothing loaded");
    assert!(layout(&t, tr).key_map.empty.is_some());
    // A sample dropped anywhere on the view (here on the ghosted Zone card) loads as zone 1.
    let good = t.sample("Dropped.wav", 4_800);
    t.drop_file(&good, mid(layout(&t, tr).group));
    assert!(t.patch(tr).zone(0).is_some(), "zone 1 holds the dropped sample");
    assert_eq!(t.undo_len(), undo + 1);
    assert!(layout(&t, tr).note.is_none(), "the next edit that lands clears the note");
}

#[test]
fn loading_reads_analyzing_in_the_corner_and_stays_editable() {
    let (mut t, tr) = layered();
    let st = view(&t).snapshot.data.unwrap();
    let id = st.span(st.zone(1).unwrap()).unwrap().source.unwrap();
    let audio = t.app.engine.session().pool.get(id).unwrap().clone();
    let bare = transit_model::SourceAudio { buffer: audio.buffer.clone(), peaks: Vec::new(), disk: None };
    t.app.engine.session_mut().pool.insert(id, std::sync::Arc::new(bare));
    t.frames(3);
    let w = layout(&t, tr).zone.unwrap().wave;
    let (text, tag) = w.tag.clone().expect("the status tag");
    assert_eq!(text, bench::ANALYZING);
    assert_eq!(w.columns, 0, "no waveform yet");
    assert!(w.missing.is_none());
    // The tag sits in the well's corner under the brace row; the region readout moves under it.
    let area = [w.well[0] + 1.0, w.well[1] + 1.0, w.well[2] - 1.0, w.well[3] - 1.0];
    assert_eq!((h(tag[2]), h(tag[1])), (h(area[2] - tk::TAG_INSET), h(area[1] + tk::WAVE_LOOP_BRACE + tk::TAG_INSET)));
    assert!(w.readout.1[1] >= tag[3] + SPACE[0] - 0.01, "the readout {:?} under the tag {tag:?}", w.readout.1);
    // Everything stays editable: a nudge lands.
    t.send("ui.key", json!({"key": egui::Key::ArrowRight.name(), "alt": true}));
    assert_eq!(t.patch(tr).zone(0).unwrap().start, 1);
}

#[test]
fn missing_media_hatches_the_well_names_the_file_and_keeps_ids_and_places() {
    let (mut t, tr) = grouped();
    let before = t.patch(tr);
    let st = view(&t).snapshot.data.unwrap();
    let id = st.span(st.zone(1).unwrap()).unwrap().source.unwrap();
    t.app.engine.session_mut().pool.remove(id);
    t.frames(3);
    let l = layout(&t, tr);
    let w = l.zone.as_ref().unwrap().wave.clone();
    let (card, text, relink) = w.missing.clone().expect("the missing-media card");
    assert_eq!(text, "G1.wav is missing");
    assert!(w.tag.is_none(), "never Analyzing… for a missing source");
    assert!(relink[0] >= card[0] && relink[2] <= card[2], "Relink… on the card");
    // The card centered on the well under its brace row.
    let area = [w.well[0] + 1.0, w.well[1] + 1.0, w.well[2] - 1.0, w.well[3] - 1.0];
    let body_mid = ((area[0] + area[2]) / 2.0, (area[1] + tk::WAVE_LOOP_BRACE + area[3]) / 2.0);
    // The recorded rect keeps hundredths (`r4`), so its middle is within one of the body's.
    assert!((mid(card).0 - body_mid.0).abs() <= 0.01 && (mid(card).1 - body_mid.1).abs() <= 0.01, "{card:?} centered on {body_mid:?}");
    // The zone, its id and its round-robin place are kept: the map and the Group panel read them.
    assert!(l.key_map.zone(1).is_some());
    assert_eq!(l.group_panel.unwrap().layers.iter().map(|r| r.chips.len()).sum::<usize>(), 3);
    assert_eq!(t.patch(tr).zones, before.zones, "nothing rewritten");
}

#[test]
fn a_legacy_patch_shows_new_kit_on_the_instrument_view() {
    let mut t = T::new();
    let tr = t.sampler_track();
    t.open_card(0);
    let switch = t.panel(tr).bar.switch.unwrap();
    t.click(mid(switch.segments[0]), 1);
    let p = t.panel(tr);
    assert!(p.instrument.is_none(), "no Instrument view on the one-sample path");
    let legacy = p.legacy.expect("the legacy body with its New Kit notice");
    assert!(legacy.new_kit != [0.0; 4]);
    assert!(t.app.engine.session().track(tr).unwrap().instrument.as_ref().unwrap().sampler_patch.is_none(), "never converted on open");
}

/// Layer 0 of `grouped()` filled to `count` alternatives (zones 5 on, with zone 1's source).
fn fill_layer(t: &mut T, tr: TrackId, count: u8) {
    let sid = t.app.engine.session().sources.iter().find(|s| s.name.starts_with("G1")).map(|s| s.id.0).expect("G1's source");
    for (i, alt) in (2..count).enumerate() {
        let p = json!({"track": tr.0, "zone": 5 + i, "source": sid, "playback": {"keys": [48, 52], "velocities": [1, 63], "root": 50,
            "membership": {"group": 0, "layer": 0, "alternative": alt}}});
        t.app.engine.execute("mix.sampler_zone_source", &p).unwrap();
    }
    t.frames(3);
}

#[test]
fn a_ninth_alternative_is_refused_in_the_engines_words_and_leaves_nothing_behind() {
    let (mut t, tr) = grouped();
    fill_layer(&mut t, tr, 8);
    let path = t.sample("Ninth.wav", 4_800);
    let p = path.to_string_lossy().to_string();
    let p2 = p.clone();
    t.app.services.pick_open = Some(Box::new(move |_, _| Some(p2.clone())));
    let (undo, sources, pooled) = (t.undo_len(), t.app.engine.session().sources.len(), t.app.engine.session().pool.len());
    let want = refusal(
        &mut t,
        "mix.sampler_zone_source",
        json!({"track": tr.0, "zone": 11, "path": p, "playback": {"keys": [48, 52], "velocities": [1, 63], "root": 50,
            "membership": {"group": 0, "layer": 0, "alternative": 8}}}),
    );
    // Eight chips do not fit one line of the card: they wrap, and the add chip stays inside it.
    let g = layout(&t, tr).group_panel.unwrap();
    let (row, add) = (g.layers[1].row, g.layers[1].add);
    assert_eq!(g.layers[1].chips.len(), 8);
    assert!(add[2] <= g.panel[2] && add[3] <= row[3], "the add chip {add:?} inside the card {:?} and its row {row:?}", g.panel);
    assert!(g.layers[1].chips.iter().all(|c| c.rect[2] <= g.panel[2]), "every chip inside the card");
    assert!(row[3] - row[1] > tk::LAYER_HEIGHT, "the row took a second line");
    assert_eq!(h(g.divider[1]), h(row[3] + tk::GROUP_ROW_GAP), "the divider under the taller row");
    t.click(mid(add), 1);
    assert_eq!(layout(&t, tr).note.expect("the refusal").0, want);
    assert!(want.contains("ninth alternative"), "{want}");
    assert_eq!(t.undo_len(), undo, "no step");
    let s = t.app.engine.session();
    assert_eq!((s.sources.len(), s.pool.len()), (sources, pooled), "the refused file left no source behind");
}

#[test]
fn with_every_zone_in_use_the_add_is_refused_in_the_engines_words() {
    let (mut t, tr) = grouped();
    let sid = t.app.engine.session().sources.iter().find(|s| s.name.starts_with("G1")).map(|s| s.id.0).expect("G1's source");
    // Zones 5 to 256, one key each (two to a key at most), no group.
    for n in 5..=transit_model::sampler_patch::ZONES as u64 {
        let k = (n % 100) + 10;
        let p = json!({"track": tr.0, "zone": n, "source": sid, "playback": {"keys": [k, k]}});
        t.app.engine.execute("mix.sampler_zone_source", &p).unwrap();
    }
    t.frames(3);
    assert_eq!(t.patch(tr).zones.len(), transit_model::sampler_patch::ZONES);
    let path = t.sample("More.wav", 4_800);
    let p = path.to_string_lossy().to_string();
    let p2 = p.clone();
    t.app.services.pick_open = Some(Box::new(move |_, _| Some(p2.clone())));
    let want = refusal(&mut t, "mix.sampler_zone_source", json!({"track": tr.0, "zone": 257, "path": p}));
    let undo = t.undo_len();
    let add = layout(&t, tr).group_panel.unwrap().layers[1].add;
    t.click(mid(add), 1);
    assert_eq!(layout(&t, tr).note.expect("the refusal").0, want);
    assert_eq!(t.undo_len(), undo);
}
