//! Every pedal the instrument holds is released once at an edit and none is pressed again;
//! expression is kept (2.13 steps 2 to 4; decision 13's P3a evidence).

use super::*;

fn ccs_given(r: &Rig, from: u64) -> Vec<(u64, &'static str, u8, u8)> {
    r.given_from(from).into_iter().filter(|g| g.1 == "cc").collect()
}

#[test]
fn a_held_key_under_the_pedal_ends_at_the_edit_and_the_instruments_pedal_is_up() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let (before, after) = chains(at, &[(OWN, &[], false)]);
            let mut r = Rig::new(mode);
            r.live(5, A, cc(64, 127));
            r.live(10, A, on_id(60, 42));
            r.live(100, A, off_id(60, 42));
            r.run_until(&before, 64, 64);
            r.run_until(&after, 256, 64);
            assert_eq!(
                r.given_from(0),
                [(5, "cc", 64, 127), (10, "on", 60, 100), (64, "off", 60, 0), (64, "cc", 64, 0)],
                "{ctx}: the note-off, then the pedal-up, at the edit; the release sends nothing"
            );
            assert_eq!(r.m.spans(60), [(10, Some(64))], "{ctx}: nothing sounds from the edit on");
            assert_eq!(r.m.ctl[0][64], 0, "{ctx}: the instrument's pedal is up");
            assert_eq!(r.e.fx.pedal_value(0, 64), 0, "{ctx}: and the host's table says so");
        }
    }
}

#[test]
fn a_note_held_only_by_the_pedal_ends_at_the_edit() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let (before, after) = chains(at, &[(OWN, &[], false)]);
            let mut r = Rig::new(mode);
            r.live(5, A, cc(64, 127));
            r.live(10, A, on(60));
            r.live(20, A, off(60));
            r.run_until(&before, 64, 64);
            assert_eq!(r.m.sounding(), [60], "{ctx}: the pedal holds it");
            r.run_until(&after, 192, 64);
            assert_eq!(r.m.spans(60), [(10, Some(64))], "{ctx}");
            assert_eq!(ccs_given(&r, 64), [(64, "cc", 64, 0)], "{ctx}");
        }
    }
}

#[test]
fn after_a_lift_and_a_new_press_the_old_key_is_neither_sustained_nor_captured_again() {
    for pedal in [64u8, 66] {
        for at in ATS {
            for mode in MODES {
                let ctx = format!("CC {pedal} {at:?} {mode:?}");
                let (before, after) = chains(at, &[(OWN, &[], false)]);
                let mut r = Rig::new(mode);
                // 60 struck before the press, so the sostenuto pedal captures it.
                r.live(5, A, on(60));
                r.live(8, A, cc(pedal, 127));
                r.live(100, A, cc(pedal, 0));
                r.live(120, A, cc(pedal, 127));
                r.live(150, A, off(60));
                r.live(160, A, on(62));
                r.live(170, A, off(62));
                r.live(200, A, cc(pedal, 0));
                r.run_until(&before, 64, 64);
                r.run_until(&after, 320, 64);
                assert_eq!(r.m.spans(60), [(5, Some(64))], "{ctx}: 60 ended at the edit and nothing brought it back");
                assert_eq!(
                    ccs_given(&r, 64),
                    [(64, "cc", pedal, 0), (120, "cc", pedal, 127), (200, "cc", pedal, 0)],
                    "{ctx}: the stale lift at 100 sends nothing; the new press and its lift pass"
                );
                if pedal == 64 {
                    assert_eq!(r.m.spans(62), [(160, Some(200))], "{ctx}: a key struck under the new press is sustained until the lift");
                }
                assert!(r.m.sounding().is_empty(), "{ctx}");
            }
        }
    }
}

#[test]
fn consecutive_edits_leave_no_generated_note_sounding_past_its_gate() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            // The owning pass-through added, then the gated generator in the next block.
            let (before, first) = chains(at, &[(OWN, &[], false)]);
            let mut second = first.clone();
            second.midi_effects.extend(track(&[(GATED, &[("period", 32.0), ("gate", 16.0)], false)]).midi_effects);
            let mut r = Rig::new(mode);
            r.live(5, A, cc(64, 127));
            // A key held into the generator, a physical pedal-up during the second edit's block, a
            // new press later (the generator consumes it), the key released under it.
            r.live(130, A, on(60));
            r.live(140, A, cc(64, 0));
            r.live(300, A, cc(64, 127));
            r.live(400, A, off(60));
            r.run_until(&before, 64, 64);
            r.run_until(&first, 128, 64);
            r.run_until(&second, 640, 64);
            let spans = r.m.spans(60);
            // Steps on the generator's clock (from its reset at 128) every 32 frames while it holds
            // 60: from 160 to 608.
            let expected: Vec<(u64, Option<u64>)> = (160..640).step_by(32).map(|on| (on, Some(on + 16))).collect();
            assert_eq!(spans, expected, "{ctx}: every generated note ends at its gate");
            assert_eq!(
                ccs_given(&r, 64),
                [(64, "cc", 64, 0)],
                "{ctx}: only the first edit's pedal-up; the lift at 140 and the press at 300 send nothing"
            );
            assert_eq!(r.m.ctl[0][64], 0, "{ctx}");
        }
    }
}

#[test]
fn a_pedal_held_through_an_edit_sustains_nothing_until_it_is_pressed_again() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let (before, after) = chains(at, &[(OWN, &[], false)]);
            let mut r = Rig::new(mode);
            r.live(5, A, cc(64, 127));
            r.live(80, A, on(62));
            r.live(90, A, off(62));
            r.live(100, A, cc(64, 0));
            r.live(120, A, cc(64, 127));
            r.live(130, A, on(64));
            r.live(140, A, off(64));
            r.live(200, A, cc(64, 0));
            r.run_until(&before, 64, 64);
            r.run_until(&after, 256, 64);
            assert_eq!(r.m.spans(62), [(80, Some(90))], "{ctx}: played after the edit, not sustained");
            assert_eq!(r.m.spans(64), [(130, Some(200))], "{ctx}: after a lift and a new press, sustained until the lift");
        }
    }
}

#[test]
fn two_sources_holding_the_pedal_through_an_edit_send_nothing_at_their_releases() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let (before, after) = chains(at, &[(OWN, &[], false)]);
            let mut r = Rig::new(mode);
            r.live(5, A, cc(64, 127));
            r.live(6, B, cc(64, 127));
            r.live(100, A, cc(64, 0));
            r.live(102, A, on(62));
            r.live(104, A, off(62));
            r.live(110, B, cc(64, 0));
            r.live(120, A, cc(64, 127));
            r.run_until(&before, 64, 64);
            r.run_until(&after, 256, 64);
            assert_eq!(ccs_given(&r, 64), [(64, "cc", 64, 0), (120, "cc", 64, 127)], "{ctx}: neither release sends anything; the next press passes");
            assert_eq!(r.m.spans(62), [(102, Some(104))], "{ctx}: a note between the two releases is not sustained");
            assert_eq!(r.m.ctl[0][64], 127, "{ctx}");
        }
    }
}

#[test]
fn a_clip_pedal_down_one_frame_after_an_edit_reaches_the_instrument_on_that_frame() {
    // 3584 starts a block at every size: the edit takes effect at the same frame in every partition.
    const EDIT: u64 = 3584;
    for at in ATS {
        for mode in MODES {
            let mut renders = Vec::new();
            for frames in [1usize, 7, 64, 512] {
                let ctx = format!("{at:?} {mode:?} block {frames}");
                let (before, after) = chains(at, &[(OWN, &[], false)]);
                let mut r = Rig::new(mode);
                r.live(5, A, cc(64, 127));
                r.live(10, A, on(60));
                r.clip_pedal(EDIT + 1, 127);
                r.run_until(&before, EDIT, frames);
                r.run_until(&after, EDIT + 1024, frames);
                let from = r.given_from(EDIT);
                assert_eq!(from, [(EDIT, "off", 60, 0), (EDIT, "cc", 64, 0), (EDIT + 1, "cc", 64, 127)], "{ctx}");
                assert_eq!(r.m.ctl[0][64], 127, "{ctx}: the instrument's pedal is down");
                renders.push(r.m.given.iter().map(|(f, e)| summary(*f, e)).collect::<Vec<_>>());
            }
            assert!(renders.windows(2).all(|w| w[0] == w[1]), "{at:?} {mode:?}: the same events at every frame, every partition");
        }
    }
}

#[test]
fn pitch_bend_and_the_mod_wheel_set_before_an_edit_are_kept() {
    for at in ATS {
        for mode in MODES {
            let ctx = format!("{at:?} {mode:?}");
            let (before, after) = chains(at, &[(OWN, &[], false)]);
            let mut r = Rig::new(mode);
            r.live(5, A, Message::PitchBend { value: 10_000 });
            r.live(6, A, cc(1, 90));
            r.live(10, A, on(60));
            r.run_until(&before, 64, 64);
            r.run_until(&after, 192, 64);
            assert_eq!(r.given_from(64), [(64, "off", 60, 0)], "{ctx}: the note ends; nothing is sent for expression, no CC 120, 121 or 123");
            assert_eq!((r.m.bend[0], r.m.ctl[0][1]), (10_000, 90), "{ctx}");
        }
    }
}
