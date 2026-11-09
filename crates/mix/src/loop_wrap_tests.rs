//! #422: a looped MIDI clip under the arrangement loop. A one-bar kick clip, looped and dragged out
//! to the loop end, at a tempo whose bar is not a whole number of samples (102.5 BPM at 48 kHz: a
//! bar is 112,390.24 samples). The clip loop's end is stored rounded (112,390), the clip length and
//! the arrangement loop end are the eight-bar tick converted once (899,122). Every repeat of the
//! clip loop must land where its tick lands, so the kick that begins the ninth repeat sits on the
//! loop end and never fires inside the pass, and the kick at the loop start fires once per pass.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::PlanRig;
use super::*;
use std::sync::Mutex;
use transit_audio_io::AudioBuffer;
use transit_midi::{Note, Sequence};
use transit_model::{ClipId, Insert, SourceAudio, SourceId};
use transit_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};

const KICK: u8 = 36;
const BAR: i64 = 4 * TICKS_PER_QUARTER;

/// What the counting instrument heard: an output frame (frames processed before the event's block
/// plus its offset), and the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heard {
    On(u64, u8),
    Off(u64, u8),
    AllOff(u64),
}

/// An instrument that records every note event with the output frame it was given at.
struct Counter {
    inner: Box<dyn Plugin>,
    frame: u64,
    heard: Arc<Mutex<Vec<Heard>>>,
}

impl Counter {
    fn new(heard: &Arc<Mutex<Vec<Heard>>>) -> Self {
        Counter { inner: transit_dsp::create("gain").unwrap(), frame: 0, heard: Arc::clone(heard) }
    }
}

impl Plugin for Counter {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
        self.frame += frames as u64;
    }
    fn note_on(&mut self, offset: usize, note: u8, _velocity: u8) {
        self.heard.lock().unwrap().push(Heard::On(self.frame + offset as u64, note));
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.heard.lock().unwrap().push(Heard::Off(self.frame + offset as u64, note));
    }
    fn all_notes_off(&mut self) {
        self.heard.lock().unwrap().push(Heard::AllOff(self.frame));
    }
}

/// A kick on every beat of a one-bar clip, looped and dragged out to `clip_bars`, under an
/// arrangement loop of `loop_bars` from 0, at `hz` and `bpm` in 4/4. The clip loop end, the clip
/// length and the loop end are stored as the editor stores them: each a tick converted to samples.
/// Returns the session, the track and the loop end (samples).
fn kick_session(hz: u32, bpm: f64, loop_bars: i64, clip_bars: i64) -> (Session, TrackId, Samples) {
    let mut s = Session { sample_rate: SampleRate::new(hz).unwrap(), tempo: TempoMap::new(bpm, 4, 4).unwrap(), ..Session::default() };
    let sr = s.sample_rate;
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    for beat in 0..4 {
        seq.notes.push(Note { pitch: KICK, velocity: 127, release_velocity: 64, channel: 0, start: beat * TICKS_PER_QUARTER, length: 240 });
    }
    let mut c = Clip::midi(ClipId(1), "kick", 0, s.tempo.tick_to_samples(clip_bars * BAR, sr), seq);
    c.set_loop(Some(ClipLoop { start: 0, end: s.tempo.tick_to_samples(BAR, sr) }));
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(c);
    tr.instrument = Some(Insert::new("clap:test.counter"));
    let end = s.tempo.tick_to_samples(loop_bars * BAR, sr);
    s.edit.loop_playback = true;
    s.edit.loop_range = Range::new(0, end);
    (s, t, end)
}

/// The kicks one pass should play: every beat of `bars`, at its tick's sample.
fn grid_kicks(s: &Session, bars: i64) -> Vec<Samples> {
    (0..bars * 4).map(|b| s.tempo.tick_to_samples(b * TICKS_PER_QUARTER, s.sample_rate)).collect()
}

/// The note-ons of one pass of the loop, visited the way the engine visits a track's clips, in
/// blocks of `block` (the last cut at the loop end, as playback cuts it).
fn pass_ons(s: &Session, t: TrackId, end: Samples, block: usize) -> Vec<Samples> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < end {
        let n = block.min((end - pos) as usize);
        visit_clip_events(s, s.track(t).unwrap(), pos, n, |at, e| {
            if let ClipEvent::On(KICK, _, _) = e {
                out.push(pos + at as Samples);
            }
        });
        pos += n as Samples;
    }
    out.sort_unstable();
    out
}

/// Plays `passes` passes of the loop `0..end` through the engine in blocks of at most `block`,
/// each pass cut at the loop end the way the player cuts it. What the counting instrument heard,
/// with output frames.
fn engine_passes(s: &Session, t: TrackId, end: Samples, block: usize, passes: usize) -> Vec<Heard> {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let mut rig = PlanRig::new(block).fake(t, INSTRUMENT_SLOT, Counter::new(&heard));
    rig.publish(s);
    let mut buf = vec![vec![0.0f32; block]; rig.eng.main_channels().max(2)];
    for _ in 0..passes {
        let mut pos = 0;
        while pos < end {
            let n = block.min((end - pos) as usize);
            rig.eng.render_active(pos, n, &mut buf);
            pos += n as Samples;
        }
    }
    heard.lock().unwrap().clone()
}

/// The maintainer's numbers: 48 kHz, 102.5 BPM, an eight-bar loop of 899,122 samples and the kick
/// clip's stored loop end of 112,390. Four passes through the engine, as the player cuts them: one
/// kick per beat per pass, each at its tick's sample, none at or after the loop end.
#[test]
fn the_kick_at_the_loop_start_fires_once_per_pass_with_the_sessions_numbers() {
    let (s, t, end) = kick_session(48_000, 102.5, 8, 8);
    assert_eq!(end, 899_122);
    let clip = &s.track(t).unwrap().clips()[0];
    assert_eq!((clip.length, clip.looping), (899_122, Some(ClipLoop { start: 0, end: 112_390 })));
    let want: Vec<u64> = (0..4u64).flat_map(|p| grid_kicks(&s, 8).into_iter().map(move |k| p * end as u64 + k as u64)).collect();
    for block in [512, 4093] {
        let heard = engine_passes(&s, t, end, block, 4);
        let ons: Vec<u64> = heard.iter().filter_map(|h| if let Heard::On(at, KICK) = h { Some(*at) } else { None }).collect();
        assert_eq!(ons, want, "block {block}: every note-on heard: {heard:?}");
    }
}

/// The right half of a split saved by a build that took the cut's cycle phase as a sample modulo of
/// the stored range: 44.1 kHz, 102.5 BPM (a bar is 103,258.54 samples, stored 103,259), the one-bar
/// kick loop dragged to eight bars and split at the start of bar 3 (sample 206,517). The phase was
/// 206,517 mod 103,259 = 103,258, so the right half's loop is `{-103,258, 1}` and its notes moved
/// 3,840 ticks earlier. The loop end snaps to the right half's grid origin (an empty first pass),
/// but its cycle is a whole bar of ticks: the frame stays on the grid and every repeat of the
/// right half sits on its tick.
#[test]
fn a_right_half_whose_loop_ends_at_its_grid_origin_repeats_on_its_ticks() {
    let (mut s, t, end) = kick_session(44_100, 102.5, 8, 8);
    let sr = s.sample_rate;
    let cut = s.tempo.tick_to_samples(2 * BAR, sr);
    assert_eq!((s.tempo.tick_to_samples(BAR, sr), cut), (103_259, 206_517));
    let phase = cut % 103_259;
    let shift = s.tempo.samples_to_ticks(phase, sr);
    assert_eq!((phase, shift), (103_258, BAR));
    let clip = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
    clip.start = cut;
    clip.length = end - cut;
    clip.looping = Some(ClipLoop { start: -phase, end: 1 });
    let ClipContent::Midi { sequence } = &mut clip.content else { panic!("a MIDI clip") };
    sequence.notes.iter_mut().for_each(|n| n.start -= shift);
    let want: Vec<Samples> = grid_kicks(&s, 8).split_off(8);
    for block in [512, 1 << 20] {
        assert_eq!(pass_ons(&s, t, end, block), want, "block {block}");
    }
}

/// Block size does not matter: the same pass at block sizes 1, 32, 128 and 512, and through the
/// engine at a size that does not divide the loop (the last block of every pass is cut at the wrap).
#[test]
fn a_pass_plays_the_same_kicks_at_any_block_size() {
    let (s, t, end) = kick_session(48_000, 102.5, 8, 8);
    let want = grid_kicks(&s, 8);
    for block in [1, 32, 128, 512] {
        assert_eq!(pass_ons(&s, t, end, block), want, "block {block}");
    }
    let heard = engine_passes(&s, t, end, 1000, 2);
    let ons = heard.iter().filter(|h| matches!(h, Heard::On(_, KICK))).count();
    assert_eq!(ons, 2 * want.len(), "{heard:?}");
}

/// Loop lengths 1 to 16 bars at 60, 90, 102.5, 120, 127.3, 140 and 174 BPM, at 44.1 and 48 kHz, the
/// clip ending at the loop end or dragged two bars past it: one kick per beat per pass, at its tick.
#[test]
fn every_tempo_rate_and_length_plays_one_kick_per_beat_per_pass() {
    let mut bad = Vec::new();
    for hz in [44_100, 48_000] {
        for bpm in [60.0, 90.0, 102.5, 120.0, 127.3, 140.0, 174.0] {
            for bars in 1..=16 {
                for past in [0, 2] {
                    let (s, t, end) = kick_session(hz, bpm, bars, bars + past);
                    let got = pass_ons(&s, t, end, 1 << 20);
                    let want = grid_kicks(&s, bars);
                    if got != want {
                        let late: Vec<Samples> = got.iter().copied().filter(|g| !want.contains(g)).collect();
                        bad.push(format!("{hz} Hz {bpm} BPM {bars} bars (+{past}): {} kicks, off-grid {late:?}, loop end {end}", got.len()));
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} cases:\n{}", bad.len(), bad.join("\n"));
}

/// A note that crosses the loop end (an unlooped clip running past it) gets exactly one note-off at
/// or before each wrap, and nothing is left sounding.
#[test]
fn a_note_over_the_wrap_is_released_once_at_the_wrap() {
    let (mut s, t, end) = kick_session(48_000, 102.5, 2, 2);
    let four = s.tempo.tick_to_samples(4 * BAR, s.sample_rate);
    let clip = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
    clip.looping = None;
    clip.length = four;
    let ClipContent::Midi { sequence } = &mut clip.content else { panic!("a MIDI clip") };
    // A long note from the last beat of bar 2 over the loop end.
    sequence.notes.push(Note {
        pitch: 50,
        velocity: 100,
        release_velocity: 64,
        channel: 0,
        start: 7 * TICKS_PER_QUARTER,
        length: 2 * TICKS_PER_QUARTER,
    });
    let heard = engine_passes(&s, t, end, 512, 3);
    let on = s.tempo.tick_to_samples(7 * TICKS_PER_QUARTER, s.sample_rate) as u64;
    let long: Vec<Heard> = heard.iter().copied().filter(|h| matches!(h, Heard::On(_, 50) | Heard::Off(_, 50))).collect();
    let e = end as u64;
    assert_eq!(
        long,
        vec![Heard::On(on, 50), Heard::Off(e, 50), Heard::On(e + on, 50), Heard::Off(2 * e, 50), Heard::On(2 * e + on, 50)],
        "{heard:?}"
    );
    let (ons, offs) = heard.iter().fold((0, 0), |(a, b), h| match h {
        Heard::On(..) => (a + 1, b),
        Heard::Off(..) => (a, b + 1),
        Heard::AllOff(_) => (a, b),
    });
    // The third pass's notes are the only ones still open: its long note and nothing else (the
    // kicks' offs are in the clip).
    assert_eq!(ons - offs, 1, "{heard:?}");
}

/// Consolidating the looped kick writes one note per beat of the clip, each on its tick: nothing
/// on the clip's last tick.
#[test]
fn consolidating_the_looped_kick_writes_one_note_per_beat_on_its_tick() {
    let (s, t, _) = kick_session(48_000, 102.5, 8, 8);
    let seq = unroll_midi_loop(&s, &s.track(t).unwrap().clips()[0]).unwrap();
    let starts: Vec<i64> = seq.notes.iter().map(|n| n.start).collect();
    assert_eq!(starts, (0..32).map(|b| b * TICKS_PER_QUARTER).collect::<Vec<_>>());
}

/// The audio sibling with the session's numbers: a four-bar loop file of 449,561 frames (four bars
/// at 102.5 BPM is 449,560.98), looped over the clip to the 899,122-sample loop end, and the same
/// file as two clips laid end to end. The second cycle (or clip) starts at 449,561, the sample the
/// fifth bar's tick converts to; the last sample of the pass is the file's last; nothing overlaps,
/// nothing is skipped, nothing plays past the loop end.
#[test]
fn a_four_bar_audio_loop_one_sample_over_the_grid_plays_end_to_end_to_the_wrap() {
    const FRAMES: i64 = 449_561;
    let mut s = Session { tempo: TempoMap::new(102.5, 4, 4).unwrap(), ..Session::default() };
    assert_eq!(s.tempo.tick_to_samples(4 * BAR, s.sample_rate), FRAMES);
    let end = s.tempo.tick_to_samples(8 * BAR, s.sample_rate);
    let data: Vec<f32> = (0..FRAMES).map(|i| (i + 1) as f32).collect();
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![data] })));
    let mut looped = Clip::audio(ClipId(1), "loop", SourceId(1), 0, 0, end);
    looped.set_loop(Some(ClipLoop { start: 0, end: FRAMES }));
    let first = Clip::audio(ClipId(2), "a", SourceId(1), 0, 0, FRAMES);
    let second = Clip::audio(ClipId(3), "b", SourceId(1), 0, FRAMES, FRAMES);
    let want: Vec<f32> = (0..end).map(|i| (i % FRAMES + 1) as f32).collect();
    let pass = |clips: &[&Clip]| {
        let mut got = vec![0.0f32; end as usize + 64];
        let mut pos = 0usize;
        while pos < got.len() {
            let n = 4093.min(got.len() - pos);
            for c in clips {
                let mut buf = vec![vec![0.0f32; n]];
                render_clip(&s, c, pos as i64, n, &mut buf);
                for (g, b) in got[pos..pos + n].iter_mut().zip(&buf[0]) {
                    *g += *b;
                }
            }
            pos += n;
        }
        got
    };
    for (name, got) in [("looped", pass(&[&looped])), ("two clips", pass(&[&first, &second]))] {
        assert_eq!(&got[..end as usize], &want[..], "{name}");
        assert!(got[end as usize..].iter().all(|v| *v == 0.0), "{name}: nothing past the loop end");
    }
}

/// The other users of the cycle rule at the session's numbers: a controller in the looped clip
/// repeats on its tick, a note as long as the cycle ends on the sample the next cycle's note
/// starts (never after it, which would cut the new note), and the chase finds the latest repeat
/// before a position on its tick.
#[test]
fn controllers_cycle_long_notes_and_the_chase_follow_the_tick_grid() {
    let (mut s, t, end) = kick_session(48_000, 102.5, 8, 8);
    let clip = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
    let ClipContent::Midi { sequence } = &mut clip.content else { panic!("a MIDI clip") };
    sequence.notes = vec![Note { pitch: 40, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: BAR }];
    sequence.ctrls.push(transit_midi::CtrlEvent { tick: 2 * TICKS_PER_QUARTER, channel: 0, kind: transit_midi::CtrlKind::Cc(1), value: 90 });
    let sr = s.sample_rate;
    let at = |tick: i64| s.tempo.tick_to_samples(tick, sr);
    let mut ons = Vec::new();
    let mut offs = Vec::new();
    let mut ccs = Vec::new();
    visit_clip_events(&s, s.track(t).unwrap(), 0, end as usize, |o, e| match e {
        ClipEvent::On(..) => ons.push(o as Samples),
        ClipEvent::Off(..) => offs.push(o as Samples),
        ClipEvent::Control(..) => ccs.push(o as Samples),
    });
    ons.sort_unstable();
    offs.sort_unstable();
    ccs.sort_unstable();
    assert_eq!(ons, (0..8).map(|k| at(k * BAR)).collect::<Vec<_>>());
    // Each off on the next cycle's first sample; the last is the clip end (outside the pass).
    assert_eq!(offs, (1..8).map(|k| at(k * BAR)).collect::<Vec<_>>());
    assert_eq!(ccs, (0..8).map(|k| at(k * BAR + 2 * TICKS_PER_QUARTER)).collect::<Vec<_>>());
    let frame = midi_loop_frame(&s, &s.track(t).unwrap().clips()[0]).unwrap();
    let rel = at(2 * TICKS_PER_QUARTER);
    assert_eq!(frame.latest_before(&s, rel, end), Some(at(7 * BAR + 2 * TICKS_PER_QUARTER)));
    assert_eq!(frame.latest_before(&s, rel, at(7 * BAR + 2 * TICKS_PER_QUARTER)), Some(at(6 * BAR + 2 * TICKS_PER_QUARTER)));
    assert_eq!(frame.latest_before(&s, rel, at(BAR + 2 * TICKS_PER_QUARTER)), None, "the first repeat is not before it");
}
