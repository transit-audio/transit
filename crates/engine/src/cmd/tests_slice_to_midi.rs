//! Slice to MIDI (ADR 0041 "Slice sources and Slice to MIDI", Gate 2 and the amendment
//! 2026-11-08 item 9, #269). The loop is a 48,000-frame source; marker sets are set on the patch
//! directly and mapped to pads with Slices to pads. Every refusal is checked against the whole
//! document and the undo history; the Gate 2 numbers are the oracle.

use crate::Engine;
use serde_json::{Value, json};
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_midi::{Note, Sequence};
use transit_model::sampler_patch::{Marker, Markers, PatchV1, SliceMode};
use transit_model::{Clip, ClipContent, ClipId, TrackId};
use transit_time::{Range, TICKS_PER_QUARTER};

const Q: i64 = TICKS_PER_QUARTER;

/// An instrument track with a patch and a slice loop over `frames` frames of a source at `rate`.
fn setup_at(rate: u32, frames: usize) -> (Engine, TrackId) {
    let mut e = Engine::default();
    assert_eq!(e.session().sample_rate.hz(), 48_000, "the session runs at 48 kHz");
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    // A new patch starts its pads at 36; these tests were written for pads from 60.
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    let buf = AudioBuffer { sample_rate: rate, channels: vec![(0..frames).map(|i| (i as f32 * 0.01).sin() * 0.5).collect()] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    (e, t)
}

fn setup() -> (Engine, TrackId) {
    setup_at(48_000, 48_000)
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

fn edit(e: &mut Engine, t: TrackId, f: impl FnOnce(&mut PatchV1)) {
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    f(ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap());
}

fn markers(frames: &[u64]) -> Markers {
    let markers = frames.iter().enumerate().map(|(k, f)| Marker { id: k as u32 + 1, frame: *f, ..Marker::default() }).collect();
    Markers { markers, next_id: frames.len() as u32 + 1, ..Markers::default() }
}

/// Manual holds markers at `frames` and shows; Slices to pads maps it at `root`.
fn sliced(e: &mut Engine, t: TrackId, frames: &[u64], root: u8) {
    edit(e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.manual = Some(markers(frames));
        l.mode = SliceMode::Manual;
    });
    e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": root})).unwrap();
}

/// Gate 2's loop: markers 0, 12,000 and 36,000, end 48,000, mapped at 60.
fn gate_loop() -> (Engine, TrackId) {
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 12_000, 36_000], 60);
    (e, t)
}

fn steps(e: &Engine) -> usize {
    e.undo_history().len()
}

/// A refused command: the document and the history are unchanged, and its preview refuses with
/// the same text.
fn refused(e: &mut Engine, params: Value) -> String {
    let (before, n) = (e.session_arc(), steps(e));
    let text = e.execute("mix.slice_to_midi", &params).expect_err("refused").to_string();
    assert!(*e.session_arc() == *before, "{params}: refused but the document changed");
    assert_eq!(steps(e), n, "{params}: refused but a step was recorded");
    let mut preview = params.clone();
    preview["preview"] = json!(true);
    let again = e.execute("mix.slice_to_midi", &preview).expect_err("the preview refuses too").to_string();
    assert_eq!(again, text, "{params}: the preview refuses with the same text");
    assert!(*e.session_arc() == *before, "{params}: the refused preview changed the document");
    text
}

/// A command that succeeds as one undo step: undo restores the document, redo reproduces it.
fn one_step(e: &mut Engine, params: Value) -> Value {
    let (before, n) = (e.session_arc(), steps(e));
    let out = e.execute("mix.slice_to_midi", &params).unwrap_or_else(|er| panic!("{params}: {er}"));
    let after = e.session_arc();
    assert_eq!(steps(e), n + 1, "{params}: not exactly one step");
    assert_eq!(e.undo_history().last().map(String::as_str), Some("Slice to MIDI"));
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(*e.session_arc() == *before, "{params}: one undo does not restore the document");
    e.execute("edit.redo", &json!({})).unwrap();
    assert!(*e.session_arc() == *after, "{params}: redo does not reproduce the edit");
    out
}

fn ticks(out: &Value) -> Vec<i64> {
    out["ticks"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect()
}

fn clip_of(e: &Engine, out: &Value) -> Clip {
    let id: ClipId = serde_json::from_value(out["clip"].clone()).unwrap();
    e.session().find_clip(id).unwrap().1.clone()
}

fn notes_of(c: &Clip) -> Vec<Note> {
    match &c.content {
        ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => panic!("not a MIDI clip"),
    }
}

/// (key, start, length) of each note, start relative to the clip.
fn shape(c: &Clip) -> Vec<(u8, i64, i64)> {
    notes_of(c).iter().map(|n| (n.pitch, n.start, n.length)).collect()
}

fn set_tempo(e: &mut Engine, tempos: &[(i64, f64)]) {
    let map = &mut e.session_mut().tempo;
    for (tick, bpm) in tempos {
        map.set_tempo(*tick, *bpm).unwrap();
    }
}

#[test]
fn fit_mode_notes_at_two_tempos() {
    // Fit to four quarter notes: 0, 1, 3 and end 4 beats, wherever it is placed and whatever the
    // tempo (the beat length is musical).
    for (bpm, at) in [(120.0, 0), (90.0, 3 * Q + 17)] {
        let (mut e, t) = gate_loop();
        set_tempo(&mut e, &[(0, bpm)]);
        let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": at}));
        assert_eq!(ticks(&out), [at, at + Q, at + 3 * Q, at + 4 * Q], "{bpm} BPM");
        let c = clip_of(&e, &out);
        assert_eq!(shape(&c), [(60, 0, Q), (61, Q, 2 * Q), (62, 3 * Q, Q)]);
        let s = e.session();
        assert_eq!((c.start, c.end()), (s.tempo.tick_to_samples(at, s.sample_rate), s.tempo.tick_to_samples(at + 4 * Q, s.sample_rate)));
        assert_eq!(s.tempo.samples_to_ticks(c.start, s.sample_rate), at, "the clip's start tick is the placement");
    }
    // The default beat length is the loop's duration at the session tempo at `at`: one second is
    // two quarter notes at 120 BPM and one and a half at 90.
    for (bpm, want) in [(120.0, [0, 480, 1440, 1920]), (90.0, [0, 360, 1080, 1440])] {
        let (mut e, t) = gate_loop();
        set_tempo(&mut e, &[(0, bpm)]);
        let out = one_step(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
        assert_eq!(ticks(&out), want, "{bpm} BPM");
        assert_eq!(out["beat_length"], json!(want[3] as f64 / Q as f64));
    }
    // The loop's stored beat length wins over the default.
    let (mut e, t) = gate_loop();
    e.execute("mix.sampler_slice_set", &json!({"track": t, "beat_length": 8.0})).unwrap();
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert_eq!(ticks(&out), [0, 2 * Q, 6 * Q, 8 * Q]);
    // A second source rate: 48,000 frames at 96 kHz are half a second (the session holds them as
    // 24,000 frames at its 48 kHz), one quarter at 120 BPM.
    let (mut e, t) = setup_at(96_000, 48_000);
    sliced(&mut e, t, &[0, 6_000, 18_000], 60);
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert_eq!(ticks(&out), [0, 240, 720, 960]);
    // Nearest tick, ties upward: 960 * 1 * 25 / 48,000 is exactly 0.5 and becomes 1; 26 frames
    // are 0.52 and become 1; 12,012 frames are 240.24 and become 240.
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 25, 12_012], 60);
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 1.0, "at": 0}));
    assert_eq!(ticks(&out), [0, 1, 240, 960]);
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 26, 12_012], 60);
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 1.0, "at": 0}));
    assert_eq!(ticks(&out), [0, 1, 240, 960]);
}

#[test]
fn keep_mode_follows_tempo_map() {
    // Keep duration: at 120 BPM 0, .5, 1.5 and end 2 beats; at 60 BPM 0, .25, .75 and end 1.
    for (bpm, want) in [(120.0, [0, Q / 2, 3 * Q / 2, 2 * Q]), (60.0, [0, Q / 4, 3 * Q / 4, Q])] {
        let (mut e, t) = gate_loop();
        set_tempo(&mut e, &[(0, bpm)]);
        let out = one_step(&mut e, json!({"track": t, "timing": "keep", "at": 0}));
        assert_eq!(ticks(&out), want, "{bpm} BPM");
        assert_eq!(out["beat_length"], Value::Null);
    }
    // 120 BPM to tick 960, then 60 BPM. Samples 0, 12,000, 36,000, 48,000: the first beat is
    // 24,000 samples, so 12,000 is tick 480; 36,000 is 12,000 samples into the 60 BPM part
    // (48,000 samples a beat), 960 + 240 = 1,200; 48,000 is 960 + 480 = 1,440.
    let (mut e, t) = gate_loop();
    set_tempo(&mut e, &[(0, 120.0), (960, 60.0)]);
    let out = one_step(&mut e, json!({"track": t, "timing": "keep", "at": 0}));
    assert_eq!(ticks(&out), [0, 480, 1200, 1440]);
    let c = clip_of(&e, &out);
    assert_eq!(shape(&c), [(60, 0, 480), (61, 480, 720), (62, 1200, 240)]);
    assert_eq!((c.start, c.end()), (0, 48_000), "the clip lasts the loop's duration");
    // Placed after the tempo change, at tick 1,920 (sample 72,000), all at 60 BPM: 0, .25, .75, 1.
    let out = one_step(&mut e, json!({"track": t, "timing": "keep", "at": 2 * Q}));
    assert_eq!(ticks(&out), [2 * Q, 2 * Q + Q / 4, 2 * Q + 3 * Q / 4, 3 * Q]);
    // A one-second source at 24 kHz: the session holds it at its own 48 kHz (48,000 frames), so
    // the loop's frames are session frames and Gate 2's markers give Gate 2's ticks.
    let (mut e, t) = setup_at(24_000, 24_000);
    sliced(&mut e, t, &[0, 12_000, 36_000], 60);
    let out = one_step(&mut e, json!({"track": t, "timing": "keep", "at": 0}));
    assert_eq!(ticks(&out), [0, Q / 2, 3 * Q / 2, 2 * Q]);
}

#[test]
fn notes_are_root_plus_index_velocity_100_channel_1_min_one_tick() {
    // Mapped at 62. The last slice starts at frame 47,999: fit to four beats it starts on tick
    // 3,839.92, rounded to 3,840, the loop's end, so its note is one tick long.
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 12_000, 47_999], 62);
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 1000}));
    assert_eq!((out["first_key"].clone(), out["last_key"].clone(), out["notes"].clone()), (json!(62), json!(64), json!(3)));
    assert_eq!((out["velocity"].clone(), out["channel"].clone(), out["placement"].clone()), (json!(100), json!(1), json!("explicit")));
    assert_eq!((out["at_tick"].clone(), out["takes"].clone()), (json!(1000), json!(false)));
    assert_eq!(out["position"], json!("1|2|40"), "tick 1,000 is bar 1, beat 2, tick 40 in 4/4 at 960 per beat");
    let c = clip_of(&e, &out);
    let notes = notes_of(&c);
    assert_eq!(shape(&c), [(62, 0, Q), (63, Q, 2880), (64, 3840, 1)]);
    assert!(notes.iter().all(|n| (n.velocity, n.channel) == (100, 0)), "velocity 100 on channel 1: {notes:?}");
    let s = e.session();
    assert_eq!(c.end(), s.tempo.tick_to_samples(1000 + 3841, s.sample_rate), "the clip holds the one-tick note");
    assert_eq!(s.track(t).unwrap().clips().len(), 1);
}

#[test]
fn root_overflow_refused() {
    let (mut e, t) = setup();
    let frames: Vec<u64> = (0..8).map(|k| k * 6_000).collect();
    sliced(&mut e, t, &frames, 60);
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0, "root": 121}));
    assert!(text.ends_with("Root 121 is too high for 8 slices: the highest root is 120."), "{text}");
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0, "root": 128}));
    assert!(text.contains("`root` must be an integer from 0 to 127"), "{text}");
    // A chosen root must match the ascending mapping.
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0, "root": 61}));
    assert!(text.ends_with("Root 61 does not match the slices' keys, which start at 60: use Map slices to keys."), "{text}");
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "at": 0, "root": 60}));
    assert_eq!((out["first_key"].clone(), out["last_key"].clone()), (json!(60), json!(67)));
    // The highest mapping: eight slices at 120 end on key 127.
    let (mut e, t) = setup();
    sliced(&mut e, t, &frames, 120);
    let out = one_step(&mut e, json!({"track": t, "timing": "keep", "at": 0}));
    assert_eq!((out["first_key"].clone(), out["last_key"].clone()), (json!(120), json!(127)));
}

#[test]
fn one_undo_step() {
    // No overlap, then an overlap over the first clip: each is one step that undo restores whole.
    let (mut e, t) = gate_loop();
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 0}));
    assert_eq!(out["takes"], json!(false));
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 2.0, "at": Q}));
    assert_eq!(out["takes"], json!(true));
    assert_eq!(e.session().track(t).unwrap().playlists.len(), 2, "the overlap duplicated the playlist in the same step");
}

/// A MIDI clip with one note at its start, placed directly.
fn old_clip(e: &mut Engine, t: TrackId, start: i64, len: i64, pitch: u8) -> ClipId {
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch, velocity: 90, release_velocity: 64, channel: 0, start: 0, length: Q });
    let s = e.session_mut();
    let id = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(id, "old", start, len, seq));
    id
}

struct Dir(transit_test_support::TestDir);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = transit_test_support::TestDir::new(&format!("slice-to-midi-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

#[test]
fn overlap_makes_a_take_stack() {
    // At 120 BPM a quarter note is 24,000 samples. The old take covers 48,000..240,000; an earlier
    // clip 0..24,000 and a later one 288,000..312,000 are outside. The generated clip, fit to four
    // quarters at tick 3,840, covers 96,000..192,000, inside the old take.
    let (mut e, t) = gate_loop();
    let earlier = old_clip(&mut e, t, 0, 24_000, 40);
    let take = old_clip(&mut e, t, 48_000, 192_000, 50);
    let later = old_clip(&mut e, t, 288_000, 24_000, 70);
    let whole = e.session().find_clip(take).unwrap().1.clone();
    let preview = e.execute("mix.slice_to_midi", &json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 4 * Q, "preview": true})).unwrap();
    assert_eq!((preview["overlap"].clone(), preview["takes"].clone()), (json!(true), json!(true)));
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 4 * Q}));
    assert_eq!(out["takes"], json!(true));
    let new: ClipId = serde_json::from_value(out["clip"].clone()).unwrap();
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.playlists.len(), 2, "the active playlist was duplicated");
    assert_eq!(tr.active_playlist, 0, "the original playlist stays active (ADR 0041)");
    let active = tr.clips();
    let alternate = &tr.playlists[1];
    // The alternate playlist (the duplicate) keeps the old take whole, numbered take 1.
    let kept: Vec<(i64, i64, Option<u32>)> = alternate.clips.iter().map(|c| (c.start, c.end(), c.take)).collect();
    assert_eq!(kept, [(0, 24_000, None), (48_000, 240_000, Some(1)), (288_000, 312_000, None)]);
    let old = alternate.clips.iter().find(|c| c.range() == Range::new(48_000, 240_000)).unwrap();
    assert_eq!(notes_of(old), notes_of(&whole), "the old take is whole, notes and all");
    // The original playlist: the material outside the range keeps its clips and ids, the old take
    // is cut around the new one (take 2), which plays over its range.
    let playing: Vec<(i64, i64, Option<u32>)> = active.iter().map(|c| (c.start, c.end(), c.take)).collect();
    assert_eq!(
        playing,
        [(0, 24_000, None), (48_000, 96_000, Some(1)), (96_000, 192_000, Some(2)), (192_000, 240_000, Some(1)), (288_000, 312_000, None)]
    );
    let id_at = |start: i64| active.iter().find(|c| c.start == start).map(|c| c.id);
    assert_eq!((id_at(0), id_at(96_000), id_at(288_000)), (Some(earlier), Some(new), Some(later)), "outside clips keep their ids");
    let stacks = tr.take_stacks();
    assert_eq!(stacks.len(), 1, "one take stack");
    assert_eq!(stacks[0].active(), Some(2), "the new take plays");
    // Choosing the old take back makes it play whole again (its pieces join); choosing the new
    // one again puts the new clip back (ADR 0039 decision 3).
    let before_back = e.session_arc();
    e.execute("takes.set_active", &json!({"track": t, "take": 1, "stack": 0})).unwrap();
    let tr = e.session().track(t).unwrap();
    let back: Vec<(i64, i64, Option<u32>)> = tr.clips().iter().map(|c| (c.start, c.end(), c.take)).collect();
    assert_eq!(back, [(0, 24_000, None), (48_000, 240_000, Some(1)), (288_000, 312_000, None)]);
    let joined = tr.clips().iter().find(|c| c.range() == Range::new(48_000, 240_000)).unwrap();
    assert_eq!(notes_of(joined), notes_of(&whole), "the old take plays whole again");
    assert!(tr.playlists[1].clips.iter().any(|c| c.id == new), "the new take waits on the alternate playlist");
    e.execute("takes.set_active", &json!({"track": t, "take": 2, "stack": 0})).unwrap();
    assert_eq!(e.session().track(t).unwrap().take_stacks()[0].active(), Some(2));
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(*e.session_arc() == *before_back);
    // Save and open: the playlists, the numbers and the notes come back.
    let d = Dir::new("takes");
    let file = d.path("takes.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": file})).unwrap();
    let (a, b) = (e.session().track(t).unwrap(), back.session().track(t).unwrap());
    assert_eq!((b.active_playlist, b.playlists.len()), (a.active_playlist, a.playlists.len()));
    for (pa, pb) in a.playlists.iter().zip(&b.playlists) {
        assert_eq!(serde_json::to_value(&pb.clips).unwrap(), serde_json::to_value(&pa.clips).unwrap(), "playlist {}", pa.name);
    }
}

#[test]
fn collapsed_pair_refused() {
    // Fit to four beats, frames 12,000 and 12,001 are ticks 960 and 960.08: one tick.
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 12_000, 12_001, 36_000], 60);
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 0}));
    assert!(text.ends_with("Slices 2 and 3 land on the same tick; zoom or remap."), "{text}");
    // A longer beat length separates them.
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 64.0, "at": 0}));
    assert_eq!(ticks(&out)[1..3], [15_360, 15_361]);
}

#[test]
fn frozen_track_refused() {
    let (mut e, t) = gate_loop();
    let tr = e.session_mut().track_mut(t).unwrap();
    tr.freeze =
        Some(transit_model::Frozen { source: transit_model::SourceId(1), start: 0, length: 10, hash: tr.freeze_hash(), revision: 1, latency: 0 });
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert_eq!(text, crate::EngineError::TrackFrozen.to_string());
}

#[test]
fn no_slices_refused() {
    // Auto shows and holds no markers yet.
    let (mut e, t) = setup();
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert!(text.ends_with("The Auto set has no slice markers yet: detect or add slices first."), "{text}");
    // Markers without pads: nothing to read keys from.
    edit(&mut e, t, |p| p.slicing.as_mut().unwrap().auto = Some(markers(&[0, 24_000])));
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert!(text.ends_with("The slices have no pads yet: use Slices to pads."), "{text}");
}

#[test]
fn stale_mapping_refused_offers_map() {
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 12_000, 24_000, 36_000], 60);
    // Swap slices 1 and 2 between pads A1 and A2 by the pad commands.
    let (za, zb) = (patch(&e, t).pad(0).zone.unwrap(), patch(&e, t).pad(1).zone.unwrap());
    for (cmd, pad, zone) in [
        ("mix.sampler_pad_unbind", 1, None),
        ("mix.sampler_pad_unbind", 2, None),
        ("mix.sampler_pad_bind", 1, Some(zb)),
        ("mix.sampler_pad_bind", 2, Some(za)),
    ] {
        let mut p = json!({"track": t, "pad": pad});
        if let Some(z) = zone {
            p["zone"] = json!(z + 1);
        }
        e.execute(cmd, &p).unwrap();
    }
    let stale = patch(&e, t);
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert!(text.ends_with("The slices' keys no longer run in order: slices 1 and 2 swapped pads."), "{text}");
    assert_eq!(patch(&e, t), stale, "the refusal remapped nothing");
    // The separate edit the UI offers makes it succeed.
    e.execute("mix.sampler_map_slices", &json!({"track": t, "root": 60})).unwrap();
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 0}));
    assert_eq!(shape(&clip_of(&e, &out)), [(60, 0, Q), (61, Q, Q), (62, 2 * Q, Q), (63, 3 * Q, Q)]);
}

#[test]
fn uses_the_showing_set_only() {
    // Auto holds three slices on pads from 36, Manual five from 60.
    let (mut e, t) = setup();
    edit(&mut e, t, |p| p.slicing.as_mut().unwrap().auto = Some(markers(&[0, 16_000, 32_000])));
    e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 36})).unwrap();
    sliced(&mut e, t, &[0, 9_600, 19_200, 28_800, 38_400], 60);
    let manual = e.execute("mix.slice_to_midi", &json!({"track": t, "timing": "fit", "beat_length": 5.0, "at": 0, "preview": true})).unwrap();
    assert_eq!((manual["notes"].clone(), manual["first_key"].clone()), (json!(5), json!(60)));
    assert_eq!(ticks(&manual), [0, Q, 2 * Q, 3 * Q, 4 * Q, 5 * Q]);
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "auto"})).unwrap();
    let out = one_step(&mut e, json!({"track": t, "timing": "fit", "beat_length": 3.0, "at": 0}));
    assert_eq!(shape(&clip_of(&e, &out)), [(36, 0, Q), (37, Q, Q), (38, 2 * Q, Q)]);
}

#[test]
fn preview_matches_applied_and_edits_nothing() {
    let (mut e, t) = gate_loop();
    old_clip(&mut e, t, 0, 24_000, 40);
    for params in [json!({"track": t, "timing": "keep", "at": 0}), json!({"track": t, "timing": "fit", "beat_length": 4.0, "at": 8 * Q})] {
        let (before, n) = (e.session_arc(), steps(&e));
        let mut p = params.clone();
        p["preview"] = json!(true);
        let mut preview = e.execute("mix.slice_to_midi", &p).unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()), "a preview edits nothing");
        assert_eq!(steps(&e), n, "and records no step");
        assert_eq!(preview["clip"], Value::Null);
        let overlap = preview.as_object_mut().unwrap().remove("overlap").unwrap();
        let mut applied = one_step(&mut e, params.clone());
        assert_eq!(overlap, applied["takes"], "the preview's overlap is the applied take placement");
        applied["clip"] = Value::Null;
        assert_eq!(preview, applied, "the preview is the applied answer");
    }
}

#[test]
fn refuses_without_at() {
    let (mut e, t) = gate_loop();
    for params in [json!({"track": t, "timing": "fit"}), json!({"track": t, "timing": "keep", "at": null}), json!({"track": t})] {
        let text = refused(&mut e, params);
        assert!(text.ends_with("Slice to MIDI needs a placement."), "{text}");
    }
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": -1}));
    assert!(text.ends_with("`at` must be a tick at or after 0"), "{text}");
}

#[test]
fn refuses_a_placement_past_the_timeline() {
    // The first tick past the timeline's end, and one near the largest tick: refused before any
    // arithmetic on `at`, in both timings, the preview too.
    let (mut e, t) = gate_loop();
    let s = e.session();
    let first_past = (0..)
        .map(|k: i64| s.tempo.samples_to_ticks(transit_time::MAX_POSITION, s.sample_rate) + k)
        .find(|tick| s.tempo.tick_to_samples(*tick, s.sample_rate) > transit_time::MAX_POSITION)
        .unwrap();
    for at in [i64::MAX - 10, first_past] {
        for timing in ["fit", "keep"] {
            let text = refused(&mut e, json!({"track": t, "timing": timing, "beat_length": null, "at": at}));
            assert!(text.ends_with("The placement is past the end of the session's timeline."), "{timing} at {at}: {text}");
        }
    }
}

/// Moves zones between pads with the pad commands: every named pad is unbound, then each takes
/// the zone that pad `from` held (1-based pads).
fn rebind(e: &mut Engine, t: TrackId, moves: &[(u8, u8)]) {
    let held: Vec<u16> = moves.iter().map(|(_, from)| patch(e, t).pad(from - 1).zone.unwrap()).collect();
    for (pad, _) in moves {
        e.execute("mix.sampler_pad_unbind", &json!({"track": t, "pad": pad})).unwrap();
    }
    for ((pad, _), z) in moves.iter().zip(held) {
        e.execute("mix.sampler_pad_bind", &json!({"track": t, "pad": pad, "zone": z + 1})).unwrap();
    }
}

#[test]
fn stale_text_names_the_swapped_pair_or_the_first_break() {
    // Slices 3 and 5 swap pads (keys 60, 61, 64, 63, 62): the text names exactly those two.
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 9_600, 19_200, 28_800, 38_400], 60);
    rebind(&mut e, t, &[(3, 5), (5, 3)]);
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert!(text.ends_with("The slices' keys no longer run in order: slices 3 and 5 swapped pads."), "{text}");
    // A rotation (slice 1 on A2, 2 on A3, 3 on A1: keys 61, 62, 60) is no single swap: the text
    // names the first break, which is true for any order.
    let (mut e, t) = setup();
    sliced(&mut e, t, &[0, 16_000, 32_000], 60);
    rebind(&mut e, t, &[(2, 1), (3, 2), (1, 3)]);
    let text = refused(&mut e, json!({"track": t, "timing": "fit", "at": 0}));
    assert!(text.ends_with("The slices' keys no longer run in order: slice 3 on pad A1 plays key 60, not 63 after slice 2."), "{text}");
}

#[test]
fn keep_first_boundary_is_the_placement() {
    // An 11,025 Hz session at 1000 BPM has 0.689 samples a tick: tick 2 is sample 1, and sample 1
    // converts back to tick 1.45, which rounds to 1. The first boundary is the placement itself.
    let mut e = Engine::default();
    e.session_mut().sample_rate = transit_time::SampleRate::new(11_025).unwrap();
    set_tempo(&mut e, &[(0, 1000.0)]);
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    let buf = AudioBuffer { sample_rate: 11_025, channels: vec![(0..11_025).map(|i| (i as f32 * 0.01).sin() * 0.5).collect()] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    sliced(&mut e, t, &[0, 2_756, 8_269], 60);
    let s = e.session();
    assert_eq!(s.tempo.samples_to_ticks(s.tempo.tick_to_samples(2, s.sample_rate), s.sample_rate), 1, "the coarse grid rounds tick 2 down");
    let out = one_step(&mut e, json!({"track": t, "timing": "keep", "at": 2}));
    assert_eq!(ticks(&out)[0], 2, "the first boundary is the placement");
    assert_eq!(notes_of(&clip_of(&e, &out))[0].start, 0, "the first note starts with the clip");
}
