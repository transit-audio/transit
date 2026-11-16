# Bounce in Place silently muted unselected pickups (#713)

1. **Failure.** Bouncing only clips after bar 1 with source Mute, Mute and deactivate, or Hide
   stopped other clips before bar 1 on the same track without a notice.
2. **Mechanism.** `crates/engine/src/cmd/clip_bounce.rs` required the selected span to start
   before bar 1, although `bounce_track_range` applies the source choice to the whole track.
3. **Why checks missed it.** The existing notice tests selected the clip before bar 1. They
   did not separate the printed span from the scope of the source action. They also lacked a
   track with both muted and unmuted clips before bar 1.
4. **Regression test.** In `crates/engine/tests/bounce_notices.rs`,
   `bouncing_later_clips_reports_the_unselected_pickup_it_silences` rendered audible audio
   before the bounce and silence afterward, then failed on the missing notice before the fix.
   It covers all source choices and already-muted tracks and clips. The three
   `*_reports_a_mix_of_muted_and_unmuted_zone_clips` tests failed with `any` changed to `all`.
5. **Gate class.** A notice about lost playback is tested across the full scope of the state
   change, including unselected content and a mixture of muted and audible clips.
6. **Protocol change, owner, blind spot.** The engine tests and the review rule in
   `../contributing/evidence/gates.md` cover this class; the maintainer owns the gate. The check
   uses track and clip mute state, not signal analysis of routing, automation or plug-in output.
