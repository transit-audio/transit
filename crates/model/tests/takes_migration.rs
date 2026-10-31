//! Migration detection shares the take-stack range and material rules.

use transit_model::{ClipId, Playlist, Session, TrackId};

fn old() -> Session {
    Session::from_json(include_str!("corpus/v0.5.1-pre-take-lanes.transit")).unwrap()
}

#[test]
fn the_old_fixture_detects_both_stacks_in_recording_order() {
    for track in [TrackId(2), TrackId(9)] {
        let s = old();
        let t = s.track(track).unwrap();
        let (base, stack) = t.legacy_take_stack().unwrap();
        assert_eq!(base, 0);
        assert_eq!(stack, t.take_stacks()[0]);
        assert_eq!(stack.active(), Some(3));
        assert!(stack.takes.iter().all(|k| !k.explicit));
    }
}

#[test]
fn a_numbered_stack_and_an_older_selected_playlist_are_not_legacy_recordings() {
    let mut s = old();
    let t = s.track_mut(TrackId(9)).unwrap();
    t.active_playlist = 1;
    assert!(t.legacy_take_stack().is_none(), "an older selected take is left alone");
    t.active_playlist = 2;
    t.playlists[0].clips[0].take = Some(1);
    assert!(t.legacy_take_stack().is_none(), "a partially numbered stack is not a pre-#25 recording");
}

#[test]
fn the_original_playlist_is_the_first_participant_not_an_unrelated_earlier_playlist() {
    let mut s = old();
    let t = s.track_mut(TrackId(9)).unwrap();
    let mut outside = t.clips()[0].clone();
    outside.id = ClipId(100);
    outside.start = 192_000;
    t.playlists.insert(0, Playlist { name: "Unrelated".into(), clips: vec![outside] });
    t.active_playlist += 1;
    assert_eq!(t.legacy_take_stack().unwrap().0, 1);
}
