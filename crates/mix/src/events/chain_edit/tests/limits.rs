//! The stated limits of 2.13 under refusal, pinned as today's behavior so that ADR 0059 changes
//! them knowingly (decision 13's P3a evidence, last item).

use super::*;

/// CC 64 held and 60 held through an edit whose note-off the instrument refuses in the edit's
/// block only; with `press`, the player lifts the stale pedal and presses it again in that block.
fn refused_once(at: At, mode: Mode, press: bool) -> Rig {
    let (before, after) = chains(at, &[(OWN, &[], false)]);
    let mut r = Rig::new(mode);
    r.live(5, A, cc(64, 127));
    r.live(10, A, on(60));
    if press {
        r.live(90, A, cc(64, 0));
        r.live(100, A, cc(64, 127));
        r.live(200, A, cc(64, 0));
    }
    r.run_until(&before, 64, 64);
    r.m.refuse_offs = 1..2;
    r.run_until(&after, 320, 64);
    r
}

#[test]
fn a_refused_edit_note_off_is_retried_at_the_next_block_and_the_voice_ends() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let r = refused_once(at, mode, false);
            assert_eq!(r.given_from(64), [(64, "cc", 64, 0), (128, "off", 60, 0)], "{ctx}: the pedal-up accepted, the note-off retried at offset 0");
            assert_eq!(r.m.spans(60), [(10, Some(128))], "{ctx}");
            assert_eq!(r.e.fx.counts.retry_overflow, 0, "{ctx}");
        }
    }
}

#[test]
fn a_new_press_before_the_retried_note_off_sustains_the_voice_until_the_next_lift() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let r = refused_once(at, mode, true);
            assert_eq!(
                r.given_from(64),
                [(64, "cc", 64, 0), (100, "cc", 64, 127), (128, "off", 60, 0), (200, "cc", 64, 0)],
                "{ctx}: the stale lift at 90 sends nothing"
            );
            assert_eq!(r.m.spans(60), [(10, Some(200))], "{ctx}: today's limit: sustained by the new press (#600)");
        }
    }
}

#[test]
fn seventy_notes_and_a_pedal_refused_at_an_edit_keep_sixty_four_releases_and_stop_ends_the_rest() {
    let keys = 30u8..100;
    for at in ATS {
        let ctx = format!("{at:?}");
        let (before, after) = chains(at, &[(OWN, &[], false)]);
        let mut r = Rig::new(Mode::Plain);
        r.live(5, A, cc(64, 127));
        for k in keys.clone() {
            r.live(10, A, on(k));
        }
        r.run_until(&before, 64, 64);
        assert_eq!(r.m.sounding().len(), 70, "{ctx}");
        // Every release refused in the edit's block.
        r.m.refuse_releases = 1..2;
        r.run(&after, 64);
        assert!(r.given_from(64).is_empty(), "{ctx}");
        r.run(&after, 64);
        let given = r.given_from(128);
        match at {
            At::Later => {
                // The host's retry buffer keeps the first 64 note-offs; six note-offs and the
                // pedal-up are lost and counted (#600).
                let kept: Vec<_> = keys.clone().take(64).map(|k| (128, "off", k, 0)).collect();
                assert_eq!(given, kept, "{ctx}");
                assert_eq!(r.e.fx.counts.retry_overflow, 7, "{ctx}");
                assert_eq!(r.m.ctl[0][64], 127, "{ctx}: the lost pedal-up leaves the pedal down");
                assert_eq!(r.m.sounding().len(), 70, "{ctx}: 64 sustained by the pedal, 6 never released");
            }
            At::First => {
                // The direct note-offs keep the mix's per-entry retry (bounded by its entries, so
                // none is lost); only the pedal-up uses the host's retry buffer.
                let mut all: Vec<_> = keys.clone().map(|k| (128, "off", k, 0)).collect();
                all.push((128, "cc", 64, 0));
                assert_eq!(given, all, "{ctx}");
                assert_eq!(r.e.fx.counts.retry_overflow, 0, "{ctx}");
                assert!(r.m.sounding().is_empty(), "{ctx}");
            }
        }
        r.stop(&after, 64);
        assert!(r.m.sounding().is_empty(), "{ctx}: after Stop nothing sounds at a built-in instrument");
    }
}
