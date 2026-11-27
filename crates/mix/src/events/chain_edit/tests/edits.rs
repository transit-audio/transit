//! The notes from before an edit end at it: the first insertion through the mix's table, a later
//! edit through the host's own (2.13 step 1; decision 13's P3a evidence, first two items).

use super::*;

/// The note-offs and note-ons the instrument accepted, summarized.
fn notes_given(r: &Rig) -> Vec<(u64, &'static str, u8, u8)> {
    r.given_from(0).into_iter().filter(|g| g.1 != "cc").collect()
}

#[test]
fn the_first_effect_added_ends_a_note_the_mix_gave_directly_and_its_release_sends_nothing() {
    let added: [(&[(&str, f32)], &str); 2] = [(&[], OWN), (&[("semitones", 12.0)], TRANSPOSE)];
    for (params, id) in added {
        for mode in MODES {
            let ctx = format!("{id} {mode:?}");
            let (before, after) = chains(At::First, &[(id, params, false)]);
            let mut r = Rig::new(mode);
            r.live(10, A, on(60));
            r.live(100, A, off(60));
            r.run_until(&before, 64, 64);
            assert!(!r.e.fx.active(), "{ctx}: no chain, the note went to the instrument directly");
            r.run_until(&after, 256, 64);
            // The note ends at the add, at offset 0, with the voice id it was given; its release at
            // 100 sends nothing (through a chain, Transpose would have written an off of 72).
            assert_eq!(notes_given(&r), [(10, "on", 60, 100), (64, "off", 60, 0)], "{ctx}");
            let ids: Vec<i32> = r.m.given.iter().map(|(_, e)| e.note_id).collect();
            assert_eq!(ids[0], ids[1], "{ctx}: the off carries the voice id the on was given");
            assert_eq!(r.m.spans(60), [(10, Some(64))], "{ctx}");
            assert!(r.m.sounding().is_empty(), "{ctx}");
        }
    }
}

#[test]
fn at_the_first_insertion_a_refused_note_off_is_given_directly_until_accepted() {
    let added: [(&[(&str, f32)], &str); 2] = [(&[], OWN), (&[("semitones", 12.0)], TRANSPOSE)];
    for (params, id) in added {
        for mode in MODES {
            let ctx = format!("{id} {mode:?}");
            let (before, after) = chains(At::First, &[(id, params, false)]);
            let mut r = Rig::new(mode);
            r.live(10, A, on(60));
            r.live(100, A, off(60));
            r.run_until(&before, 64, 64);
            // The instrument refuses every note-off in the edit's block and the next.
            r.m.refuse_offs = 1..3;
            r.run_until(&after, 320, 64);
            // Given again at offset 0 of each later block, directly (never through the chain:
            // Transpose would write 72, the owning fixture would consume it), and accepted at 192.
            assert_eq!(notes_given(&r), [(10, "on", 60, 100), (192, "off", 60, 0)], "{ctx}");
            assert_eq!(r.m.spans(60), [(10, Some(192))], "{ctx}");
            assert_eq!(r.e.fx.counts.retry_overflow, 0, "{ctx}");
            assert!(!r.e.notes.iter().any(|n| n.used), "{ctx}: the mix's entry is freed once accepted");
        }
    }
}

/// A later edit (`after` from `before`): a note through the old chain ends at the edit, its late
/// release is swallowed, and a new note goes through the new chain.
fn a_later_edit_ends_the_chains_notes(before: &Track, after: &Track, new_key: u8) {
    for mode in MODES {
        let mut r = Rig::new(mode);
        r.live(10, A, on(60));
        r.live(100, A, off(60));
        r.live(110, A, on(62));
        r.live(120, A, off(62));
        r.run_until(before, 64, 64);
        r.run_until(after, 256, 64);
        assert_eq!(
            notes_given(&r),
            [(10, "on", 60, 100), (64, "off", 60, 0), (110, "on", new_key, 100), (120, "off", new_key, 0)],
            "{mode:?}: the late release of 60 is swallowed"
        );
        assert_eq!(r.e.fx.counts.released_on_edit, 1, "{mode:?}");
        assert!(r.m.sounding().is_empty(), "{mode:?}");
    }
}

#[test]
fn an_insertion_into_an_existing_chain_ends_its_notes_and_swallows_their_releases() {
    let (before, after) = chains(At::Later, &[(TRANSPOSE, &[("semitones", 12.0)], false)]);
    a_later_edit_ends_the_chains_notes(&before, &after, 74);
}

#[test]
fn an_un_bypass_ends_the_chains_notes_and_swallows_their_releases() {
    let before = track(&[(OWN, &[], true)]);
    let mut after = before.clone();
    after.midi_effects[0].bypass = false;
    a_later_edit_ends_the_chains_notes(&before, &after, 62);
}

#[test]
fn removing_the_last_effect_ends_the_chains_notes_and_swallows_their_releases() {
    let before = track(&[(OWN, &[], false)]);
    let after = track(&[]);
    a_later_edit_ends_the_chains_notes(&before, &after, 62);
}
