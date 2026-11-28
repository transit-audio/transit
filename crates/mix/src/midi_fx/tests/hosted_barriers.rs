//! The hosted stage's barriers, its generation protocol, the failing block, the states the user did
//! not ask for and between-blocks waiting (ADR 0064 HM2: decisions 4.6, 6 and 7), with the fake note
//! plug-in in the slot and a recording instrument, at host block sizes 1, 7, 64 and 512.

use super::hosted::{CLAP, Rig, SIZES, cell, desc, sounding, stop_and_settle};
use super::hosted_fake::{Fake, Handle, midi, out};
use super::*;
use transit_dsp::hosted_out::{NoteOutputInfo, OutKind};

/// The barrier frame: a block start at every size (`Rig::run` cuts the last block short).
const B: u64 = 1000;

fn on(ch: i16, key: i16, id: i32) -> transit_dsp::hosted_out::HostedOut {
    out(OutKind::NoteOn, ch, key, id, 1.0)
}

fn off(ch: i16, key: i16, id: i32) -> transit_dsp::hosted_out::HostedOut {
    out(OutKind::NoteOff, ch, key, id, 0.0)
}

fn input(key: u8, voice: i32, on: bool) -> PluginEvent {
    let message = if on { Message::NoteOn { key, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key, velocity: 0, id: NoteId::NONE } };
    PluginEvent { channel: 0, note_id: voice, message }
}

/// A fresh fake instance in a cell for the rig's stage `index`.
fn fake_cell(r: &Rig, index: usize, generation: u64, native: usize) -> (crate::PluginSlot, Handle) {
    let id = r.track.midi_effects[index].plugin.clone();
    let (mut f, h) = Fake::new(native);
    f.prepare(48_000.0, native, 2);
    (cell(Box::new(f), &id, r.track.midi_effects[index].uid.get(), generation), h)
}

/// The standard scene: the plug-in holds a note (host id 0) and presses the pedal; it would write
/// that note's release at B + 30 (after the barrier).
fn scene(size: usize) -> Rig {
    let mut r = Rig::new(CLAP, 512);
    r.fake.lock().at = vec![(10, on(0, 60, 1)), (12, midi(OutKind::Controller, [0xB0, 64, 127])), (B + 30, off(0, 60, 1))];
    r.run(B, size, &[]);
    assert_eq!(r.heard(), vec![(10, "on", 0, 60, 127, 0), (12, "cc", 0, 64, 127, -1)]);
    r
}

/// After a barrier at `B`: the note ended at offset 0 of the barrier block, a pedal-up there when
/// `edit`, and the plug-in's later release consumed (nothing for that note afterwards).
fn ended_at_b(r: &Rig, edit: bool, what: &str) {
    let mut want = vec![(B, "off", 0, 60, 0, 0)];
    if edit {
        want.push((B, "cc", 0, 64, 0, -1));
    }
    assert_eq!(r.heard_from(B), want, "{what}: the barrier at offset 0 of its block, nothing after");
}

#[test]
fn every_barrier_ends_the_stages_notes_at_offset_0_and_consumes_the_plug_ins_later_releases() {
    for size in SIZES {
        let ctx = |w: &str| format!("{w}, block {size}");
        // Removing the stage (the plan drops its cell).
        let mut r = scene(size);
        r.track.midi_effects.clear();
        r.cells.clear();
        r.run(2048, size, &[]);
        ended_at_b(&r, true, &ctx("remove"));
        stop_and_settle(&mut r, "remove");

        // Bypassing it: the plug-in no longer runs.
        let mut r = scene(size);
        r.track.midi_effects[0].bypass = true;
        r.run(2048, size, &[]);
        ended_at_b(&r, true, &ctx("bypass"));
        assert_eq!(r.edits, vec![0, B]);
        stop_and_settle(&mut r, "bypass");

        // Replacing it by another plug-in (a new slot, a new instance).
        let mut r = scene(size);
        r.track.midi_effects[0] = MidiEffectSlot::new(CLAP);
        let (c, h) = fake_cell(&r, 0, 8, 512);
        r.cells[0] = Some(c);
        // The new instance's own time starts at its first call (B).
        h.lock().at = vec![(50, on(0, 62, 1))];
        r.run(2048, size, &[]);
        let mut want = vec![(B, "off", 0, 60, 0, 0), (B, "cc", 0, 64, 0, -1)];
        want.push((B + 50, "on", 0, 62, 127, 1));
        assert_eq!(r.heard_from(B), want, "{}", ctx("replace"));
        assert_eq!(h.lock().resets, vec![0], "the new instance (its own frame 0)'s reset ran before its first call");
        stop_and_settle(&mut r, "replace");

        // Stop and panic (the host's panic): no pedal-ups from the host (the mix releases pedals).
        let mut r = scene(size);
        r.stop();
        r.run(2048, size, &[]);
        ended_at_b(&r, false, &ctx("stop"));
        stop_and_settle(&mut r, "stop");

        // A restart, latency changed or not (both are a new generation to the stage), and a state
        // load: the barrier is an edit (pedal-ups too).
        for active_load in ["restart", "state load"] {
            let mut r = scene(size);
            r.fake.lock().bump(true);
            r.run(2048, size, &[]);
            ended_at_b(&r, true, &ctx(active_load));
            assert_eq!(r.fake.lock().resets.last(), Some(&B), "{}: the reset before the next call", ctx(active_load));
            stop_and_settle(&mut r, active_load);
        }

        // A state load applied while calls are skipped: the barrier at B; the reset runs at the first
        // call actually made (B + 100, when the main thread lets go).
        let mut r = scene(size);
        {
            let mut f = r.fake.lock();
            f.held = true;
            f.bump(true);
            f.at.push((B + 150, on(0, 61, 2)));
        }
        r.run(B + 100, size, &[]);
        r.fake.lock().held = false;
        r.run(2048, size, &[]);
        let mut want = vec![(B, "off", 0, 60, 0, 0), (B, "cc", 0, 64, 0, -1)];
        want.push((B + 150, "on", 0, 61, 127, 1));
        assert_eq!(r.heard_from(B), want, "{}", ctx("load while skipped"));
        assert_eq!(r.fake.lock().resets.last(), Some(&(B + 100)), "{}", ctx("load while skipped"));
        stop_and_settle(&mut r, "load while skipped");
    }
}

#[test]
fn bypassing_the_stage_mid_note_and_un_bypassing_it_end_every_sounding_note() {
    for size in SIZES {
        // Bypassed: the input passes through; un-bypassing at B ends the input note that sounds,
        // swallows its later release, and the plug-in starts from its reset.
        let mut r = Rig::new(CLAP, 512);
        r.track.midi_effects[0].bypass = true;
        let events = [(5, input(70, 3, true)), (B + 40, input(70, 3, false))];
        r.run(B, size, &events);
        assert_eq!(r.heard(), vec![(5, "on", 0, 70, 100, 3)], "block {size}: bypass passes the input through");
        r.track.midi_effects[0].bypass = false;
        // Its own time starts at its first call (B): it was not processed while bypassed.
        r.fake.lock().at = vec![(60, on(0, 61, 9))];
        r.run(2048, size, &events);
        assert_eq!(r.heard_from(B), vec![(B, "off", 0, 70, 0, 3), (B + 60, "on", 0, 61, 127, 0)], "block {size}: the un-bypass");
        assert_eq!(r.fake.lock().resets, vec![0], "block {size}: its reset before its first call (its own frame 0)");
        stop_and_settle(&mut r, "un-bypass");
    }
}

#[test]
fn a_bypassed_stage_gives_the_instrument_exactly_the_events_it_gets_with_no_chain() {
    let events = [
        (3, input(60, 1, true)),
        (3, PluginEvent { channel: 2, note_id: -1, message: Message::PitchBend { value: 1234 } }),
        (90, PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 64, value: 127 } }),
        (400, input(60, 1, false)),
        (600, PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 64, value: 0 } }),
    ];
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.track.midi_effects[0].bypass = true;
        r.fake.lock().at = vec![(10, on(0, 61, 1))];
        r.run(1024, size, &events);
        let want: Vec<_> = events.iter().map(|(f, e)| desc(*f, e)).collect();
        assert_eq!(r.heard(), want, "block {size}");
        assert_eq!(r.fake.lock().blocks, 0, "a bypassed stage is not processed");
    }
}

#[test]
fn the_instance_leaving_and_arriving_is_a_barrier_and_nothing_is_reconstructed_for_held_keys() {
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().echo = Some(12);
        // A held input key: 60 from frame 5, released at 1900.
        let events = [(5, input(60, 1, true)), (1500, input(64, 2, true)), (1900, input(60, 1, false)), (1950, input(64, 2, false))];
        r.run(B, size, &events);
        assert_eq!(r.heard(), vec![(5, "on", 0, 72, 100, 0)], "block {size}: the echo of the held key");
        // The instance leaves (back to Loading): the barrier ends its note; input is consumed.
        let held = r.cells[0].take();
        r.run(1200, size, &events);
        assert_eq!(r.heard_from(B), vec![(B, "off", 0, 72, 0, 0)], "block {size}: leaving");
        // A new instance arrives at 1200: another barrier; the key held since 5 is not
        // reconstructed, a key struck after the arrival sounds, and the old key's release is
        // swallowed.
        drop(held);
        let (c, h) = fake_cell(&r, 0, 9, 512);
        h.lock().echo = Some(12);
        r.cells[0] = Some(c);
        r.run(2048, size, &events);
        assert_eq!(r.edits, vec![0, B, 1200], "block {size}");
        assert_eq!(r.heard_from(1200), vec![(1500, "on", 0, 76, 100, 1), (1950, "off", 0, 76, 0, 1)], "block {size}");
        assert_eq!(h.lock().heard.iter().map(|(f, e)| desc(*f, e).3).collect::<Vec<_>>(), vec![64, 64], "block {size}: only the new key reached it");
        assert_eq!(h.lock().resets, vec![0], "its reset before its first call (its own frame 0)");
        stop_and_settle(&mut r, "arrival");
    }
}

#[test]
fn a_refused_reactivation_silences_the_stage_until_an_activation_succeeds() {
    for size in SIZES {
        let mut r = scene(size);
        r.fake.lock().bump(false);
        r.fake.lock().echo = Some(12);
        let events = [(1100, input(65, 4, true)), (1150, input(65, 4, false)), (1600, input(67, 5, true)), (1700, input(67, 5, false))];
        r.run(1500, size, &events);
        ended_at_b(&r, true, &format!("refused, block {size}"));
        assert_eq!(r.fake.lock().offered, 0, "block {size}: its input is consumed while it is silent, never handed to it");
        // An activation that succeeds: another barrier, and the stage runs again.
        r.fake.lock().bump(true);
        r.run(2048, size, &events);
        assert_eq!(r.edits, vec![0, B, 1500], "block {size}");
        assert_eq!(r.heard_from(1500), vec![(1600, "on", 0, 79, 100, 1), (1700, "off", 0, 79, 0, 1)], "block {size}");
        stop_and_settle(&mut r, "refused reactivation");
    }
}

#[test]
fn a_generation_change_inside_the_block_discards_its_output_runs_the_barrier_and_the_next_call_resets() {
    // Decision 6's example: the stage checks generation G; a state load (or a restart) moves the
    // instance to G + 1 before its call; that call writes a note-on and keeps its release scheduled.
    for what in ["state load", "restart"] {
        for size in SIZES {
            let mut r = scene(size);
            {
                let mut f = r.fake.lock();
                f.at.retain(|(fr, _)| *fr < B);
                f.change_in_process = Some(true);
                f.holds = vec![(B, on(0, 64, 5), 100)];
                f.at.push((B + 600, on(0, 66, 6)));
                f.at.push((B + 650, off(0, 66, 6)));
            }
            r.run(2048, size, &[]);
            let ctx = format!("{what}, block {size}");
            // The note-on of the G + 1 call never reaches the instrument; the barrier ran at offset 0
            // of that block with an edit's steps (the note-off and the pedal-up); the next call ran
            // the reset and its output is accepted.
            assert_eq!(
                r.heard_from(B),
                vec![(B, "off", 0, 60, 0, 0), (B, "cc", 0, 64, 0, -1), (B + 600, "on", 0, 66, 127, 1), (B + 650, "off", 0, 66, 0, 1)],
                "{ctx}"
            );
            assert!(r.host.counts.hosted_discarded >= 1, "{ctx}");
            assert_eq!(r.fake.lock().resets.last(), Some(&(B + size as u64)), "{ctx}: the reset before the next call");
            assert!(r.fake.lock().due.is_empty(), "{ctx}: the reset erased the scheduled release");
            // Every note the instrument received is released, with no further Stop.
            assert!(sounding(&r.inst.heard).is_empty(), "{ctx}");
        }
    }
}

#[test]
fn no_output_is_accepted_before_its_epochs_reset_ran_nor_from_an_old_epoch_after_its_barrier() {
    for size in SIZES {
        // A barrier (Stop) whose reset never runs: nothing the plug-in writes afterwards is accepted.
        let mut r = scene(size);
        r.fake.lock().no_reset = true;
        r.fake.lock().at.push((B + 100, on(0, 70, 7)));
        r.stop();
        r.run(2048, size, &[]);
        assert_eq!(r.heard_from(B), vec![(B, "off", 0, 60, 0, 0)], "block {size}: nothing after the barrier");
        assert!(r.host.counts.hosted_discarded >= 1);

        // A call under the generation before the epoch's: discarded, and the barrier runs.
        let mut r = scene(size);
        r.fake.lock().at.push((B, on(0, 70, 7)));
        r.fake.lock().regress_in_process = true;
        r.run(2048, size, &[]);
        assert_eq!(r.heard_from(B)[0], (B, "off", 0, 60, 0, 0), "block {size}: the barrier");
        assert!(r.heard_from(B).iter().all(|h| !(h.1 == "on" && h.3 == 70)), "block {size}: the old epoch's note-on never reached the instrument");
        stop_and_settle(&mut r, "old epoch");
    }
}

#[test]
fn a_processing_error_fails_the_block_whole_and_the_stage_is_silent_until_stop() {
    // Host blocks of 64, a native maximum of 16: the error is reported by the second of the four
    // calls of the block at 64 (native call 5).
    let mut r = Rig::new(CLAP, 16);
    {
        let mut f = r.fake.lock();
        f.at = vec![(10, on(0, 60, 1)), (66, on(0, 62, 2)), (90, on(0, 63, 3))];
        f.fail_call = Some(5);
    }
    let events = [(70, input(80, 1, true)), (200, input(81, 2, true))];
    r.run(256, 64, &events);
    // The block's whole output is discarded (its first call's note-on too), the barrier ends the
    // sounding note at offset 0 of the failing block, and nothing passes afterwards.
    assert_eq!(r.heard(), vec![(10, "on", 0, 60, 127, 0), (64, "off", 0, 60, 0, 0)]);
    assert!(sounding(&r.inst.heard).is_empty(), "the instrument holds nothing after the failing block");
    let heard: Vec<u64> = r.fake.lock().heard.iter().map(|(f, _)| *f).collect();
    assert_eq!(heard, vec![70], "the failing block's input reached it; later input is consumed");
    assert_eq!(r.fake.lock().blocks, 2, "not processed while failed");
    // Stop ends the failure; the stage runs again from the next block, after its reset. (The
    // plug-in's own time stood still while it was not processed: 128 frames behind the host's.)
    assert_eq!(r.fake.lock().frame, 128);
    r.fake.lock().at.extend([(172, on(0, 65, 4)), (202, off(0, 65, 4))]);
    r.stop();
    r.run(512, 64, &[]);
    assert_eq!(r.heard_from(256), vec![(300, "on", 0, 65, 127, 1), (330, "off", 0, 65, 0, 1)]);
    assert_eq!(r.fake.lock().resets.last(), Some(&128), "the reset before its first call after Stop (its own frame 128)");
    stop_and_settle(&mut r, "failure");
}

#[test]
fn each_state_the_user_did_not_ask_for_passes_nothing_and_consumes_its_input() {
    let events = [(5, input(60, 1, true)), (300, input(60, 1, false)), (700, input(62, 2, true)), (900, input(62, 2, false))];
    for size in SIZES {
        // No instance yet (Loading), and a creation that returned nothing: the plan has no cell.
        let mut r = Rig::new(CLAP, 512);
        r.cells[0] = None;
        r.run(1024, size, &events);
        assert!(r.heard().is_empty(), "block {size}: no instance");

        // An instance with no readable note output ("Does not write notes"): not processed.
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().note_output = None;
        r.fake.lock().at = vec![(10, on(0, 60, 1))];
        r.run(600, size, &events);
        assert!(r.heard().is_empty(), "block {size}: no note output");
        assert!(r.fake.lock().heard.is_empty() && r.fake.lock().blocks == 0, "block {size}");
        // It reports a note output from a later configuration: leaving the state is a barrier, and
        // the key held since 5 is not reconstructed.
        {
            let mut f = r.fake.lock();
            f.note_output = Some(NoteOutputInfo { read: 0, declared: 1 });
            f.bump(true);
            f.echo = Some(1);
            f.at.clear();
        }
        r.run(1024, size, &events);
        assert_eq!(r.edits, vec![0, 600], "block {size}");
        assert_eq!(r.heard(), vec![(700, "on", 0, 63, 100, 0), (900, "off", 0, 63, 0, 0)], "block {size}");

        // Inactive from the start (its activation was refused).
        let mut r = Rig::new(CLAP, 512);
        r.fake.lock().bump(false);
        r.run(1024, size, &events);
        assert!(r.heard().is_empty() && r.fake.lock().offered == 0, "block {size}: inactive");
    }
}

#[test]
fn an_event_sent_between_blocks_reaches_the_plug_in_at_offset_0_of_the_next_actual_call() {
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.run(B, size, &[]);
        // The main thread holds the instance: the calls of [B, 1500) are skipped and the input of
        // that span (at 1200) is kept for the next actual call.
        r.fake.lock().held = true;
        r.run(1500, size, &[(1200, input(61, 1, true))]);
        let blocks = r.fake.lock().blocks;
        // Between blocks: waits; the stage is not processed.
        r.between(input(62, 2, true));
        assert_eq!(r.fake.lock().blocks, blocks, "block {size}: never processed between blocks");
        r.fake.lock().held = false;
        r.run(2048, size, &[(1600, input(63, 3, true))]);
        let heard: Vec<(u64, u16)> = r.fake.lock().heard.iter().map(|(f, e)| (*f, desc(*f, e).3)).collect();
        assert_eq!(heard, vec![(1500, 61), (1500, 62), (1600, 63)], "block {size}: kept input, then the between-blocks event, then the block's own");
    }
}

#[test]
fn a_skipped_call_extends_the_wait_and_a_barrier_discards_what_waits() {
    for size in SIZES {
        let mut r = Rig::new(CLAP, 512);
        r.run(B, size, &[]);
        r.between(input(62, 2, true));
        r.fake.lock().held = true;
        r.run(1200, size, &[]);
        r.fake.lock().held = false;
        r.run(1400, size, &[]);
        assert_eq!(r.fake.lock().heard.iter().map(|(f, _)| *f).collect::<Vec<_>>(), vec![1200], "block {size}: it waited through the skipped calls");
        // An event waiting when a barrier comes is discarded with the stage's retained input.
        r.between(input(64, 3, true));
        r.stop();
        r.run(2048, size, &[]);
        assert_eq!(r.fake.lock().heard.len(), 1, "block {size}: the waiting event was discarded");
    }
}
