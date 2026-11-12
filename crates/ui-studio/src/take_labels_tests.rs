//! #317: label rules use the existing recording fixtures; painted text comes from egui shapes.

use super::*;
use crate::takes::labels::{LaneLabel, lane_label};
use transit_model::{StackTake, TakeStack};
use transit_time::Range;

fn label(t: &T, track: TrackId, number: u32) -> LaneLabel {
    let tr = t.app.engine.session().track(track).unwrap();
    lane_label(tr, &tr.take_stacks(), number, |at| crate::transport::bars_beats(&t.app, at))
}

fn six_laps(t: &mut T, dir: &Dir, partial: bool) -> TrackId {
    let track = t.track(json!({"kind": "audio", "name": "Audio 1"}));
    let path = dir.0.join("six.wav");
    // 120 BPM, 48 kHz: one bar is 96000 samples; the last third-bar ends at 1.2.
    write_wav(&path, 5 * 96_000 + if partial { 32_000 } else { 96_000 });
    let plan = transit_engine::io::Recording {
        start: 0,
        pass: Some(96_000),
        punch: None,
        rate: 48_000,
        takes: Vec::new(),
        midi_journal: None,
        midi_placed: false,
        cycle: None,
    };
    t.app.place_takes(&plan, vec![(track, path)], 96_000, 1);
    t.app.run("view.zoom_set", json!({"samples_per_px": 200.0})).unwrap();
    t.frames(3);
    track
}

#[test]
fn a_single_take_covering_its_stack_keeps_the_plain_playing_label() {
    let dir = Dir::new("label-single");
    let mut t = T::new();
    let track = t.track(json!({"kind": "audio"}));
    t.record(&dir, track, 0, 2.0, 1);
    let tr = t.app.engine.session().track(track).unwrap();
    let c = &tr.clips()[0];
    // The model only discovers stacks with >=2 takes. Exercise the one-take rule explicitly.
    let stack = TakeStack {
        range: c.range(),
        takes: vec![StackTake { number: 1, clips: vec![(tr.active_playlist, c.id)], active_len: c.length, explicit: true }],
    };
    let l = lane_label(tr, &[stack], 1, |at| crate::transport::bars_beats(&t.app, at));
    assert_eq!(l.text, "Take 1 · playing");
    assert_eq!(l.hover, "Take 1 plays:\n1.1–2.1\nClick to play take 1 wherever it was recorded");
}

#[test]
fn six_full_laps_label_only_take_six_as_playing() {
    let dir = Dir::new("label-full");
    let mut t = T::new();
    let track = six_laps(&mut t, &dir, false);
    assert_eq!(label(&t, track, 6).text, "Take 6 · playing");
    for n in 1..6 {
        assert_eq!(label(&t, track, n).text, format!("Take {n}"));
    }
    assert_eq!(label(&t, track, 6).hover, "Take 6 plays:\n1.1–2.1\nClick to play take 6 wherever it was recorded");
}

#[test]
fn a_partial_sixth_lap_names_where_six_stops_and_five_resumes() {
    let dir = Dir::new("label-partial");
    let mut t = T::new();
    let track = six_laps(&mut t, &dir, true);
    assert_eq!(label(&t, track, 6).text, "Take 6 · playing to 1.2");
    assert_eq!(label(&t, track, 5).text, "Take 5 · playing from 1.2");
    let text = painted_text(&mut t);
    assert!(text.iter().any(|(s, _)| s == "Take 6 · playing to 1.2"));
    assert!(text.iter().any(|(s, _)| s == "Take 5 · playing from 1.2"));
}

#[test]
fn an_inner_take_names_its_range_and_the_surrounding_take_lists_both_pieces() {
    let dir = Dir::new("label-inner");
    let mut t = T::new();
    let track = two_takes(&mut t, &dir);
    t.record(&dir, track, 24_000, 1.0, 3);
    assert_eq!(label(&t, track, 3).text, "Take 3 · playing 1.2–1.4");
    assert_eq!(label(&t, track, 2).text, "Take 2 · playing (2 ranges)");
    assert_eq!(label(&t, track, 2).hover, "Take 2 plays:\n1.1–1.2\n1.4–3.1\nClick to play take 2 wherever it was recorded");
}

#[test]
fn separate_stacks_count_separately_even_when_each_is_fully_played() {
    let dir = Dir::new("label-stacks");
    let mut t = T::new();
    let track = two_takes(&mut t, &dir);
    t.record(&dir, track, 240_000, 2.0, 3);
    t.record(&dir, track, 240_000, 2.0, 4);
    assert_eq!(label(&t, track, 2).text, "Take 2 · playing (2 ranges)");
    assert_eq!(label(&t, track, 2).hover, "Take 2 plays:\n1.1–3.1\n3.3–4.3\nClick to play take 2 wherever it was recorded");
}

#[test]
fn touching_and_overlapping_pieces_union_without_counting_inactive_material() {
    let dir = Dir::new("label-union");
    let mut t = T::new();
    let track = two_takes(&mut t, &dir);
    let mut tr = t.app.engine.session().track(track).unwrap().clone();
    let mut stack = tr.take_stacks().remove(0);
    let mut piece = tr.clips()[0].clone();
    let original = piece.id;
    piece.id = ClipId(10_000);
    piece.start = 48_000;
    piece.length = 144_000;
    let active = tr.active_playlist;
    tr.playlists[active].clips[0].length = 48_000;
    tr.playlists[active].clips.push(piece.clone());
    let take = stack.takes.iter_mut().find(|k| k.number == 2).unwrap();
    take.clips.push((active, piece.id));
    // Clip bounds must be clipped to the stack, not the other way around.
    stack.range = Range::new(24_000, 96_000);
    let l = lane_label(&tr, std::slice::from_ref(&stack), 2, |at| crate::transport::bars_beats(&t.app, at));
    assert_eq!(l.text, "Take 2 · playing");
    assert_eq!(l.hover, "Take 2 plays:\n1.2–2.1\nClick to play take 2 wherever it was recorded");
    tr.playlists[active].clips.iter_mut().find(|c| c.id == original).unwrap().length = 72_000;
    let l = lane_label(&tr, &[stack], 2, |at| crate::transport::bars_beats(&t.app, at));
    assert_eq!(l.text, "Take 2 · playing");
}

fn painted_text(t: &mut T) -> Vec<(String, egui::Rect)> {
    fn collect(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.job.text.clone(), clip)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, clip, out)),
            _ => {}
        }
    }
    t.time += 1.0 / 60.0;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
        time: Some(t.time),
        ..Default::default()
    };
    let out = t.ctx.run_ui(raw, |ui| t.app.ui(ui));
    let mut text = Vec::new();
    for shape in out.shapes {
        collect(&shape.shape, shape.clip_rect, &mut text);
    }
    text
}

#[test]
fn stacked_clip_paints_its_take_number_plain_clip_and_stored_names_stay_plain() {
    let dir = Dir::new("label-clips");
    let mut t = T::new();
    let track = six_laps(&mut t, &dir, false);
    let plain_track = t.track(json!({"kind": "audio", "name": "Plain"}));
    t.record(&dir, plain_track, 0, 2.0, 10);
    let session = t.app.engine.session_arc();
    let stacked = session.track(track).unwrap().clips()[0].clone();
    let plain = session.track(plain_track).unwrap().clips()[0].clone();
    let text = painted_text(&mut t);
    let expected = format!("{} · take 6", stacked.name);
    let (_, clip_rect) = text.iter().find(|(s, _)| s == &expected).expect("the arrangement paints the take suffix");
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == stacked.id.0).unwrap().rect;
    assert!(clip_rect.min.x >= r[0] && clip_rect.max.x <= r[2], "the name remains clipped to its strip");
    assert!(text.iter().any(|(s, _)| s == &plain.name), "a clip outside stacks keeps its plain name");
    assert!(text.iter().any(|(s, _)| s == &stacked.name), "lane clips keep the plain name");
    assert_eq!(t.app.engine.session().track(track).unwrap().clips()[0].name, stacked.name);
    assert_eq!(t.app.engine.session().track(plain_track).unwrap().clips()[0].name, plain.name);
}
