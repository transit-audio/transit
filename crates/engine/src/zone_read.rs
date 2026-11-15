//! Stored positions read at or after the session start (ADR 0052 decision 7). A stored selection,
//! insertion point or cycle range may start before the session start once a pre-count zone has
//! gone (its clips deleted or moved to bar 1 or later): a command that reads one floors it at the
//! session start. The stored value is never rewritten, so undo brings the zone back with the
//! positions meaning what they did. While a zone exists, a range in it is read as it is.

use transit_model::Session;
use transit_time::{Range, Samples};

/// `at`, or the session start when `at` is before it.
pub fn at_or_after_start(s: &Session, at: Samples) -> Samples {
    if at >= 0 { at } else { at.max(s.start()) }
}

/// `r` with its start floored at the session start; a range that ends at or before the session
/// start is empty there.
pub fn from_start(s: &Session, r: Range) -> Range {
    let start = at_or_after_start(s, r.start);
    Range { start, end: r.end.max(start) }
}

/// The edit selection (the insertion point when empty) as commands read it.
pub fn selection(s: &Session) -> Range {
    from_start(s, s.edit.selection)
}

/// The cycle range as Play, Cycle and Record read it while Cycle is on: floored at the session
/// start, `None` when Cycle is off or nothing of the range is left.
pub fn cycle(s: &Session) -> Option<Range> {
    let r = from_start(s, s.edit.cycle_range());
    (s.edit.loop_playback && !r.is_empty()).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use transit_midi::Sequence;
    use transit_model::{ChannelFormat, Clip, TrackKind};

    fn session(zone: Option<Samples>) -> Session {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        if let Some(at) = zone {
            let id = s.new_clip_id();
            crate::edit::place_clip(&mut s, t, Clip::midi(id, "zone", at, 1_000, Sequence::default()));
        }
        s
    }

    #[test]
    fn a_stored_range_before_the_session_start_is_floored_there_and_one_in_the_zone_is_kept() {
        let gone = session(None);
        assert_eq!(from_start(&gone, Range::new(-500, 300)), Range::new(0, 300));
        assert!(from_start(&gone, Range::new(-500, -100)).is_empty(), "wholly before: nothing left");
        assert_eq!(from_start(&gone, Range::new(100, 300)), Range::new(100, 300));
        let zone = session(Some(-1_000));
        assert_eq!(from_start(&zone, Range::new(-500, 300)), Range::new(-500, 300));
        assert_eq!(from_start(&zone, Range::new(-2_000, 300)), Range::new(-1_000, 300));
    }
}
