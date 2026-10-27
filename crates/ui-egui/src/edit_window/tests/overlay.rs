use super::*;

#[test]
fn follow_jumps_to_offscreen_playhead_while_playing() {
    // Steady view, follow mode, playhead left the screen: jump to it.
    assert_eq!(follow_update(true, true, false, 1_000, 1_000, None, false, 96_000), (false, Some(96_000)));
}

#[test]
fn follow_leaves_onscreen_playhead_alone() {
    assert_eq!(follow_update(true, true, true, 1_000, 1_000, None, false, 96_000), (false, None));
}

#[test]
fn follow_never_scrolls_when_stopped_nor_in_free_modes() {
    assert_eq!(follow_update(false, true, false, 1_000, 1_000, None, false, 96_000), (false, None));
    // `none`/`after_playback` never follow, even with a steady view.
    assert_eq!(follow_update(true, false, false, 1_000, 1_000, None, false, 96_000), (false, None));
    assert_eq!(follow_update(true, false, false, 9_000, 1_000, None, false, 96_000), (false, None));
}

#[test]
fn outside_view_move_pauses_the_follow() {
    // The view moved and it was not our own jump: pause, no jump.
    assert_eq!(follow_update(true, true, false, 9_000, 1_000, None, false, 96_000), (true, None));
    // A held view stays held while it sits still elsewhere.
    assert_eq!(follow_update(true, true, false, 9_000, 9_000, None, true, 96_000), (true, None));
}

#[test]
fn own_follow_jump_is_not_mistaken_for_a_takeover() {
    // The scroll equals the target we issued last frame: keep following.
    assert_eq!(follow_update(true, true, false, 96_000, 1_000, Some(96_000), false, 97_000), (false, Some(97_000)));
}

#[test]
fn hold_clears_when_playhead_returns_or_transport_stops() {
    assert_eq!(follow_update(true, true, true, 9_000, 9_000, None, true, 96_000), (false, None));
    assert_eq!(follow_update(false, true, false, 9_000, 9_000, None, true, 96_000), (false, None));
}
