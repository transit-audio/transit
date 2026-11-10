//! Edits around a looped clip (ADR 0019) must not change how it sounds: every start-trim path
//! keeps the cycles on the old grid, consolidate writes the cycles out, and operations that render
//! audio to a new file clear the loop.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use serde_json::json;
use transit_audio_io::AudioBuffer;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

fn note(pitch: u8, beat: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: beat * Q, length: Q }
}

/// A 16-beat MIDI clip at 0: C60 at beat 0, E64 at beat 2, a CC at beat 1; looped over beats 0..4.
fn midi() -> (Engine, TrackId, ClipId, Samples) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let beat = s.tempo.tick_to_samples(Q, s.sample_rate);
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 0));
    seq.notes.push(note(64, 2));
    seq.ctrls.push(CtrlEvent { tick: Q, channel: 0, kind: CtrlKind::Cc(1), value: 90 });
    let id = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", 0, 16 * beat, seq));
    s.edit.selected_tracks = vec![t];
    let mut e = Engine::new(s);
    e.execute("clip.set_loop", &json!({"clip": id.0, "start": 0, "end": 4 * beat})).unwrap();
    (e, t, id, beat)
}

fn ons(e: &Engine, t: TrackId, beat: Samples) -> Vec<(f64, u8)> {
    transit_mix::clip_note_ons(e.session(), t, 0, (20 * beat) as usize).into_iter().map(|(a, k)| (a as f64 / beat as f64, k)).collect()
}

fn full() -> Vec<(f64, u8)> {
    vec![(0.0, 60), (2.0, 64), (4.0, 60), (6.0, 64), (8.0, 60), (10.0, 64), (12.0, 60), (14.0, 64)]
}

#[test]
fn baseline_plays_every_cycle() {
    let (e, t, _, beat) = midi();
    assert_eq!(ons(&e, t, beat), full());
}

#[test]
fn trim_drag_keeps_the_cycles_on_the_old_grid_and_the_cut_note_silent() {
    let (mut e, t, id, beat) = midi();
    e.execute("edit.trim_clip", &json!({"clip": id.0, "start": beat})).unwrap();
    assert_eq!(ons(&e, t, beat), full()[1..].to_vec(), "no C at the new start; Cs at 4, 8, 12");
    // The controller the trim cut is kept: it still plays in the later cycles.
    let c = e.session().find_clip(id).unwrap().1.clone();
    let transit_model::ClipContent::Midi { sequence } = &c.content else { panic!() };
    assert_eq!(sequence.ctrls.len(), 1);
}

#[test]
fn trim_start_into_a_later_cycle_keeps_the_phase() {
    let (mut e, t, _, beat) = midi();
    e.execute("edit.trim_start_to_insertion", &json!({"tracks": [t.0], "at": 9 * beat})).unwrap();
    assert_eq!(ons(&e, t, beat), vec![(10.0, 64), (12.0, 60), (14.0, 64)]);
}

#[test]
fn trim_to_selection_keeps_the_phase() {
    let (mut e, t, _, beat) = midi();
    e.execute("edit.trim_to_selection", &json!({"tracks": [t.0], "start": 9 * beat, "end": 15 * beat})).unwrap();
    assert_eq!(ons(&e, t, beat), vec![(10.0, 64), (12.0, 60), (14.0, 64)]);
}

#[test]
fn copy_range_and_paste_keeps_the_phase() {
    let (mut e, t, _, beat) = midi();
    let s = e.session_mut();
    let parts = crate::edit::copy_range(s, t, Range::new(beat, 16 * beat));
    let t2 = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    crate::edit::paste(s, t2, beat, &parts, 15 * beat, false).unwrap();
    assert_eq!(ons(&e, t2, beat), full()[1..].to_vec());
}

#[test]
fn split_keeps_both_halves_on_the_old_grid() {
    let (mut e, t, _, beat) = midi();
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 5 * beat})).unwrap();
    assert_eq!(e.session().track(t).unwrap().clips().len(), 2);
    assert_eq!(ons(&e, t, beat), full());
}

/// A one-bar kick loop (ticks 0, 960, 1,920, 2,880) dragged to `bars` bars at `hz`, `bpm` and an
/// optional tempo change `(tick, bpm)`, each edge stored as the editor stores it (a tick converted).
fn kick_loop(hz: u32, bpm: f64, change: Option<(i64, f64)>, bars: i64) -> (Session, TrackId) {
    let mut s = Session {
        sample_rate: transit_time::SampleRate::new(hz).unwrap(),
        tempo: transit_time::TempoMap::new(bpm, 4, 4).unwrap(),
        ..Session::default()
    };
    if let Some((tick, bpm)) = change {
        s.tempo.set_tempo(tick, bpm).unwrap();
    }
    let sr = s.sample_rate;
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let mut seq = Sequence::default();
    for beat in 0..4 {
        seq.notes.push(Note { pitch: 36, velocity: 127, release_velocity: 64, channel: 0, start: beat * Q, length: 240 });
    }
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "kick", 0, s.tempo.tick_to_samples(bars * 4 * Q, sr), seq);
    c.set_loop(Some(transit_model::ClipLoop { start: 0, end: s.tempo.tick_to_samples(4 * Q, sr) }));
    crate::edit::place_clip(&mut s, t, c);
    (s, t)
}

/// A split of a looped MIDI clip takes the cut's cycle phase in ticks (ADR 0019): the two halves
/// together play every kick of the unsplit clip, each at its tick's sample, none dropped, none
/// doubled. Split on every bar line of a 64-bar clip and a beat and a half after it, at 44.1 and
/// 48 kHz and four tempos whose bar is not a whole number of samples (a phase counted in samples
/// drifts by the bar's rounding per cycle: at 48 kHz and 102.5 BPM a split at bar 61 moved the right
/// half a tick early and dropped its downbeat), and under a tempo change at bar 3 (120 BPM to 90: a
/// sample phase of a split at bar 4 was 32,000 samples where the true phase is 0).
#[test]
fn splitting_a_looped_kick_on_any_bar_keeps_every_kick_on_its_tick() {
    const BARS: i64 = 64;
    let mut cases: Vec<(u32, f64, Option<(i64, f64)>)> = Vec::new();
    for hz in [44_100, 48_000] {
        for bpm in [93.7, 102.5, 127.3, 174.0] {
            cases.push((hz, bpm, None));
        }
    }
    cases.push((48_000, 120.0, Some((8 * Q, 90.0))));
    cases.push((44_100, 102.5, Some((8 * Q, 140.0))));
    let mut bad = Vec::new();
    for (hz, bpm, change) in cases {
        let (s0, t) = kick_loop(hz, bpm, change, BARS);
        let sr = s0.sample_rate;
        let at = |tick: i64| s0.tempo.tick_to_samples(tick, sr);
        let len = at(BARS * 4 * Q);
        let want: Vec<(Samples, u8)> = (0..BARS * 4).map(|b| (at(b * Q), 36)).collect();
        assert_eq!(transit_mix::clip_note_ons(&s0, t, 0, len as usize), want, "{hz} Hz {bpm} BPM {change:?}: the unsplit clip");
        for bar in 1..BARS {
            for past in [0, 6 * Q / 4] {
                let cut = at(bar * 4 * Q + past);
                let mut s = s0.clone();
                assert_eq!(crate::edit::separate_at(&mut s, t, cut), 1);
                let got = transit_mix::clip_note_ons(&s, t, 0, len as usize);
                if got != want {
                    let missing: Vec<Samples> = want.iter().filter(|w| !got.contains(w)).map(|w| w.0).collect();
                    let extra: Vec<Samples> = got.iter().filter(|g| !want.contains(g)).map(|g| g.0).collect();
                    bad.push(format!(
                        "{hz} Hz {bpm} BPM {change:?}, split at bar {} (+{past} ticks, sample {cut}): {} kicks, missing {missing:?}, off-tick {extra:?}",
                        bar + 1,
                        got.len()
                    ));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} splits:\n{}", bad.len(), bad.join("\n"));
}

/// Every clip event of track `t` over `0..len` as (sample, message), through the external output
/// (the same visit playback uses): note-ons, note-offs and controllers.
fn clip_messages(s: &Session, t: TrackId, len: Samples) -> Vec<(Samples, Vec<u8>)> {
    let mut out = Vec::new();
    transit_mix::external_midi_events(s, s.track(t).unwrap(), 0, len as usize, false, |e| out.push((e.offset as Samples, e.message().to_vec())));
    out
}

/// A split of a looped MIDI clip at a sample between two ticks: a point on the tick the cut rounds
/// to belongs to the right half (whose grid starts there, up to half a tick before its start
/// sample), a point on an earlier tick to the left half, so the two halves together play every
/// note-on and controller of the unsplit clip once: none doubled on the cut's tick (48 kHz,
/// 102.5 BPM, a cut at 112,400 played the kick at 112,390 in both halves), none dropped. Cuts at
/// every off-tick sample after a kick, on both sides of the rounding midpoint, at 44.1 and 48 kHz,
/// four tempos and a tempo change inside the clip. A note sounding across the cut ends at it in
/// the left half; every note-on still gets exactly one note-off at or after it.
#[test]
fn splitting_a_looped_kick_between_ticks_plays_each_kick_once() {
    const BARS: i64 = 16;
    let mut cases: Vec<(u32, f64, Option<(i64, f64)>)> = Vec::new();
    for hz in [44_100, 48_000] {
        for bpm in [93.7, 102.5, 127.3, 174.0] {
            cases.push((hz, bpm, None));
        }
    }
    cases.push((48_000, 120.0, Some((8 * Q, 90.0))));
    cases.push((44_100, 102.5, Some((8 * Q, 140.0))));
    let mut bad = Vec::new();
    let mut cuts = 0;
    for (hz, bpm, change) in cases {
        let (mut s0, t) = kick_loop(hz, bpm, change, BARS);
        let tr = s0.track_mut(t).unwrap();
        tr.midi_output = transit_model::MidiOutput::External {
            port: transit_model::MidiOutPort::default(),
            channel: transit_model::MidiOutChannel::AsRecorded,
            offset_ms: 0.0,
        };
        let c = &mut tr.playlist_mut().unwrap().clips[0];
        let transit_model::ClipContent::Midi { sequence } = &mut c.content else { panic!() };
        sequence.ctrls.push(CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Cc(1), value: 90 });
        let sr = s0.sample_rate;
        let at = |tick: i64| s0.tempo.tick_to_samples(tick, sr);
        let len = at(BARS * 4 * Q);
        let split = |m: &[(Samples, Vec<u8>)]| -> (Vec<(Samples, Vec<u8>)>, Vec<(Samples, Vec<u8>)>) {
            let mut ons: Vec<_> = m.iter().filter(|(_, b)| b[0] & 0xF0 != 0x80).cloned().collect();
            ons.sort();
            let offs = m.iter().filter(|(_, b)| b[0] & 0xF0 == 0x80).cloned().collect();
            (ons, offs)
        };
        let (want, _) = split(&clip_messages(&s0, t, len));
        assert_eq!(want.iter().filter(|(_, b)| b[0] & 0xF0 == 0x90).count() as i64, BARS * 4, "{hz} Hz {bpm} BPM: the unsplit clip");
        for beat in (1..BARS * 4).step_by(3) {
            let (k, next) = (at(beat * Q), at(beat * Q + 1));
            for cut in k + 1..next {
                cuts += 1;
                let mut s = s0.clone();
                assert_eq!(crate::edit::separate_at(&mut s, t, cut), 1);
                let (got, offs) = split(&clip_messages(&s, t, len));
                let note_ons = got.iter().filter(|(_, b)| b[0] & 0xF0 == 0x90).count();
                let paired =
                    offs.len() == note_ons && got.iter().filter(|(_, b)| b[0] & 0xF0 == 0x90).zip(offs.iter()).all(|((on, _), (off, _))| off >= on);
                if got != want || !paired {
                    let missing: Vec<Samples> = want.iter().filter(|w| !got.contains(w)).map(|w| w.0).collect();
                    let extra: Vec<Samples> = got.iter().filter(|g| !want.contains(g)).map(|g| g.0).collect();
                    let doubled: Vec<Samples> = got.windows(2).filter(|w| w[0] == w[1]).map(|w| w[0].0).collect();
                    bad.push(format!(
                        "{hz} Hz {bpm} BPM {change:?}, cut at {cut} ({} after the kick at {k}, next tick {next}): {} events, doubled {doubled:?}, missing {missing:?}, off-tick {extra:?}, {} offs for {note_ons} ons",
                        cut - k,
                        got.len(),
                        offs.len()
                    ));
                }
            }
        }
    }
    assert!(cuts > 3000, "{cuts} cuts");
    assert!(bad.is_empty(), "{} of {cuts} cuts:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn consolidate_writes_the_cycles_out_as_they_sound() {
    let (mut e, t, _, beat) = midi();
    let before = ons(&e, t, beat);
    e.execute("edit.consolidate", &json!({"tracks": [t.0], "start": 0, "end": 16 * beat})).unwrap();
    let clips = e.session().track(t).unwrap().clips().to_vec();
    assert_eq!(clips.len(), 1);
    assert!(clips[0].looping.is_none());
    assert_eq!(ons(&e, t, beat), before);
    let transit_model::ClipContent::Midi { sequence } = &clips[0].content else { panic!() };
    assert_eq!(sequence.ctrls.len(), 4, "the controller in every cycle");
}

fn audio() -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let data: Vec<f32> = (0..1000).map(|i| (i + 1) as f32 / 1000.0).collect();
    let src = crate::io::add_source(&mut s, "a", AudioBuffer { sample_rate: 48_000, channels: vec![data] }, None, transit_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, Clip::audio(id, "a", src, 0, 0, 100));
    s.edit.selected_tracks = vec![t];
    (Engine::new(s), t, id)
}

fn render(e: &Engine, t: TrackId, len: Samples) -> Vec<f32> {
    transit_mix::render_clips(e.session(), t, Range::new(0, len)).remove(0)
}

#[test]
fn reverse_of_a_looped_clip_equals_the_reversed_live_render() {
    let (mut e, t, id) = audio();
    e.execute("clip.set_loop", &json!({"clip": id.0, "start": 20, "end": 100})).unwrap();
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 330})).unwrap();
    let mut want = render(&e, t, 330);
    want.reverse();
    e.execute("audiosuite.process", &json!({"process": "reverse", "clip": id.0})).unwrap();
    let c = e.session().track(t).unwrap().clips()[0].clone();
    assert_eq!(c.looping, None, "the new file holds every cycle");
    assert_eq!(render(&e, t, 330), want);
}

#[test]
fn time_stretch_of_a_looped_clip_stretches_every_cycle() {
    let (mut e, t, id) = audio();
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 400})).unwrap();
    // The live render of all four cycles, stretched the way the process does.
    let live = render(&e, t, 400);
    let want = transit_dsp::offline::time_stretch(&[live], 2.0, 48_000.0).remove(0);
    e.execute("audiosuite.process", &json!({"process": "time_stretch", "clip": id.0, "params": {"ratio": 2.0}})).unwrap();
    let c = e.session().track(t).unwrap().clips()[0].clone();
    assert_eq!((c.looping, c.length), (None, 800));
    assert_eq!(render(&e, t, 800), want[..800].to_vec());
}

#[test]
fn a_loop_must_be_at_least_the_minimum_cycle() {
    let (mut e, _, id) = audio();
    assert!(e.execute("clip.set_loop", &json!({"clip": id.0, "start": 0, "end": 10})).is_err());
    assert!(e.execute("clip.set_loop", &json!({"clip": id.0, "start": 0, "end": 64})).is_ok());
}

#[test]
fn set_loop_checks_the_file_through_the_stretch() {
    let (mut e, _, id) = audio();
    e.session_mut().find_clip_mut(id).unwrap().stretch = 2.0;
    // Clip time 0..2000 reads 0..1000 of the 1000-frame file at stretch 2.
    assert!(e.execute("clip.set_loop", &json!({"clip": id.0, "start": 0, "end": 2000})).is_ok());
    assert!(e.execute("clip.set_loop", &json!({"clip": id.0, "start": 0, "end": 2100})).is_err());
}

#[test]
fn remove_pitch_shift_of_a_looped_stretched_clip_reads_the_file_at_the_stretch() {
    let (mut e, t, id) = audio();
    e.session_mut().find_clip_mut(id).unwrap().stretch = 2.0;
    // Clip time 0..100 is file 0..50 at stretch 2; loop it out to 800.
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 800})).unwrap();
    let before = render(&e, t, 800);
    let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    e.execute("clip.remove_pitch_shift", &json!({"clip": id.0})).unwrap();
    let c = e.session().track(t).unwrap().clips()[0].clone();
    assert_eq!(c.looping, None);
    let after = render(&e, t, c.length);
    // The source ramps to 0.05 inside a cycle's file range; reading past it (the old bug) reaches 0.1.
    assert!(peak(&after) < peak(&before) * 1.3, "{} vs {}", peak(&after), peak(&before));
}

/// A time-locked empty clip (when `with_lock`) and the looped clip of `midi()` at beat 8, retimed
/// from 120 to 240 bpm.
fn retime_rig(lock_beats: i64, with_lock: bool) -> (Session, TrackId, Samples) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let beat = s.tempo.tick_to_samples(Q, s.sample_rate);
    if with_lock {
        let id = s.new_clip_id();
        let mut a = Clip::midi(id, "lock", 0, lock_beats * beat, Sequence::default());
        a.time_locked = true;
        crate::edit::place_clip(&mut s, t, a);
    }
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 0));
    seq.notes.push(note(64, 2));
    let id = s.new_clip_id();
    let mut b = Clip::midi(id, "m", 8 * beat, 16 * beat, seq);
    b.looping = Some(transit_model::ClipLoop { start: 0, end: 4 * beat });
    crate::edit::place_clip(&mut s, t, b);
    let old = s.tempo.clone();
    s.tempo.set_tempo(0, 240.0).unwrap();
    crate::retime::retime_to_tempo(&mut s, &old).unwrap();
    (s, t, beat)
}

#[test]
fn retime_trims_a_looped_midi_clip_through_the_start_helper() {
    // At 240 bpm the looped clip starts at beat 8 and the time-locked clip ends at beat 10.
    let (s, t, beat) = retime_rig(5, true);
    let (r, rt, _) = retime_rig(5, false);
    let half = beat / 2;
    let ons = |s: &Session, t: TrackId| -> Vec<(f64, u8)> {
        transit_mix::clip_note_ons(s, t, 0, (40 * half) as usize).into_iter().map(|(a, k)| (a as f64 / half as f64, k)).collect()
    };
    let want: Vec<(f64, u8)> = ons(&r, rt).into_iter().filter(|(a, _)| *a >= 10.0).collect();
    assert_eq!(ons(&s, t), want);
}

fn neighbors(e: &Engine, t: TrackId) -> Vec<(Samples, Samples)> {
    e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end())).collect()
}

fn with_neighbors(e: &mut Engine, t: TrackId, spans: &[(Samples, Samples)]) {
    for &(a, b) in spans {
        let src = e.session().track(t).unwrap().clips()[0].source().unwrap();
        let id = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), t, Clip::audio(id, "n", src, 0, a, b - a));
    }
}

#[test]
fn a_loop_stops_at_the_next_clip_unless_it_overwrites() {
    let (mut e, t, id) = audio();
    with_neighbors(&mut e, t, &[(250, 350)]);
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 330})).unwrap();
    assert_eq!(neighbors(&e, t), vec![(0, 250), (250, 350)], "stopped at the neighbor");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(neighbors(&e, t), vec![(0, 100), (250, 350)]);
    // Overwriting: the loop reaches 330 and the neighbor keeps what is after it.
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 330, "overwrite": true})).unwrap();
    assert_eq!(neighbors(&e, t), vec![(0, 330), (330, 350)]);
    let c = e.session().find_clip(id).unwrap().1.clone();
    assert!(c.looping.is_some());
    // One undo step brings back the clip, the neighbor and the unlooped state.
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(neighbors(&e, t), vec![(0, 100), (250, 350)]);
    assert_eq!(e.session().find_clip(id).unwrap().1.looping, None);
}

#[test]
fn an_overwriting_loop_removes_covered_clips_and_stops_at_an_edit_locked_one() {
    let (mut e, t, id) = audio();
    with_neighbors(&mut e, t, &[(120, 180), (200, 260), (300, 360)]);
    let locked = e.session().track(t).unwrap().clips().iter().find(|c| c.start == 300).unwrap().id;
    e.session_mut().find_clip_mut(locked).unwrap().edit_locked = true;
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 340, "overwrite": true})).unwrap();
    assert_eq!(neighbors(&e, t), vec![(0, 300), (300, 360)], "covered clips gone, the locked clip untouched");
}

#[test]
fn an_overwriting_loop_does_not_delete_an_edit_locked_clip_that_abuts_it() {
    let (mut e, t, id) = audio();
    with_neighbors(&mut e, t, &[(100, 160)]);
    let locked = e.session().track(t).unwrap().clips().iter().find(|c| c.start == 100).unwrap().id;
    e.session_mut().find_clip_mut(locked).unwrap().edit_locked = true;
    e.execute("clip.loop_to", &json!({"clip": id.0, "to": 300, "overwrite": true})).unwrap();
    assert_eq!(neighbors(&e, t), vec![(0, 100), (100, 160)], "the locked clip is still there, untouched");
    assert!(e.session().find_clip(locked).is_some());
}

/// A MIDI clip whose loop cycle is 100 samples: a tempo change that makes the cycle shorter than
/// the 64-sample minimum is refused (K40, ADR 0019) instead of silently dropping the loop.
fn tiny_loop() -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "tiny", 0, 1000, Sequence::default());
    c.looping = Some(transit_model::ClipLoop { start: 0, end: 100 });
    crate::edit::place_clip(&mut s, t, c);
    (Engine::new(s), t, id)
}

#[test]
fn a_tempo_change_that_squeezes_a_loop_below_the_minimum_cycle_is_refused_and_rolls_back() {
    let (mut e, _, id) = tiny_loop();
    let err = e.execute("event.tempo", &json!({"bpm": 240.0})).unwrap_err().to_string();
    assert!(err.contains("loop") && err.contains("tiny") && err.contains("64"), "{err}");
    let c = e.session().find_clip(id).unwrap().1.clone();
    assert_eq!(c.looping, Some(transit_model::ClipLoop { start: 0, end: 100 }), "the loop is kept");
    assert_eq!((c.length, e.session().tempo.tempos()[0].bpm), (1000, 120.0), "nothing changed");
    // A change that leaves the cycle valid still retimes it: 120 -> 160 bpm makes 100 samples 75.
    e.execute("event.tempo", &json!({"bpm": 160.0})).unwrap();
    assert_eq!(e.session().find_clip(id).unwrap().1.looping, Some(transit_model::ClipLoop { start: 0, end: 75 }));
}

#[test]
fn opening_a_session_that_needs_an_unknown_feature_posts_one_warning() {
    let (e, _, _) = tiny_loop();
    let mut v: serde_json::Value = serde_json::from_str(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(v["needs"], json!(["clip_loop"]), "a looped session is marked");
    let mut r = Engine::new(Session::default());
    crate::io::open_session_bytes(&mut r, "a.transit", v.to_string().as_bytes()).unwrap();
    assert!(r.messages.iter().all(|m| !m.contains("does not know")), "known feature: no warning");
    v["needs"] = json!(["clip_loop", "from_the_future"]);
    crate::io::open_session_bytes(&mut r, "b.transit", v.to_string().as_bytes()).unwrap();
    let warned: Vec<_> = r.messages.iter().filter(|m| m.contains("from_the_future")).collect();
    assert_eq!(warned.len(), 1, "{:?}", r.messages);
    assert!(r.session().find_clip(ClipId(1)).is_some() || !r.session().tracks.is_empty(), "the session still opens");
}
