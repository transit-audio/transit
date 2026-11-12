//! Bounce in Place of the selected clips and the `range` of `track.bounce`; deterministic, no device.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_engine::{Engine, edit, io};
use transit_model::{ClipContent, ClipId, TrackId};

const SPANS: [(i64, i64); 3] = [(10_000, 20_000), (30_000, 40_000), (300_000, 310_000)];

fn track(e: &mut Engine, name: &str, level: f32, spans: &[(i64, i64)]) -> (TrackId, Vec<ClipId>) {
    let t = TrackId(e.execute("track.new", &json!({"kind": "audio", "format": "mono", "name": name})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    let mut ids = Vec::new();
    for (a, b) in spans {
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: vec![vec![level; (b - a) as usize]] };
        let src = io::add_source(s, name, buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        edit::place_clip(s, t, transit_model::Clip::audio(id, name, src, 0, *a, b - a));
        ids.push(id);
    }
    (t, ids)
}

fn ids(v: &[ClipId]) -> Vec<u64> {
    v.iter().map(|c| c.0).collect()
}

fn print_of(e: &Engine, t: TrackId, i: usize) -> (i64, Vec<f32>) {
    let c = &e.session().track(t).unwrap().clips()[i];
    let ClipContent::Audio { source, .. } = c.content else { panic!("an audio clip") };
    (c.start, e.session().pool.get(source).unwrap().buffer.channels[0].clone())
}

fn dest(r: &serde_json::Value, i: usize) -> TrackId {
    TrackId(r["tracks"][i].as_u64().unwrap())
}

#[test]
fn a_middle_clip_prints_its_span_plus_the_tail_at_its_start() {
    let mut e = Engine::default();
    let (t, c) = track(&mut e, "Keys", 0.25, &SPANS);
    let r = e.execute("clip.bounce", &json!({"clips": ids(&c[1..2])})).unwrap();
    let d = dest(&r, 0);
    let tr = e.session().track(d).unwrap();
    assert_eq!(tr.clips().len(), 1);
    let (start, print) = print_of(&e, d, 0);
    assert_eq!((start, tr.clips()[0].length), (30_000, 10_000 + 96_000));
    assert_eq!(tr.name, "Keys bounce");
    assert_eq!(print.len(), 106_000);
    let want: Vec<f32> = (0..106_000).map(|i| if i < 10_000 { 0.25 } else { 0.0 }).collect();
    let first = print.iter().zip(&want).position(|(g, w)| g.to_bits() != w.to_bits());
    assert_eq!(first.map(|i| (i, print[i], want[i])), None);
    // The source keeps every clip and is muted as the default says.
    let src = e.session().track(t).unwrap();
    assert_eq!(src.clips().len(), 3);
    assert!(src.mixer.mute);
}

#[test]
fn two_clips_on_one_track_become_one_print_covering_both() {
    let mut e = Engine::default();
    let (_, c) = track(&mut e, "Keys", 0.25, &SPANS);
    let r = e.execute("clip.bounce", &json!({"clips": [c[0].0, c[2].0]})).unwrap();
    assert_eq!(r["tracks"].as_array().unwrap().len(), 1);
    let d = dest(&r, 0);
    let (start, print) = print_of(&e, d, 0);
    assert_eq!((e.session().track(d).unwrap().clips().len(), start, print.len()), (1, 10_000, 300_000 + 96_000));
    // The gap between the clips prints as silence, the clips as signal.
    assert_eq!((print[0], print[15_000], print[295_000], print[305_000]), (0.25, 0.0, 0.25, 0.0));
}

#[test]
fn clips_on_two_tracks_print_twice_in_one_undo_step() {
    let mut e = Engine::default();
    let (a, ca) = track(&mut e, "A", 0.25, &SPANS[..1]);
    let (b, cb) = track(&mut e, "B", 0.5, &SPANS[1..2]);
    let before = e.session().to_json().unwrap();
    let steps = e.undo_history().len();
    let r = e.execute("clip.bounce", &json!({"clips": [ca[0].0, cb[0].0]})).unwrap();
    assert_eq!(r["tracks"].as_array().unwrap().len(), 2);
    assert_eq!(e.undo_history().len(), steps + 1);
    let (da, db) = (dest(&r, 0), dest(&r, 1));
    assert_eq!((print_of(&e, da, 0).0, print_of(&e, db, 0).0), (10_000, 30_000));
    assert_eq!((print_of(&e, da, 0).1[0], print_of(&e, db, 0).1[0]), (0.25, 0.5));
    assert!(e.session().track(a).unwrap().mixer.mute && e.session().track(b).unwrap().mixer.mute);
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), before);
}

#[test]
fn a_destination_replaces_inside_the_span_and_keeps_outside() {
    let mut e = Engine::default();
    let (_, c) = track(&mut e, "Keys", 0.25, &SPANS[1..2]);
    // The destination holds material before, inside and after the printed span [30000, 136000).
    let (d, _) = track(&mut e, "Dest", 0.75, &[(1_000, 5_000), (35_000, 45_000), (200_000, 210_000)]);
    let r = e.execute("clip.bounce", &json!({"clips": [c[0].0], "destination": d.0})).unwrap();
    assert_eq!(dest(&r, 0), d);
    let clips = e.session().track(d).unwrap().clips().to_vec();
    let starts: Vec<(i64, i64)> = clips.iter().map(|c| (c.start, c.end())).collect();
    assert_eq!(starts, vec![(1_000, 5_000), (30_000, 136_000), (200_000, 210_000)]);
}

#[test]
fn a_destination_with_clips_on_two_tracks_is_refused_before_any_print() {
    let mut e = Engine::default();
    let (_, ca) = track(&mut e, "A", 0.25, &SPANS[..1]);
    let (_, cb) = track(&mut e, "B", 0.5, &SPANS[..1]);
    let (d, _) = track(&mut e, "Dest", 0.75, &[]);
    let before = e.session().to_json().unwrap();
    let err = e.execute("clip.bounce", &json!({"clips": [ca[0].0, cb[0].0], "destination": d.0})).unwrap_err();
    assert!(matches!(err, transit_engine::EngineError::BadParams(..)), "{err:?}");
    assert_eq!(e.session().to_json().unwrap(), before);
}

#[test]
fn an_inactive_source_refuses_as_the_track_bounce_does() {
    let mut e = Engine::default();
    let (t, c) = track(&mut e, "Keys", 0.25, &SPANS);
    e.execute("track.make_inactive", &json!({"tracks": [t.0], "inactive": true})).unwrap();
    let before = e.session().to_json().unwrap();
    let err = e.execute("clip.bounce", &json!({"clips": [c[0].0]})).unwrap_err();
    let transit_engine::EngineError::BadParams(_, why) = &err else { panic!("{err:?}") };
    assert_eq!(why, "Keys is inactive: Track > Make Active first");
    assert_eq!(e.session().to_json().unwrap(), before);
}

#[test]
fn a_ranged_track_bounce_prints_the_range_and_the_default_stays_the_whole_track() {
    let mut e = Engine::default();
    let (t, _) = track(&mut e, "Keys", 0.25, &SPANS);
    let before = e.session().to_json().unwrap();
    let r = e.execute("track.bounce", &json!({"tracks": [t.0], "source": "leave", "range": {"start": 30_000, "length": 10_000}})).unwrap();
    let (start, print) = print_of(&e, dest(&r, 0), 0);
    assert_eq!((start, print.len(), print[0], print[9_999]), (30_000, 10_000, 0.25, 0.25));
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), before);
    // No range: the whole track from zero plus the two-second tail, as before.
    let r = e.execute("track.bounce", &json!({"tracks": [t.0], "source": "leave"})).unwrap();
    let (start, print) = print_of(&e, dest(&r, 0), 0);
    assert_eq!((start, print.len(), print[10_000], print[25_000], print[30_000]), (0, 310_000 + 96_000, 0.25, 0.0, 0.25));
    let bad = e.execute("track.bounce", &json!({"tracks": [t.0], "range": {"start": 5, "length": 0}}));
    assert!(bad.is_err());
}

#[test]
fn a_range_starting_at_or_past_the_end_of_the_material_is_refused_and_changes_nothing() {
    let mut e = Engine::default();
    let (t, _) = track(&mut e, "Keys", 0.25, &SPANS);
    let before = e.session().to_json().unwrap();
    for cmd in ["track.bounce", "track.commit"] {
        for start in [310_000, 310_001, i64::MAX - 1] {
            let err = e.execute(cmd, &json!({"tracks": [t.0], "range": {"start": start, "length": 10}})).unwrap_err();
            assert!(err.to_string().contains("after the end of the session's material"), "{cmd} {start}: {err}");
            assert_eq!(e.session().to_json().unwrap(), before, "{cmd} {start}: the session is unchanged");
        }
    }
    // The last sample of material is still accepted, and a whole-track bounce afterwards works.
    e.execute("track.bounce", &json!({"tracks": [t.0], "source": "leave", "range": {"start": 309_999, "length": 10}})).unwrap();
    e.execute("track.bounce", &json!({"tracks": [t.0], "source": "leave"})).unwrap();
}
