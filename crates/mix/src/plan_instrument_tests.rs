#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan_tests::*;
use super::*;

// Coverage for the review's mutants.

fn synth_track(s: &mut Session) -> TrackId {
    use transit_midi::{Note, Sequence};
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    for (i, p) in [60u8, 64, 67].into_iter().enumerate() {
        seq.notes.push(Note { pitch: p, velocity: 100, release_velocity: 64, channel: 0, start: i as i64 * 10, length: 40_000 });
    }
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 96_000, seq));
    tr.instrument = Some(builtin("macro_oscillator"));
    t
}

#[test]
fn held_notes_move_with_the_instrument_and_note_chasing_with_the_strip() {
    let mut s = Session::default();
    let t = synth_track(&mut s);
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(4);
    let strip = |rig: &Rig| {
        let p = rig.eng.plan.as_deref().unwrap();
        let st = &p.tables.strips[p.tables.strip_of(t).unwrap()];
        let keys: Vec<u8> = st.instrument.as_ref().unwrap().events.as_deref().unwrap().sounding().map(|(_, key, _)| key).collect();
        (keys, st.chase)
    };
    let before = strip(&rig);
    assert_eq!(before, (vec![60, 64, 67], false), "three notes sound; the controllers were chased");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(strip(&rig), before, "a structural plan keeps the held notes and the chase state");
}
