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
