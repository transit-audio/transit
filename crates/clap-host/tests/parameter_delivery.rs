//! #219: block-start values have reserved room beside MIDI, including at the FFI boundary.
#![allow(clippy::unwrap_used)]
use transit_dsp::event::{Message, NoteId, PluginEvent};

mod common;

#[test]
fn a_refused_processing_start_keeps_the_value_for_the_first_processed_block() {
    common::fixture_dir();
    let mut plugin = transit_clap_host::create("clap:org.soundcraft.test.gain").unwrap();
    plugin.prepare(48_000.0, 256, 2);
    assert!(plugin.load_state(b"CMD:refuse-start-once"));
    assert!(plugin.set_param("7", 1.5));
    let mut io = vec![vec![0.5; 256]; 2];
    plugin.process(&mut io, 256);
    assert!(plugin.params_pending(), "a refused start did not consume any parameter");
    io.iter_mut().for_each(|c| c.fill(0.5));
    plugin.process(&mut io, 256);
    assert!(!plugin.params_pending());
    assert!(io.iter().all(|c| c[96..].iter().all(|v| (*v - 0.75).abs() < 1e-6)));
}

#[test]
fn a_parameter_and_all_releases_reach_the_first_block_after_midi_saturation() {
    common::events_fixture_dir();
    let mut plugin = transit_clap_host::create("clap:org.soundcraft.test.events").unwrap();
    plugin.prepare(48_000.0, 8192, 2);
    let pressure = PluginEvent { channel: 0, note_id: -1, message: Message::PolyPressure { key: 60, value: 1, id: NoteId::NONE } };
    for _ in 0..767 {
        assert!(plugin.event(1, &pressure));
    }
    assert!(!plugin.event(1, &pressure));
    let release = PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 64, value: 0 } };
    for _ in 0..257 {
        assert!(plugin.event(2, &release));
    }
    // No process calls while these values change, just as with a bypassed insert.
    assert!(plugin.set_param("5", 0.25));
    assert!(plugin.set_param("5", 0.75));
    assert!(plugin.params_pending());
    let mut io = vec![vec![0.0; 8192]; 2];
    plugin.process(&mut io, 8192);
    let records: Vec<_> = io[0].as_chunks::<4>().0.iter().take_while(|r| r[0] != 0.0).collect();
    assert_eq!(records.len(), 1025, "FFI must include the reserved value and every release");
    assert_eq!(*records[0], [4.0, 0.0, 5.0, 750_000.0], "latest value at the start of the first block");
    assert!(!plugin.params_pending());
}

#[test]
fn a_plugin_reported_process_error_does_not_replay_its_events() {
    common::events_fixture_dir();
    let mut plugin = transit_clap_host::create("clap:org.soundcraft.test.events").unwrap();
    plugin.prepare(48_000.0, 8192, 2);
    assert!(plugin.load_state(b"CMD:process-error-once"));
    let note = PluginEvent { channel: 0, note_id: -1, message: Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE } };
    assert!(plugin.event(10, &note));
    assert!(plugin.set_param("5", 0.5));
    let mut io = vec![vec![0.0; 8192]; 2];
    plugin.process(&mut io, 8192);
    plugin.process(&mut io, 8192);
    assert_eq!(io[0][8191], 0.0, "note-ons delivered again after the plug-in's error");
    assert!(!plugin.params_pending(), "events delivered to an error-returning plug-in are consumed");
}
