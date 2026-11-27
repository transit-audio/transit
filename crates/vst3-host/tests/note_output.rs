//! ADR 0064 slice HM1 through the VST3 adapter: the fixture generator ("Fixture Notes") and the
//! zero-audio "Fixture Note Effect", prepared for a maximum native block of 16 and run in blocks
//! of 64 (four native calls a block). Each test compares what `take_note_output` hands out with
//! the fixture's own record of what it wrote and what the host called (its component state).
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

/// The fixture's input note records: (kind, offset, channel, pitch, id).
fn inputs(log: &[String]) -> Vec<(String, i64, i64, i64, i64)> {
    log.iter()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.first() == Some(&"in"))
                .then(|| (w[1].to_string(), w[2].parse().unwrap(), w[3].parse().unwrap(), w[4].parse().unwrap(), w[5].parse().unwrap()))
        })
        .collect()
}

fn make(dir: &Path, name: &str) -> Box<dyn Plugin> {
    let id = transit_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == name).unwrap().id;
    let mut p = transit_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, NATIVE, 2);
    p
}

fn generator() -> Box<dyn Plugin> {
    make(common::notes_fixture_dir(), "Fixture Notes")
}

/// Sends commands to the fixture (a component state).
fn cmd(p: &mut Box<dyn Plugin>, text: &str) {
    assert!(p.load_state(text.as_bytes()), "{text}");
}

/// The fixture's log since the last read.
fn log(p: &mut Box<dyn Plugin>) -> Vec<String> {
    let blob = p.save_state().unwrap();
    let (component, _) = transit_vst3_host::preset::split_state(&blob).unwrap();
    String::from_utf8(component.to_vec()).unwrap().lines().map(str::to_string).collect()
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

fn out(offset: u32, seq: u32, kind: OutKind, port: i32, channel: i16, key: i16, note_id: i32, values: OutValues) -> HostedOut {
    HostedOut { offset, seq, kind, port, channel, key, note_id, form: OutForm::Native, values }
}

fn on(key: u8) -> PluginEvent {
    PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE } }
}

fn written(log: &[String]) -> Vec<&String> {
    log.iter().filter(|l| l.starts_with("wrote")).collect()
}

#[test]
fn every_scripted_event_comes_out_with_its_signed_addresses_at_its_call_start_in_write_order() {
    let _serial = serial();
    let mut p = generator();
    cmd(
        &mut p,
        "CLEAR;EMIT on 0 10 0 3 60 7 0.5;EMIT off 0 2 0 3 60 7 0.25;EMIT on 1 0 0 15 127 -1 1.0;\
         EMIT pp 2 5 0 -1 64 2147483647 0.5;EMIT off 3 15 0 0 0 -7 0",
    );
    log(&mut p);
    block(&mut p);
    let (got, report) = take(&mut p);
    let rec = log(&mut p);
    assert_eq!(written(&rec).len(), 5, "the fixture wrote five events: {rec:?}");
    assert!(written(&rec).iter().all(|l| l.ends_with("-> 0")), "each accepted: {rec:?}");
    let want = vec![
        out(10, 0, OutKind::NoteOn, 0, 3, 60, 7, OutValues::Velocity(0.5)),
        out(2, 1, OutKind::NoteOff, 0, 3, 60, 7, OutValues::Velocity(0.25)),
        out(16, 2, OutKind::NoteOn, 0, 15, 127, -1, OutValues::Velocity(1.0)),
        out(37, 3, OutKind::PolyPressure, 0, -1, 64, i32::MAX, OutValues::Pressure(0.5)),
        out(63, 4, OutKind::NoteOff, 0, 0, 0, -7, OutValues::Velocity(0.0)),
    ];
    assert_eq!(got, want);
    assert_eq!((report.made, report.skipped, report.error, report.offset_clamped, report.overflow), (4, 0, false, 0, 0));
    // Taken once: the buffer is empty afterwards.
    assert!(take(&mut p).0.is_empty());
}

#[test]
fn the_first_event_bus_is_read_and_another_bus_is_dropped_and_counted() {
    let _serial = serial();
    let mut p = generator();
    assert_eq!(p.note_output(), Some(NoteOutputInfo { read: 0, declared: 1 }));
    let silent = make(common::notes_fixture_dir(), "Fixture Notes Silent");
    assert_eq!(silent.note_output(), None, "no event output bus: no readable note output");
    cmd(&mut p, "CLEAR;EMIT on 0 4 1 2 61 3 0.5;EMIT on 0 5 0 2 62 4 0.5");
    block(&mut p);
    let (got, report) = take(&mut p);
    assert_eq!(
        got,
        vec![
            out(4, 0, OutKind::Dropped(OutDrop::OtherOutput), 1, 2, 61, 3, OutValues::Velocity(0.5)),
            out(5, 1, OutKind::NoteOn, 0, 2, 62, 4, OutValues::Velocity(0.5)),
        ]
    );
    assert_eq!(report.dropped[OutDrop::OtherOutput.index()], 1);
}

#[test]
fn every_row_of_the_table_comes_out_as_the_record_says_and_each_drop_is_counted() {
    let _serial = serial();
    let mut p = generator();
    let rows = [
        ("on 0 0 0 1 60 5 0.5", OutKind::NoteOn, 1, 60, 5, OutValues::Velocity(0.5)),
        ("off 0 1 0 1 60 5 0.25", OutKind::NoteOff, 1, 60, 5, OutValues::Velocity(0.25)),
        ("pp 0 2 0 1 60 5 0.75", OutKind::PolyPressure, 1, 60, 5, OutValues::Pressure(0.75)),
        ("cc 0 3 0 7 2 17 99", OutKind::Controller, 2, -1, -1, OutValues::Controller { number: 7, value: 17 }),
        ("cc 0 4 0 128 2 17 99", OutKind::ChannelPressure, 2, -1, -1, OutValues::Controller { number: 128, value: 17 }),
        ("cc 0 5 0 130 2 17 99", OutKind::Program, 2, -1, -1, OutValues::Controller { number: 130, value: 17 }),
        ("cc 0 6 0 129 2 17 99", OutKind::Dropped(OutDrop::LegacyPitchBend), 2, -1, -1, OutValues::Controller { number: 129, value: 17 }),
        ("cc 0 7 0 131 2 17 99", OutKind::Dropped(OutDrop::LegacyPolyPressure), 2, -1, -1, OutValues::Controller { number: 131, value: 17 }),
        ("cc 0 8 0 200 2 17 99", OutKind::Dropped(OutDrop::OtherController), 2, -1, -1, OutValues::Controller { number: 200, value: 17 }),
        ("data 0 9 0 0 0 0 0", OutKind::Dropped(OutDrop::Data), -1, -1, -1, OutValues::None),
        ("expr 0 10 0 0 0 6 0.3", OutKind::Dropped(OutDrop::NoteExpression), -1, -1, 6, OutValues::None),
        ("exprint 0 11 0 0 0 6 0", OutKind::Dropped(OutDrop::NoteExpression), -1, -1, 6, OutValues::None),
        ("exprtext 0 12 0 0 0 6 0", OutKind::Dropped(OutDrop::NoteExpression), -1, -1, 6, OutValues::None),
        ("chord 0 13 0 0 0 0 0", OutKind::Dropped(OutDrop::Chord), -1, -1, -1, OutValues::None),
        ("scale 0 14 0 0 0 0 0", OutKind::Dropped(OutDrop::Scale), -1, -1, -1, OutValues::None),
    ];
    let script: Vec<String> = rows.iter().map(|r| format!("EMIT {}", r.0)).collect();
    cmd(&mut p, &format!("CLEAR;{}", script.join(";")));
    block(&mut p);
    let (got, report) = take(&mut p);
    assert_eq!(written(&log(&mut p)).len(), rows.len());
    assert_eq!(got.len(), rows.len());
    for (k, (e, r)) in got.iter().zip(rows.iter()).enumerate() {
        let want = out(k as u32, k as u32, r.1, 0, r.2, r.3, r.4, r.5);
        assert_eq!(*e, want, "row {}", r.0);
    }
    let count = |d: OutDrop| report.dropped[d.index()];
    assert_eq!(
        [
            OutDrop::LegacyPitchBend,
            OutDrop::LegacyPolyPressure,
            OutDrop::OtherController,
            OutDrop::Data,
            OutDrop::NoteExpression,
            OutDrop::Chord,
            OutDrop::Scale
        ]
        .map(count),
        [1, 1, 1, 1, 3, 1, 1],
        "each drop counted"
    );
}

#[test]
fn offsets_outside_a_call_are_clamped_into_it_and_counted_and_a_zero_frame_block_calls_nothing() {
    let _serial = serial();
    let mut p = generator();
    let mut script = vec!["CLEAR".to_string()];
    for call in 0..4 {
        for local in [-1i64, 0, 15, 16, 2_147_483_647] {
            script.push(format!("EMIT on {call} {local} 0 0 60 1 0.5"));
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
fn a_full_block_buffer_refuses_the_plug_ins_events_and_counts_them() {
    let _serial = serial();
    let mut p = generator();
    cmd(&mut p, "CLEAR;FLOOD 0 1000;FLOOD 1 100");
    block(&mut p);
    let (got, report) = take(&mut p);
    let rec = log(&mut p);
    let floods: Vec<&String> = rec.iter().filter(|l| l.starts_with("flood")).collect();
    assert_eq!(floods, ["flood 1000 0", "flood 24 76"], "the output list refused the plug-in past 1,024 events in the block");
    assert_eq!((got.len(), report.overflow), (1024, 76));
}

#[test]
fn a_skipped_call_gives_no_output_reports_itself_and_its_input_reaches_the_next_call_at_offset_0() {
    let _serial = serial();
    let mut p = generator();
    cmd(&mut p, "CLEAR;EMIT on * 3 0 0 70 1 0.5");
    log(&mut p);
    assert!(p.event(30, &on(60)));
    transit_vst3_host::while_instances_held(|| block(&mut p));
    let (got, report) = take(&mut p);
    assert!(got.is_empty(), "no output for the skipped block");
    assert_eq!((report.made, report.skipped), (0, 4));
    assert!(!log(&mut p).iter().any(|l| l.starts_with("process")));
    block(&mut p);
    let rec = log(&mut p);
    let first: Vec<String> =
        rec.iter().skip_while(|l| !l.starts_with("process 0")).skip(1).take_while(|l| !l.starts_with("process")).cloned().collect();
    assert!(
        inputs(&first).iter().any(|r| (r.0.as_str(), r.1, r.3) == ("on", 0, 60)),
        "the kept note-on at offset 0 of the next actual call: {rec:?}"
    );
    assert_eq!(take(&mut p).0.len(), 4, "the next actual block's output");
}

#[test]
fn the_zero_audio_note_effect_is_processed_with_no_audio_bus_and_its_output_comes_out() {
    let _serial = serial();
    let mut p = make(common::note_effects_fixture_dir(), "Fixture Note Effect A");
    assert_eq!(p.note_output(), Some(NoteOutputInfo { read: 0, declared: 1 }));
    cmd(&mut p, "CLEAR;EMIT on 1 3 0 4 72 9 0.5");
    log(&mut p);
    assert!(p.event(0, &on(60)));
    block(&mut p);
    let rec = log(&mut p);
    let calls: Vec<&String> = rec.iter().filter(|l| l.starts_with("process")).collect();
    assert_eq!(calls.len(), 4, "{rec:?}");
    for c in &calls {
        assert!(c.contains("n=16 in=0 out=0 inptr=0 outptr=0"), "no audio bus: {c}");
    }
    assert!(inputs(&rec).iter().any(|r| (r.0.as_str(), r.1, r.3) == ("on", 0, 60)), "its input notes arrive: {rec:?}");
    let (got, _) = take(&mut p);
    assert_eq!(got, vec![out(19, 0, OutKind::NoteOn, 0, 4, 72, 9, OutValues::Velocity(0.5))]);
}

#[test]
fn negative_positions_reach_the_plug_in_as_signed_values() {
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
    for c in calls {
        assert!(c.ends_with("pts=-48000 ptm=-2.0"), "{c}");
    }
}

#[test]
fn the_lifecycle_generation_increases_at_every_activation_attempt_and_prepare() {
    let _serial = serial();
    // `create` prepares the instance once (its first activation), and `make` prepares it again.
    let id = transit_vst3_host::scan_paths(&[common::notes_fixture_dir().to_path_buf()]).into_iter().find(|d| d.name == "Fixture Notes").unwrap().id;
    let mut p = transit_vst3_host::create(&id).unwrap();
    assert_eq!(p.lifecycle(), Lifecycle { generation: 1, active: true }, "the first activation");
    p.prepare(48_000.0, NATIVE, 2);
    assert_eq!(p.lifecycle(), Lifecycle { generation: 2, active: true }, "prepare");
    block(&mut p);
    cmd(&mut p, "LATENCY 480");
    let g = p.lifecycle().generation;
    assert_eq!(transit_vst3_host::service_requests().restarted, 1);
    assert_eq!((p.lifecycle(), p.latency()), (Lifecycle { generation: g + 1, active: true }, 480), "a restart with a changed latency");
    cmd(&mut p, "RESTART");
    let g = p.lifecycle().generation;
    assert_eq!(transit_vst3_host::service_requests().restarted, 1);
    assert_eq!((p.lifecycle(), p.latency()), (Lifecycle { generation: g + 1, active: true }, 480), "a restart with the latency unchanged");
    block(&mut p);
    let (_, report) = take(&mut p);
    assert!(report.calls().iter().all(|c| c.generation == g + 1), "each call ran under the current generation: {:?}", report.calls());
    cmd(&mut p, "REFUSE;RESTART");
    let g = p.lifecycle().generation;
    assert_eq!(transit_vst3_host::service_requests().failed, 1);
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
            transit_vst3_host::service_requests();
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
fn the_stage_reset_keeps_todays_queue_cleanup_and_cycles_processing_before_the_next_process() {
    let _serial = serial();
    let mut p = generator();
    let cc = |value| PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control { cc: 64, value } };
    assert!(p.event(0, &cc(127)));
    assert!(p.event(0, &on(60)));
    block(&mut p);
    let first = inputs(&log(&mut p));
    let id = first.iter().find(|r| r.0 == "on" && r.3 == 60).unwrap().4;
    let off = PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE } };
    assert!(p.event(0, &off));
    assert!(p.event(0, &on(62)));
    assert!(p.event(0, &cc(0)));
    p.restart_processing();
    block(&mut p);
    let rec = log(&mut p);
    let head: Vec<&str> = rec.iter().take(3).map(String::as_str).collect();
    assert_eq!(head[..2], ["setProcessing 0 -> 0", "setProcessing 1 -> 0"], "the cycle first: {rec:?}");
    assert!(head[2].starts_with("process "), "then the call: {rec:?}");
    let ins = inputs(&rec);
    assert!(ins.iter().any(|r| r.0 == "off" && r.3 == 60 && r.4 == id), "the queued release survives: {rec:?}");
    assert!(!ins.iter().any(|r| r.0 == "on" && r.3 == 62), "the queued note-on does not: {rec:?}");
    assert!(rec.iter().any(|l| l.starts_with("point 100 ") && l.ends_with(" 0")), "the release-class controller point survives: {rec:?}");
    let (_, report) = take(&mut p);
    let resets: Vec<bool> = report.calls().iter().map(|c| c.reset_before).collect();
    assert_eq!(resets, [true, false, false, false], "the reset ran before the first call's process");
    block(&mut p);
    assert!(!log(&mut p).iter().any(|l| l.starts_with("setProcessing")), "once");
}

#[test]
fn a_failed_processing_cycle_is_a_processing_error_of_that_block() {
    let _serial = serial();
    let mut p = generator();
    block(&mut p);
    cmd(&mut p, "SETPROC_ERR");
    p.restart_processing();
    let before = p.process_errors();
    block(&mut p);
    let (_, report) = take(&mut p);
    assert!(report.error);
    assert!(report.calls().first().is_some_and(|c| c.reset_before && c.error), "{:?}", report.calls());
    assert_eq!(p.process_errors(), before + 1);
}

#[test]
fn a_forgetting_plug_in_writes_no_stale_release_after_the_cycle_and_a_retaining_one_does() {
    let _serial = serial();
    for retain in [false, true] {
        let mut p = generator();
        cmd(&mut p, &format!("CLEAR;{};HOLD 0 0 0 60 9 100", if retain { "RETAIN" } else { "FORGET" }));
        block(&mut p);
        let (got, _) = take(&mut p);
        assert_eq!(got.iter().map(|e| (e.kind, e.offset)).collect::<Vec<_>>(), [(OutKind::NoteOn, 0)]);
        p.restart_processing();
        block(&mut p);
        let (got, _) = take(&mut p);
        let rec = log(&mut p);
        assert!(rec.iter().any(|l| l == "setProcessing 0 -> 0"), "{rec:?}");
        let offs: Vec<(OutKind, u32, i32)> = got.iter().map(|e| (e.kind, e.offset, e.note_id)).collect();
        if retain {
            assert_eq!(offs, [(OutKind::NoteOff, 36, 9)], "the retaining fixture writes its release afterwards");
        } else {
            assert!(offs.is_empty(), "the forgetting fixture writes no stale release: {offs:?}");
        }
    }
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
