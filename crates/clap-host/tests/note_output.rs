//! ADR 0064 slice HM1 through the CLAP adapter: the fixture generator ("Fixture Notes": output
//! note port 0 MIDI 2 only, port 1 CLAP and MIDI) and the zero-audio "Fixture Note Effect",
//! activated for a maximum native block of 16 and run in blocks of 64 (four native calls a block).
//! Each test compares what `take_note_output` hands out with the fixture's own record of what it
//! pushed and what the host called (its state).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::hosted_out::{CallReport, HostedOut, Lifecycle, NoteOutputInfo, OutDrop, OutForm, OutKind, OutValues};
use transit_dsp::{Plugin, Transport};

mod common;

const BLOCK: usize = 64;
const NATIVE: usize = 16;

/// The tests of this binary run one at a time: holding the instances (`while_instances_held`)
/// holds every live instance of the process.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn id_of(dir: &Path, name: &str) -> String {
    transit_clap_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == name).unwrap().id
}

fn make(dir: &Path, name: &str) -> Box<dyn Plugin> {
    let mut p = transit_clap_host::create(&id_of(dir, name)).unwrap();
    p.prepare(48_000.0, NATIVE, 2);
    p
}

fn generator() -> Box<dyn Plugin> {
    make(common::notes_fixture_dir(), "Fixture Notes")
}

fn effect() -> Box<dyn Plugin> {
    make(common::note_effects_fixture_dir(), "Fixture Note Effect A")
}

fn cmd(p: &mut Box<dyn Plugin>, text: &str) {
    assert!(p.load_state(text.as_bytes()), "{text}");
}

fn log(p: &mut Box<dyn Plugin>) -> Vec<String> {
    String::from_utf8(p.save_state().unwrap()).unwrap().lines().map(str::to_string).collect()
}

fn block(p: &mut Box<dyn Plugin>) {
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    p.process(&mut io, BLOCK);
}

fn take(p: &mut Box<dyn Plugin>) -> (Vec<HostedOut>, CallReport) {
    let mut v = Vec::new();
    let r = p.take_note_output(&mut |e| {
        v.push(e);
        true
    });
    (v, r)
}

fn native(offset: u32, seq: u32, kind: OutKind, port: i32, channel: i16, key: i16, note_id: i32, values: OutValues) -> HostedOut {
    HostedOut { offset, seq, kind, port, channel, key, note_id, form: OutForm::Native, values }
}

fn bytes(offset: u32, seq: u32, kind: OutKind, channel: i16, key: i16, data: [u8; 3]) -> HostedOut {
    HostedOut { offset, seq, kind, port: 1, channel, key, note_id: -1, form: OutForm::MidiBytes, values: OutValues::Midi(data) }
}

fn on(key: u8) -> PluginEvent {
    PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE } }
}

fn written(log: &[String]) -> Vec<&String> {
    log.iter().filter(|l| l.starts_with("wrote")).collect()
}

/// Runs `service_requests` and a block until a requested restart is done (the instance is
/// processing, so the first call asks the audio side to park it): the restarts and failures.
fn restart(p: &mut Box<dyn Plugin>) -> (usize, usize) {
    let mut s = transit_clap_host::service_requests();
    block(p);
    let t = transit_clap_host::service_requests();
    s.restarted += t.restarted;
    s.failed += t.failed;
    (s.restarted, s.failed)
}

#[test]
fn every_scripted_event_comes_out_with_its_signed_addresses_at_its_call_start_in_write_order() {
    let _serial = serial();
    let mut p = generator();
    cmd(
        &mut p,
        "CLEAR;EMIT on 0 10 1 3 60 7 0.5;EMIT off 0 2 1 3 60 7 0.25;EMIT on 1 0 1 15 127 -1 1.0;\
         EMIT off 2 5 -1 -1 -1 -1 0;EMIT choke 3 15 1 0 64 -7 0",
    );
    log(&mut p);
    block(&mut p);
    let (got, report) = take(&mut p);
    let rec = log(&mut p);
    assert_eq!(written(&rec).len(), 5, "{rec:?}");
    assert!(written(&rec).iter().all(|l| l.ends_with("-> true")), "each accepted: {rec:?}");
    let want = vec![
        native(10, 0, OutKind::NoteOn, 1, 3, 60, 7, OutValues::Velocity(0.5)),
        native(2, 1, OutKind::NoteOff, 1, 3, 60, 7, OutValues::Velocity(0.25)),
        native(16, 2, OutKind::NoteOn, 1, 15, 127, -1, OutValues::Velocity(1.0)),
        native(37, 3, OutKind::NoteOff, -1, -1, -1, -1, OutValues::Velocity(0.0)),
        native(63, 4, OutKind::Choke, 1, 0, 64, -7, OutValues::Velocity(0.0)),
    ];
    assert_eq!(got, want);
    assert_eq!((report.made, report.skipped, report.error, report.offset_clamped, report.overflow), (4, 0, false, 0, 0));
    assert!(take(&mut p).0.is_empty());
}

#[test]
fn the_first_readable_port_is_read_and_another_port_is_dropped_and_counted() {
    let _serial = serial();
    let mut p = generator();
    assert_eq!(p.note_output(), Some(NoteOutputInfo { read: 1, declared: 2 }), "port 1, the first readable of two");
    let silent = make(common::notes_fixture_dir(), "Fixture Notes Silent");
    assert_eq!(silent.note_output(), None, "a MIDI 2 only output is not readable");
    cmd(&mut p, "CLEAR;EMIT on 0 4 0 2 61 3 0.5;EMIT midi 0 5 0 146 62 90 0;EMIT on 0 6 1 2 63 4 0.5");
    block(&mut p);
    let (got, report) = take(&mut p);
    assert_eq!(
        got,
        vec![
            native(4, 0, OutKind::Dropped(OutDrop::OtherOutput), 0, 2, 61, 3, OutValues::Velocity(0.5)),
            HostedOut {
                offset: 5,
                seq: 1,
                kind: OutKind::Dropped(OutDrop::OtherOutput),
                port: 0,
                channel: 2,
                key: 62,
                note_id: -1,
                form: OutForm::MidiBytes,
                values: OutValues::Midi([146, 62, 90])
            },
            native(6, 2, OutKind::NoteOn, 1, 2, 63, 4, OutValues::Velocity(0.5)),
        ]
    );
    assert_eq!(report.dropped[OutDrop::OtherOutput.index()], 2);
}

#[test]
fn every_row_of_the_table_comes_out_as_the_record_says_and_each_drop_is_counted() {
    let _serial = serial();
    let mut p = generator();
    let script = [
        "on 0 0 1 1 60 5 0.5",
        "off 0 1 1 1 60 5 0.25",
        "choke 0 2 1 1 60 5 0",
        "end 0 3 1 1 60 5 0",
        "pexpr 0 4 1 1 60 5 0.75",
        "texpr 0 5 1 1 60 5 0.3",
        "midi 0 6 1 147 60 100 0",
        "midi 0 7 1 131 60 0 0",
        "midi 0 8 1 163 60 33 0",
        "midi 0 9 1 179 7 17 0",
        "midi 0 10 1 195 5 0 0",
        "midi 0 11 1 211 44 0 0",
        "midi 0 12 1 227 0 64 0",
        "sysex 0 13 1 0 0 0 0",
        "midi2 0 14 1 0 0 0 0",
        "param 0 15 0 3 0 0 0.5",
    ];
    let lines: Vec<String> = script.iter().map(|r| format!("EMIT {r}")).collect();
    cmd(&mut p, &format!("CLEAR;{}", lines.join(";")));
    block(&mut p);
    let (got, report) = take(&mut p);
    let rec = log(&mut p);
    assert_eq!(written(&rec).len(), script.len(), "{rec:?}");
    let v = OutValues::Velocity;
    let want = vec![
        native(0, 0, OutKind::NoteOn, 1, 1, 60, 5, v(0.5)),
        native(1, 1, OutKind::NoteOff, 1, 1, 60, 5, v(0.25)),
        native(2, 2, OutKind::Choke, 1, 1, 60, 5, v(0.0)),
        // NOTE_END at 3 is ignored: it reports the plug-in's own voices.
        native(4, 3, OutKind::PolyPressure, 1, 1, 60, 5, OutValues::Pressure(0.75)),
        native(5, 4, OutKind::Dropped(OutDrop::NoteExpression), 1, 1, 60, 5, OutValues::None),
        bytes(6, 5, OutKind::NoteOn, 3, 60, [147, 60, 100]),
        bytes(7, 6, OutKind::NoteOff, 3, 60, [131, 60, 0]),
        bytes(8, 7, OutKind::PolyPressure, 3, 60, [163, 60, 33]),
        bytes(9, 8, OutKind::Controller, 3, -1, [179, 7, 17]),
        bytes(10, 9, OutKind::Program, 3, -1, [195, 5, 0]),
        bytes(11, 10, OutKind::ChannelPressure, 3, -1, [211, 44, 0]),
        bytes(12, 11, OutKind::PitchBend, 3, -1, [227, 0, 64]),
        native(13, 12, OutKind::Dropped(OutDrop::Sysex), 1, -1, -1, -1, OutValues::None),
        native(14, 13, OutKind::Dropped(OutDrop::Midi2), 1, -1, -1, -1, OutValues::None),
        // The parameter value at 15 goes where editor edits go, as before: not note output.
    ];
    assert_eq!(got, want);
    let count = |d: OutDrop| report.dropped[d.index()];
    assert_eq!([OutDrop::NoteExpression, OutDrop::Sysex, OutDrop::Midi2].map(count), [1, 1, 1], "each drop counted");
}

#[test]
fn offsets_outside_a_call_are_clamped_into_it_and_counted_and_a_zero_frame_block_calls_nothing() {
    let _serial = serial();
    let mut p = generator();
    let mut script = vec!["CLEAR".to_string()];
    for call in 0..4 {
        for local in [-1i64, 0, 15, 16, 2_147_483_647] {
            script.push(format!("EMIT on {call} {local} 1 0 60 1 0.5"));
        }
    }
    cmd(&mut p, &script.join(";"));
    block(&mut p);
    let (got, report) = take(&mut p);
    let offsets: Vec<u32> = got.iter().map(|e| e.offset).collect();
    let want: Vec<u32> = (0..4u32).flat_map(|c| [0, 0, 15, 15, 15].map(|o| 16 * c + o)).collect();
    assert_eq!(offsets, want);
    assert_eq!(report.offset_clamped, 12, "three of five clamped in each call");
    log(&mut p);
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    p.process(&mut io, 0);
    let (got, report) = take(&mut p);
    assert!(got.is_empty());
    assert_eq!((report.made, report.skipped), (0, 0));
    assert!(!log(&mut p).iter().any(|l| l.starts_with("process")), "no native call");
}

#[test]
fn a_full_block_buffer_makes_try_push_return_false_and_counts() {
    let _serial = serial();
    let mut p = generator();
    cmd(&mut p, "CLEAR;FLOOD 0 1000;FLOOD 1 100");
    block(&mut p);
    let (got, report) = take(&mut p);
    let rec = log(&mut p);
    let floods: Vec<&String> = rec.iter().filter(|l| l.starts_with("flood")).collect();
    assert_eq!(floods, ["flood 1000 0", "flood 24 76"], "try_push returned false past 1,024 events in the block");
    assert_eq!((got.len(), report.overflow), (1024, 76));
}

#[test]
fn a_skipped_call_gives_no_output_reports_itself_and_its_input_reaches_the_next_call_at_offset_0() {
    let _serial = serial();
    let mut p = generator();
    cmd(&mut p, "CLEAR;EMIT on * 3 1 0 70 1 0.5");
    log(&mut p);
    assert!(p.event(30, &on(60)));
    transit_clap_host::while_instances_held(|| block(&mut p));
    let (got, report) = take(&mut p);
    assert!(got.is_empty(), "no output for the skipped block");
    assert_eq!((report.made, report.skipped), (0, 4));
    assert!(!log(&mut p).iter().any(|l| l.starts_with("process")));
    block(&mut p);
    let rec = log(&mut p);
    let first: Vec<&String> = rec.iter().skip_while(|l| !l.starts_with("process 0")).skip(1).take_while(|l| !l.starts_with("process")).collect();
    assert!(first.iter().any(|l| l.starts_with("in on 0 0 0 60 ")), "the kept note-on at offset 0 of the next actual call: {rec:?}");
    assert_eq!(take(&mut p).0.len(), 4);
}

#[test]
fn the_zero_audio_note_effect_is_processed_with_no_audio_port_hears_the_clap_dialect_and_its_output_comes_out() {
    let _serial = serial();
    let mut p = effect();
    assert_eq!(p.note_output(), Some(NoteOutputInfo { read: 0, declared: 1 }));
    cmd(&mut p, "CLEAR;EMIT on 1 3 0 4 72 9 0.5");
    log(&mut p);
    assert!(p.event(0, &on(60)));
    block(&mut p);
    let rec = log(&mut p);
    let calls: Vec<&String> = rec.iter().filter(|l| l.starts_with("process")).collect();
    assert_eq!(calls.len(), 4, "{rec:?}");
    for c in &calls {
        assert!(c.contains("n=16 ain=0 aout=0"), "no audio port: {c}");
    }
    assert!(rec.iter().any(|l| l.starts_with("in on 0 0 0 60 ")), "a CLAP note event (the port offers CLAP and MIDI): {rec:?}");
    assert!(!rec.iter().any(|l| l.starts_with("in midi")), "{rec:?}");
    let (got, _) = take(&mut p);
    assert_eq!(got, vec![native(19, 0, OutKind::NoteOn, 0, 4, 72, 9, OutValues::Velocity(0.5))]);
}

#[test]
fn an_input_port_that_drops_the_clap_dialect_across_a_restart_gets_midi_from_the_first_call_after_it() {
    let _serial = serial();
    let mut p = effect();
    block(&mut p);
    cmd(&mut p, "DIALECT_MIDI_ONLY;RESTART");
    let g = p.lifecycle().generation;
    assert_eq!(restart(&mut p), (1, 0));
    assert_eq!(p.lifecycle().generation, g + 1, "the restart is a new generation");
    log(&mut p);
    assert!(p.event(0, &on(60)));
    block(&mut p);
    let rec = log(&mut p);
    let first: Vec<&String> = rec.iter().skip_while(|l| !l.starts_with("process")).skip(1).take_while(|l| !l.starts_with("process")).collect();
    assert!(first.iter().any(|l| *l == "in midi 0 144 60 100"), "MIDI from the first call after the restart: {rec:?}");
    assert!(!rec.iter().any(|l| l.starts_with("in on")), "nothing in the old dialect: {rec:?}");
}

#[test]
fn negative_positions_reach_the_plug_in_as_signed_fixed_point_values() {
    let _serial = serial();
    let mut p = generator();
    let t = Transport {
        playing: true,
        recording: false,
        tempo: 120.0,
        numerator: 4,
        denominator: 4,
        samples: -48_000,
        seconds: -1.0,
        quarters: -2.0,
        bar_start: -4.0,
        bar: -1,
    };
    p.set_transport(&t);
    log(&mut p);
    block(&mut p);
    let rec = log(&mut p);
    let calls: Vec<&String> = rec.iter().filter(|l| l.starts_with("process")).collect();
    assert_eq!(calls.len(), 4);
    // CLAP's fixed point: 2^31 per beat and per second.
    let (beats, secs) = (-2 * (1i64 << 31), -(1i64 << 31));
    for c in calls {
        assert!(c.ends_with(&format!("beats={beats} secs={secs}")), "{c}");
    }
}

#[test]
fn the_lifecycle_generation_increases_at_every_activation_attempt_and_prepare() {
    let _serial = serial();
    let mut p = transit_clap_host::create(&id_of(common::notes_fixture_dir(), "Fixture Notes")).unwrap();
    assert_eq!(p.lifecycle(), Lifecycle { generation: 1, active: true }, "the first activation");
    p.prepare(48_000.0, NATIVE, 2);
    assert_eq!(p.lifecycle(), Lifecycle { generation: 2, active: true }, "prepare");
    block(&mut p);
    cmd(&mut p, "LATENCY 480");
    let g = p.lifecycle().generation;
    assert_eq!(restart(&mut p), (1, 0));
    assert_eq!((p.lifecycle(), p.latency()), (Lifecycle { generation: g + 1, active: true }, 480), "a restart with a changed latency");
    cmd(&mut p, "RESTART");
    let g = p.lifecycle().generation;
    assert_eq!(restart(&mut p), (1, 0));
    assert_eq!((p.lifecycle(), p.latency()), (Lifecycle { generation: g + 1, active: true }, 480), "a restart with the latency unchanged");
    block(&mut p);
    let (_, report) = take(&mut p);
    assert_eq!(report.made, 4);
    assert!(report.calls().iter().all(|c| c.generation == g + 1), "each call ran under the current generation: {:?}", report.calls());
    cmd(&mut p, "REFUSE;RESTART");
    let g = p.lifecycle().generation;
    assert_eq!(restart(&mut p), (0, 1));
    assert_eq!(p.lifecycle(), Lifecycle { generation: g + 1, active: false }, "a refused reactivation");
    block(&mut p);
    assert_eq!(take(&mut p).1.skipped, 4, "an inactive instance's calls are skipped");
}

#[test]
fn the_lifecycle_generation_increases_at_each_state_load_on_both_paths_also_while_calls_are_skipped() {
    let _serial = serial();
    let mut p = generator();
    block(&mut p);
    for skipped in [false, true] {
        if skipped {
            cmd(&mut p, "REFUSE;RESTART");
            restart(&mut p);
            assert!(!p.lifecycle().active);
        }
        let g = p.lifecycle().generation;
        cmd(&mut p, "CLEAR");
        assert_eq!(p.lifecycle().generation, g + 1, "a direct load (skipped: {skipped})");
        block(&mut p);
        assert_eq!(take(&mut p).1.skipped, if skipped { 4 } else { 0 });
        let mut reader = p.state_reader().unwrap();
        assert!(reader.load_state(b"CLEAR").is_some());
        assert_eq!(p.lifecycle().generation, g + 2, "a load through the state reader (skipped: {skipped})");
        block(&mut p);
        let (_, report) = take(&mut p);
        assert_eq!(report.skipped, if skipped { 4 } else { 0 });
        if !skipped {
            assert!(report.calls().iter().all(|c| c.generation == g + 2), "the calls after the load ran under it");
        }
    }
}

#[test]
fn the_stage_reset_requests_the_plug_ins_reset_before_the_next_call() {
    let _serial = serial();
    let mut p = generator();
    block(&mut p);
    log(&mut p);
    p.restart_processing();
    block(&mut p);
    let rec = log(&mut p);
    let reset = rec.iter().position(|l| l == "reset").expect("a reset");
    let process = rec.iter().position(|l| l.starts_with("process")).unwrap();
    assert!(reset < process, "before the next call's process: {rec:?}");
    let (_, report) = take(&mut p);
    let resets: Vec<bool> = report.calls().iter().map(|c| c.reset_before).collect();
    assert_eq!(resets, [true, false, false, false]);
    block(&mut p);
    assert!(!log(&mut p).iter().any(|l| l == "reset"), "once");
}

#[test]
fn a_processing_error_is_reported_in_the_block_it_happened() {
    let _serial = serial();
    let mut p = generator();
    cmd(&mut p, "CLEAR;FAIL 1");
    block(&mut p);
    let (_, report) = take(&mut p);
    assert!(report.error);
    let errors: Vec<bool> = report.calls().iter().map(|c| c.error).collect();
    assert_eq!(errors, [false, true], "the second call failed and the block stopped there");
}
