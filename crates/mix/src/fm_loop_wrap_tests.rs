//! #263: the Macro Oscillator on "Six-op FM 1" (8 voices, factory defaults otherwise) on a rack
//! member, replaying a one-beat note through two laps of a transport cycle.
//!
//! What the mix delivers at the wrap is not the cause: the wrap releases the clip's note and
//! chases the controllers at the block start (note-off, CC 121, then the clip's note-on, all at
//! offset 0), and the instrument renders exactly what it renders for the same notes played
//! straight through, or from live input with the off and the on at one block start. The
//! excursion is the instrument's: a note-off and note-on of one key within one 12-frame block
//! retrigger the same voice with its low-pass gate open, and the six-op engine starts the new note
//! on its second internal voice while the first plays the old note's release (this patch's
//! modulators hold at release level 99) at the same pitch. The two sum with a phase offset the
//! loop length fixes, so every other lap the fundamental cancels for about 90 ms.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::PlanRig;
use super::*;
use transit_midi::{Note, Sequence};
use transit_model::{Insert, Rack, RackMember};

const BLOCK: usize = 256;
const KEY: u8 = 60;

/// A rack whose one member plays the Macro Oscillator on Six-op FM 1, with a clip of
/// `clip_beats` beats at 0 holding key 60 at `notes` ((start, length) in beats; none: no clip).
fn rack_session(notes: &[(i64, i64)], clip_beats: i64) -> (Session, TrackId, Samples) {
    let mut s = Session::default();
    let folder = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, None);
    let member = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let beat = s.tempo.tick_to_samples(TICKS_PER_QUARTER, s.sample_rate);
    let mut seq = Sequence::default();
    for &(start, len) in notes {
        seq.notes.push(Note {
            pitch: KEY,
            velocity: 100,
            release_velocity: 64,
            channel: 0,
            start: start * TICKS_PER_QUARTER,
            length: len * TICKS_PER_QUARTER,
        });
    }
    let cid = s.new_clip_id();
    let t = s.track_mut(member).unwrap();
    t.folder = Some(folder);
    if clip_beats > 0 {
        t.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, beat * clip_beats, seq));
    }
    let mut ins = Insert::new("macro_oscillator");
    ins.params.insert("model".into(), 2.0);
    t.instrument = Some(ins);
    s.track_mut(folder).unwrap().rack = Some(Rack { members: vec![RackMember::new(member)], ..Rack::default() });
    (s, member, beat)
}

/// Renders the timeline segments `(from, frames)` in order in blocks of at most [`BLOCK`], each
/// segment cut at its end as playback cuts a block at the loop end, calling `before(rig, pos)`
/// ahead of every block. The left channel of Main.
fn render(s: &Session, segments: &[(i64, usize)], mut before: impl FnMut(&mut PlanRig, i64)) -> Vec<f32> {
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(s);
    let mut out = Vec::new();
    for &(from, frames) in segments {
        let mut done = 0;
        while done < frames {
            let n = BLOCK.min(frames - done);
            let pos = from + done as i64;
            before(&mut rig, pos);
            let mut buf = vec![vec![0.0f32; n]; rig.eng.main_channels().max(2)];
            rig.eng.render_active(pos, n, &mut buf);
            out.extend_from_slice(&buf[0]);
            done += n;
        }
    }
    out
}

/// Goertzel magnitude of `x` (Hann window) at `f` Hz, at 48 kHz.
fn goertzel(x: &[f32], f: f64) -> f64 {
    let n = x.len() as f64;
    let w = 2.0 * std::f64::consts::PI * f / 48_000.0;
    let (c, mut s1, mut s2) = (2.0 * w.cos(), 0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
        let s0 = f64::from(*v) * win + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0).sqrt()
}

/// The pitch track of the 300 ms from `at`, one 2048-frame window per 10 ms: the strongest of the
/// first eight partials of key 60 (1 is the fundamental) and the fundamental's pitch in cents
/// (the strongest Goertzel bin within 100 cents, in 1-cent steps).
fn track(x: &[f32], at: usize) -> Vec<(usize, i32)> {
    let f0 = 440.0 * 2f64.powf((f64::from(KEY) - 69.0) / 12.0);
    let cents = |c: i32| f0 * 2f64.powf(f64::from(c) / 1200.0);
    (0..30)
        .map(|k| {
            let w = &x[at + k * 480..at + k * 480 + 2048];
            let h = (1..=8usize).max_by(|a, b| goertzel(w, f0 * *a as f64).total_cmp(&goertzel(w, f0 * *b as f64))).unwrap_or(1);
            let c = (-100..=100).max_by(|a, b| goertzel(w, cents(*a)).total_cmp(&goertzel(w, cents(*b)))).unwrap_or(0);
            (h, c)
        })
        .collect()
}

/// The note on its first lap, and the three ways of replaying it (the transport cycle with the
/// note ending at the loop end, the same with the note crossing it, the notes laid end to end)
/// plus live input with the off and the on at the same block start: three beats each.
fn renders() -> (usize, [Vec<f32>; 4]) {
    let (s, _, beat) = rack_session(&[(0, 1)], 1);
    let l = usize::try_from(beat).unwrap();
    let wrap = render(&s, &[(0, l), (0, l), (0, l)], |_, _| {});
    let (s, _, _) = rack_session(&[(0, 2)], 2);
    let cross = render(&s, &[(0, l), (0, l), (0, l)], |_, _| {});
    let (s, _, _) = rack_session(&[(0, 1), (1, 1), (2, 1)], 3);
    let straight = render(&s, &[(0, 3 * l)], |_, _| {});
    let (s, member, _) = rack_session(&[], 0);
    let live = render(&s, &[(0, l), (beat, l), (2 * beat, l)], |rig, pos| {
        if pos % beat == 0 {
            if pos > 0 {
                rig.eng.live_note(member, KEY, 0);
            }
            rig.eng.live_note(member, KEY, 100);
        }
    });
    (l, [wrap, cross, straight, live])
}

/// The loop wrap, a note crossing it, straight-through playback and live input give the
/// instrument the same events at the same offsets: the outputs are the same bits.
#[test]
fn a_loop_wrap_plays_a_rack_member_as_straight_through_playback_and_live_input_do() {
    let (l, [wrap, cross, straight, live]) = renders();
    assert!(wrap.iter().any(|x| x.abs() > 0.1), "the note sounds");
    let first_difference = |a: &[f32]| a.iter().zip(&straight).position(|(x, y)| x.to_bits() != y.to_bits());
    assert_eq!(wrap.len(), 3 * l);
    assert_eq!(first_difference(&wrap), None, "the transport cycle");
    assert_eq!(first_difference(&cross), None, "a note crossing the loop end");
    assert_eq!(first_difference(&live), None, "live input");
}

/// #263: every lap should keep the pitch the first lap has (its fundamental leads and stays within
/// the patch's vibrato, 23 cents). Measured at f2498a35: the second lap's third partial leads 70
/// to 100 ms after the wrap and its second partial 100 to 160 ms (9 of 30 windows), and the
/// fundamental reads down to -90 cents; the third lap is clean.
#[test]
#[ignore = "#263: the six-op engine's second internal voice plays the old note's release under a same-key retrigger; the fix needs a design decision"]
fn every_lap_of_a_six_op_fm_loop_keeps_the_first_laps_pitch() {
    let (l, [wrap, ..]) = renders();
    for lap in 0..3 {
        let t = track(&wrap, lap * l);
        let led_by_overtones = t.iter().filter(|(h, _)| *h != 1).count();
        let worst = t.iter().map(|(_, c)| c.abs()).max().unwrap_or(0);
        assert!(
            led_by_overtones == 0 && worst <= 50,
            "lap {}: overtones lead {led_by_overtones}/30 windows, fundamental off by up to {worst} cents: {t:?}",
            lap + 1
        );
    }
}
