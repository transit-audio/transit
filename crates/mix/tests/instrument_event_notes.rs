#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_mix::{Due, EventLimits, EventOutcome, InstrumentEvent, MAX_OPEN, Message, PluginEvent, RenderFrame, Shed, TransportEpoch};
mod common;
use common::*;
#[path = "common/voice_ids.rs"]
mod voice_ids;
use voice_ids::*;
#[path = "common/voice_releases.rs"]
mod voice_releases;
use voice_releases::*;
#[path = "common/pedal_edges.rs"]
mod pedal_edges;
use pedal_edges::*;
#[path = "common/pedal_values.rs"]
mod pedal_values;
use pedal_values::*;

#[test]
fn a_note_off_survives_an_overfull_queue() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let mut admitted = 0;
    // Controllers that stay queued, then three rounds of note-on/off pairs whose offs are due
    // far ahead (future pairs), one block rendered between rounds. Without the reservation the
    // offs of the first round alone would not fit.
    let now = rig.mix.next_frame().0;
    for i in 0..120u64 {
        assert_eq!(rig.send(src, now + 7000 + i, cc(1, 1)), EventOutcome::Queued);
    }
    for round in 0..3u64 {
        let now = rig.mix.next_frame().0;
        let mut ons = Vec::new();
        for i in 0..300u32 {
            let (key, id) = ((i % 128) as u8, round as u32 * 1000 + i);
            let o = rig.send(src, now + u64::from(i % 50), on_id(key, id));
            assert!(matches!(o, EventOutcome::Queued | EventOutcome::Refused(Shed::QueueFull | Shed::NotesFull)), "{o:?}");
            ons.push((key, id, o == EventOutcome::Queued));
        }
        for (key, id, queued) in ons {
            let o = rig.send(src, now + 8000, off_id(key, id));
            if queued {
                admitted += 1;
                assert_eq!(o, EventOutcome::Queued, "an admitted note's off is always queued");
            } else {
                assert_eq!(o, EventOutcome::Unmatched);
            }
        }
        rig.render(1);
    }
    rig.render(40);
    let st = rig.mix.event_status();
    assert_eq!(st.dropped_releases, 0);
    assert_eq!(st.release_collapses, 0);
    assert_eq!(st.host_refused, [0, 0]);
    assert_eq!(admitted, MAX_OPEN, "the queue, then the table filled");
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), admitted, "every admitted note-on was delivered, and released");
}

#[test]
fn duplicate_and_early_releases() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    let limit = u64::from(EventLimits::default().max_future_frames);
    assert_eq!(rig.send(src, now + limit, on(60)), EventOutcome::Queued, "at the horizon");
    assert_eq!(rig.send(src, now + limit + 500, off(60)), EventOutcome::ReleaseAdvanced { frames: 500 });
    assert_eq!(rig.send(src, now + limit + 600, off(60)), EventOutcome::Coalesced, "a duplicate takes no slot");
    assert_eq!(rig.send(src, now, off(61)), EventOutcome::Unmatched);
    rig.render(40);
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 1);
    assert_eq!(heard.len(), 2);
    assert_eq!(heard[0].0, now + limit);
    // Advanced to the horizon, onto its on's frame; delivered one sample after it (#257: no
    // zero-length note, live or overdue).
    assert_eq!(heard[1].0, now + limit + 1, "advanced to the horizon, never before its on, never on it");
    let st = rig.mix.event_status();
    assert_eq!((st.releases_advanced, st.coalesced, st.unmatched), (1, 1, 1));
}

#[test]
fn a_release_due_before_its_note_on_is_ordered_after_it() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(src, now + 100, on(60)), EventOutcome::Queued);
    assert_eq!(rig.send(src, now + 50, off(60)), EventOutcome::Queued);
    rig.render(1);
    let heard = rig.heard_by(1);
    assert_eq!(heard.len(), 2);
    assert!(matches!(heard[0].1.message, Message::NoteOn { .. }));
    // Moved onto its on's frame at admission, delivered one sample after it (#257).
    assert_eq!(heard[1].0, now + 101);
    assert_eq!(rig.mix.event_status().reordered, 1);
}

#[test]
fn the_horizon_refuses_early_notes_and_late_ones_play_at_offset_0() {
    let mut rig = Rig::new();
    rig.render(4);
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    let limit = u64::from(EventLimits::default().max_future_frames);
    assert_eq!(rig.send(src, now + limit + 1, on(60)), EventOutcome::TooEarly);
    assert_eq!(rig.send(src, now - 300, on(61)), EventOutcome::QueuedLate { frames: 300 });
    assert_eq!(rig.mix.queue_event(&rig.event(src, Due::Next, on(62))), EventOutcome::Queued);
    rig.render(1);
    let heard = rig.heard_by(1);
    assert_eq!(heard.iter().map(|(f, _)| *f).collect::<Vec<_>>(), vec![now, now]);
    let st = rig.mix.event_status();
    assert_eq!((st.refused_too_early, st.late, st.max_late_frames), (1, 1, 300));
    assert_eq!(st.lateness_log2[9], 1, "256 <= 300 < 512");
}

#[test]
fn a_re_strike_releases_the_old_voice_first() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    rig.send(src, now, on(60));
    rig.send(src, now + 10, on(60));
    rig.send(src, now + 20, off(60));
    rig.render(1);
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 2);
    let kinds: Vec<(u64, bool)> = heard.iter().map(|(f, e)| (*f - now, matches!(e.message, Message::NoteOn { .. }))).collect();
    assert_eq!(kinds, vec![(0, true), (10, false), (10, true), (20, false)]);
    assert_eq!(rig.mix.event_status().restrikes, 1);
}

#[test]
fn a_revocation_cancels_queued_events_and_releases_sounding_notes() {
    let mut rig = Rig::new();
    let (a, b) = (port(1, 0), port(2, 1));
    let now = rig.mix.next_frame().0;
    rig.send(a, now, on(60));
    rig.send(a, now + 3000, on(62));
    rig.send(a, now, Message::PitchBend { value: 12000 });
    rig.send(b, now, on(60));
    rig.render(1);
    rig.mix.revoke_source(1, 1);
    rig.render(20);
    let heard = rig.heard_by(1);
    let a_events: Vec<_> = heard.iter().filter(|(_, e)| e.channel == 0).map(|(_, e)| e.message).collect();
    assert_eq!(a_events.len(), 4, "on, bend, then the revocation's off and neutral bend: {a_events:?}");
    assert!(matches!(a_events[2], Message::NoteOff { key: 60, .. }));
    assert_eq!(a_events[3], Message::PitchBend { value: 8192 });
    let b_events = heard.iter().filter(|(_, e)| e.channel == 1).count();
    assert_eq!(b_events, 1, "the other port's note keeps sounding");
}

#[test]
fn a_seek_releases_the_clips_notes_and_keeps_live_ones() {
    let mut rig = Rig::new();
    let mut s = rig.session.clone();
    let mut seq = transit_midi::Sequence::default();
    seq.notes.push(transit_midi::Note { pitch: 48, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 9600 });
    let cid = s.new_clip_id();
    s.track_mut(rig.track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "m", 0, 48_000 * 8, seq));
    rig.publish(&s);
    rig.pos = 0;
    let now = rig.mix.next_frame().0;
    rig.send(port(1, 0), now, on(72));
    rig.render(2);
    // Seek back to the start.
    rig.pos = 0;
    rig.render(1);
    let heard = rig.heard_by(1);
    let offs: Vec<u8> = heard
        .iter()
        .filter_map(|(_, e)| match e.message {
            Message::NoteOff { key, .. } => Some(key),
            _ => None,
        })
        .collect();
    assert_eq!(offs, vec![48], "the clip's note was released at the seek, the live one was not");
}

#[test]
fn all_notes_off_releases_every_tracked_voice() {
    let mut rig = Rig::new();
    let now = rig.mix.next_frame().0;
    rig.send(port(1, 2), now, on_id(60, 9));
    rig.send(port(1, 2), now, cc(64, 127));
    rig.send(port(1, 2), now + 4000, on(61));
    rig.render(1);
    rig.mix.all_notes_off();
    rig.render(20);
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 1, "the queued future note was dropped, the sounding one released");
    assert_eq!(
        pedal_edges(&heard.iter().map(|(f, e)| (*f, PluginEvent { channel: 0, ..*e })).collect::<Vec<_>>(), 64),
        vec![(now, true), (now + BLOCK as u64, false)]
    );
    assert!(rig.heard.lock().unwrap().iter().any(|(_, _, e)| e.is_none()), "and the plug-in was told to release everything");
}

#[test]
fn pending_events_keep_a_stopped_engine_awake_until_delivered() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    assert!(!rig.mix.has_pending_events());
    let now = rig.mix.next_frame().0;
    rig.send(src, now + 600, on(60));
    rig.send(src, now + 1200, off(60));
    let mut awake = Vec::new();
    for _ in 0..8 {
        awake.push(rig.mix.has_pending_events());
        rig.render(1);
    }
    assert_eq!(awake, [true, true, true, true, true, false, false, false], "true until the off at frame +1200 is delivered");
    // A held pedal keeps it awake until a revocation releases it.
    let now = rig.mix.next_frame().0;
    rig.send(src, now, cc(64, 127));
    rig.render(3);
    assert!(rig.mix.has_pending_events(), "the pedal is held");
    rig.mix.revoke_source(1, 1);
    assert!(!rig.mix.has_pending_events(), "released by the revocation");
    rig.render(1);
    assert_eq!(pedal_edges(&rig.heard_by(1), 64).last(), Some(&(now + 3 * BLOCK as u64, false)));
}

// ---- review fixes (F10 post-review) ----

/// Review F10-1: `MixEngine::reset` (Stop) releases every source and the plug-in, resetting like
/// a real host, still gets the pedal-up and the note-off on the next block.
#[test]
fn a_reset_releases_every_source_and_the_releases_survive_the_host_reset() {
    let mut rig = Rig::new();
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(port(1, 0), now, on(72)), EventOutcome::Queued);
    assert_eq!(rig.send(port(1, 0), now, cc(64, 127)), EventOutcome::Queued);
    rig.render(1);
    rig.heard.lock().unwrap().clear();
    rig.mix.reset();
    rig.render(1);
    let heard = rig.heard_by(1);
    assert!(heard.iter().any(|(_, e)| matches!(e.message, Message::NoteOff { key: 72, .. })), "the note-off: {heard:?}");
    assert_eq!(pedal_values(&heard, 64), vec![(now + BLOCK as u64, 0)], "the pedal-up");
    assert!(!rig.mix.has_pending_events());
    assert_eq!(rig.send(port(1, 0), rig.mix.next_frame().0, off(72)), EventOutcome::Unmatched, "the reset released it");
}

/// Review F10-2: a release carrying an older transport epoch, for a note and pedal that sound, is
/// admitted and delivered; nothing stays pending.
#[test]
fn a_release_with_a_stale_transport_is_still_delivered() {
    let mut rig = Rig::new();
    let (t1, t2) = (TransportEpoch(1), TransportEpoch(2));
    rig.mix.set_transport_epoch(t1);
    let now = rig.mix.next_frame().0;
    let src = port(3, 0);
    let tev = |rig: &Rig, at: u64, m: Message| InstrumentEvent { transport: Some(t1), ..rig.event(src, Due::Frame(RenderFrame(at)), m) };
    assert_eq!(rig.mix.queue_event(&tev(&rig, now, on(60))), EventOutcome::Queued);
    assert_eq!(rig.mix.queue_event(&tev(&rig, now, cc(64, 127))), EventOutcome::Queued);
    rig.render(1);
    rig.mix.set_transport_epoch(t2);
    assert_eq!(rig.mix.queue_event(&tev(&rig, now + 300, off(60))), EventOutcome::Queued);
    assert_eq!(rig.mix.queue_event(&tev(&rig, now + 300, cc(64, 0))), EventOutcome::Queued);
    assert_eq!(rig.mix.queue_event(&tev(&rig, now + 300, on(61))), EventOutcome::StaleTransport, "non-releases still are refused");
    // A later transport change does not cancel the admitted releases.
    rig.mix.set_transport_epoch(TransportEpoch(3));
    rig.render(2);
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 1);
    assert_eq!(pedal_values(&heard, 64), vec![(now, 127), (now + 300, 0)]);
    assert!(!rig.mix.has_pending_events());
}

/// Review F10-4d: a re-strike's off is charged with its note-on, two slots, against the
/// non-release budget.
#[test]
fn a_re_strike_is_charged_two_slots() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(src, now, on(60)), EventOutcome::Queued);
    rig.render(1);
    let now = rig.mix.next_frame().0;
    // 255 of the 256 non-release slots.
    for k in 0..255u64 {
        assert_eq!(rig.send(src, now + 1000 + k, cc(1, 1)), EventOutcome::Queued);
    }
    assert_eq!(rig.send(src, now, on(60)), EventOutcome::Refused(Shed::QueueFull), "a re-strike needs two slots");
    assert_eq!(rig.send(src, now, on(61)), EventOutcome::Queued, "a new note needs one");
    rig.render(8);
    // Delivered: the budget is back, and a re-strike fits.
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(src, now, on(60)), EventOutcome::Queued);
    rig.render(1);
    assert_eq!(rig.mix.event_status().restrikes, 1);
    rig.mix.reset();
    rig.render(1);
    assert_eq!(every_on_has_its_off(&rig.heard_by(1)), 3);
}
