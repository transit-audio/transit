//! A clip sounds the points of its tick grid in `grid_start..grid_end` (#422): the grid end is the
//! clip end rounded to a tick, never past it. These tests pin what that means for ordinary clips
//! and what it costs on the audio thread.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use transit_midi::{Note, Sequence};
use transit_model::ClipId;
use transit_time::TICKS_PER_QUARTER as Q;

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

/// What a visit of the track delivers over `from..to` in blocks of `block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Ev {
    Off(Samples, u8),
    On(Samples, u8),
    Ctrl(Samples, u8),
}

fn events(s: &Session, t: TrackId, from: Samples, to: Samples, block: usize) -> Vec<Ev> {
    let mut out = Vec::new();
    let mut pos = from;
    while pos < to {
        let n = block.min((to - pos) as usize);
        visit_clip_events(s, s.track(t).unwrap(), pos, n, |at, e| {
            let at = pos + at as Samples;
            out.push(match e {
                ClipEvent::On(key, _, _) => Ev::On(at, key),
                ClipEvent::Off(key, _) => Ev::Off(at, key),
                ClipEvent::Control(m, _) => Ev::Ctrl(at, m[0] & 0xF0),
            });
        });
        pos += n as Samples;
    }
    out.sort_by_key(|e| match *e {
        Ev::Off(at, k) => (at, 0, k),
        Ev::On(at, k) => (at, 1, k),
        Ev::Ctrl(at, k) => (at, 2, k),
    });
    out
}

/// Every note-on has exactly one note-off at or after it, before the next on of its key.
fn paired(ev: &[Ev]) -> Result<(), String> {
    let mut held = [None::<Samples>; 128];
    for e in ev {
        match *e {
            Ev::On(at, k) => {
                if let Some(on) = held[usize::from(k)].replace(at) {
                    return Err(format!("key {k}: on at {at} while the on at {on} is still sounding"));
                }
            }
            Ev::Off(at, k) => match held[usize::from(k)].take() {
                Some(on) if on <= at => {}
                other => return Err(format!("key {k}: off at {at} for {other:?}")),
            },
            Ev::Ctrl(..) => {}
        }
    }
    match held.iter().position(Option::is_some) {
        Some(k) => Err(format!("key {k} left sounding")),
        None => Ok(()),
    }
}

fn session(hz: u32, bpm: f64) -> (Session, TrackId) {
    let mut s = Session {
        sample_rate: transit_time::SampleRate::new(hz).unwrap(),
        tempo: transit_time::TempoMap::new(bpm, 4, 4).unwrap(),
        ..Session::default()
    };
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    (s, t)
}

fn bend(tick: i64) -> transit_midi::CtrlEvent {
    transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::PitchBend, value: 100 }
}

/// An ordinary unlooped clip whose end falls between two ticks: what sits on the tick the end
/// rounds DOWN to (at most half a tick before the end) is the next clip's, from its grid start, so
/// this clip does not sound a note-on or a controller there, and a note crossing the end ends on
/// that tick. An end on a tick, or one that rounds up, sounds everything before the end and ends a
/// crossing note at the end, as before the grid end. Every note-on has one note-off, at block sizes
/// 1, 37 and one block. And two clips butted at any of those ends share the ticks: every tick in a
/// window sounds exactly once, its note and its controller, none doubled or dropped.
#[test]
fn an_unlooped_clip_ending_between_ticks_sounds_its_grid_points_once() {
    let mut cases = 0;
    for (hz, bpm) in [(48_000u32, 102.5), (44_100, 93.7), (48_000, 120.0)] {
        let (s0, t) = session(hz, bpm);
        let sr = s0.sample_rate;
        let at = |tick: i64| s0.tempo.tick_to_samples(tick, sr);
        let r = 4 * Q;
        for e in at(r)..=at(r + 1) {
            cases += 1;
            let rounds_down = e > at(r) && s0.tempo.samples_to_ticks(e, sr) == r;
            // One clip, 0..e: a note and a bend on tick r, a note crossing the end.
            let mut s = s0.clone();
            let mut seq = Sequence::default();
            seq.notes.push(note(60, r, 10));
            seq.notes.push(note(61, r - 20, 100));
            seq.ctrls.push(bend(r));
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1), "a", 0, e, seq));
            let from = at(r - 40);
            let one = events(&s, t, from, e + 64, 1 << 20);
            for block in [1, 37] {
                assert_eq!(events(&s, t, from, e + 64, block), one, "{hz} Hz {bpm} BPM, end {e}: block {block}");
            }
            paired(&one).unwrap_or_else(|m| panic!("{hz} Hz {bpm} BPM, end {e}: {m}: {one:?}"));
            let sounds_r = e > at(r) && !rounds_down;
            let on60 = one.contains(&Ev::On(at(r), 60));
            let bent = one.contains(&Ev::Ctrl(at(r), 0xE0));
            let off61 = one.iter().find_map(|x| if let Ev::Off(a, 61) = x { Some(*a) } else { None });
            let crossing_ends = if rounds_down { at(r) } else { e };
            assert_eq!(
                (on60, bent, off61),
                (sounds_r, sounds_r, Some(crossing_ends)),
                "{hz} Hz {bpm} BPM, end {e} (tick {} at {}, next at {}): {one:?}",
                r,
                at(r),
                at(r + 1)
            );
            // Two clips butted at e: a note and a bend on each tick of A from r - 3 to r + 1 and on
            // each of B's first four ticks (from its grid start).
            if e == at(r) || e == at(r + 1) {
                continue;
            }
            let mut s = s0.clone();
            let (mut a, mut b) = (Sequence::default(), Sequence::default());
            for k in r - 3..=r + 1 {
                a.notes.push(note(60, k, 1));
                a.ctrls.push(bend(k));
            }
            for k in 0..4 {
                b.notes.push(note(60, k, 1));
                b.ctrls.push(bend(k));
            }
            let pl = s.track_mut(t).unwrap().playlist_mut().unwrap();
            pl.clips.push(Clip::midi(ClipId(1), "a", 0, e, a));
            pl.clips.push(Clip::midi(ClipId(2), "b", e, at(r + 8) - e, b));
            let base = s.tempo.samples_to_ticks(e, sr);
            let (from, to) = (at(r - 8), at(r + 8));
            let both = events(&s, t, from, to, 1 << 20);
            for block in [1, 37] {
                assert_eq!(events(&s, t, from, to, block), both, "{hz} Hz {bpm} BPM, butted at {e}: block {block}");
            }
            paired(&both).unwrap_or_else(|m| panic!("{hz} Hz {bpm} BPM, butted at {e}: {m}: {both:?}"));
            let ons: Vec<Samples> = both.iter().filter_map(|x| if let Ev::On(a, _) = x { Some(*a) } else { None }).collect();
            let bends: Vec<Samples> = both.iter().filter_map(|x| if let Ev::Ctrl(a, _) = x { Some(*a) } else { None }).collect();
            let want: Vec<Samples> = (r - 3..=base + 3).map(at).collect();
            assert_eq!((&ons, &bends), (&want, &want), "{hz} Hz {bpm} BPM, butted at {e} (B's grid from tick {base})");
        }
    }
    assert!(cases > 60, "{cases} ends");
}

/// The grid end is loop-invariant per clip: a visit takes it once per clip per pass (two passes,
/// offs then ons), never once per note, so a dense clip costs per note what it did before the grid
/// end existed. Counted, not timed.
#[test]
fn a_visit_takes_each_clips_grid_end_once_per_pass_not_per_note() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let sr = s.sample_rate;
    for (i, start_beat) in [0i64, 400].into_iter().enumerate() {
        let mut seq = Sequence::default();
        for k in 0..1000 {
            seq.notes.push(note(36 + (k % 48) as u8, k * 48, 40));
        }
        let start = s.tempo.tick_to_samples(start_beat * Q, sr);
        // An end between ticks, so the grid end is not the clip end.
        let len = s.tempo.tick_to_samples(1000 * 48, sr) + 7;
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1 + i as u64), "dense", start, len, seq));
    }
    let tr = s.track(t).unwrap();
    // A block inside each clip, and one across the first clip's end.
    let first_end = tr.clips()[0].end();
    let second = tr.clips()[1].start;
    for (pos, frames, clips) in [(1000, 64usize, 1usize), (second + 1000, 64, 1), (first_end - 32, 64, 1)] {
        GRID_ENDS.with(|n| n.set(0));
        let mut ons = 0;
        visit_clip_events(&s, tr, pos, frames, |_, e| ons += usize::from(matches!(e, ClipEvent::On(..))));
        let calls = GRID_ENDS.with(|n| n.get());
        assert!(calls <= 2 * clips, "block at {pos}: the grid end taken {calls} times for {clips} clip(s) of 1,000 notes ({ons} ons)");
    }
}
