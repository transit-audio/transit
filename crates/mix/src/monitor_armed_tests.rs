//! Options > Monitor Armed Tracks (#730): off, arming an audio track does not monitor it; Input
//! Monitoring still does; instrument and MIDI tracks are not affected. A frozen track never
//! monitors its input.

use super::tests::{PlanRig, session_with_dc, synth_session};

const CLIP: f32 = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
const INPUT: f32 = 0.25 * std::f32::consts::FRAC_1_SQRT_2;

/// An armed track holding a 0.5 clip, with a 0.25 live input, rendered at the sample 100 of the block.
fn armed(monitor_armed: bool, monitor_only: bool, recording: bool, input_monitor: bool) -> f32 {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = true;
    tr.mixer.input_monitor = input_monitor;
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.input = vec![vec![0.25; 512]];
    rig.eng.monitor_only = monitor_only;
    rig.eng.recording = recording;
    rig.eng.monitor_armed = monitor_armed;
    rig.render(0)[0][100]
}

#[test]
fn on_an_armed_track_is_heard_stopped_and_recording() {
    assert!((armed(true, true, false, false) - INPUT).abs() < 1e-3, "stopped");
    assert!((armed(true, false, true, false) - INPUT).abs() < 1e-3, "recording");
    assert!((armed(true, false, false, false) - CLIP).abs() < 1e-3, "playing: its material");
}

#[test]
fn off_an_armed_track_does_not_monitor_its_input() {
    assert!(armed(false, true, false, false).abs() < 1e-6, "stopped: silence, no input");
    // Recording: the track plays what it holds, never the input.
    assert!((armed(false, false, true, false) - CLIP).abs() < 1e-3, "recording: the existing clip only");
    assert!((armed(false, false, false, false) - CLIP).abs() < 1e-3, "playing: its material");
}

#[test]
fn off_with_input_monitoring_on_the_input_is_heard() {
    assert!((armed(false, true, false, true) - INPUT).abs() < 1e-3, "stopped");
    assert!((armed(false, false, true, true) - INPUT).abs() < 1e-3, "recording");
}

#[test]
fn off_an_armed_instrument_track_still_sounds() {
    let mut s = synth_session(-6.0);
    let t = s.tracks[0].id;
    s.track_mut(t).unwrap().mixer.record_arm = true;
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.monitor_only = true;
    rig.eng.monitor_armed = false;
    rig.eng.live_note(t, 72, 100);
    let mut loudest = 0.0f32;
    for _ in 0..8 {
        loudest = loudest.max(rig.render(0).iter().flatten().fold(0.0f32, |m, x| m.max(x.abs())));
    }
    assert!(loudest > 0.01, "the played note sounds: {loudest}");
}

/// The frozen dc track (its render 0.5), armed, with a live input of 0.25: what it renders at
/// sample 100.
fn frozen(monitor_only: bool, recording: bool, input_monitor: bool) -> f32 {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = true;
    tr.mixer.input_monitor = input_monitor;
    // The render is the dc source itself; it is in the pool, so the freeze plays it.
    tr.freeze =
        Some(transit_model::Frozen { source: transit_model::SourceId(900), start: 0, length: 4800, hash: tr.freeze_hash(), revision: 0, latency: 0 });
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.input = vec![vec![0.25; 512]];
    rig.eng.monitor_only = monitor_only;
    rig.eng.recording = recording;
    rig.eng.monitor_armed = true;
    rig.render(0)[0][100]
}

/// With Monitor Armed Tracks on, a frozen armed track does not play its hardware input: stopped it
/// is silent, recording or playing it plays its render; Input Monitoring does not bring the input
/// in either.
#[test]
fn a_frozen_armed_track_does_not_monitor_its_input() {
    for input_monitor in [false, true] {
        assert_eq!(frozen(true, false, input_monitor), 0.0, "stopped, Input Monitoring {input_monitor}: silence");
        assert!((frozen(false, true, input_monitor) - CLIP).abs() < 1e-3, "recording, Input Monitoring {input_monitor}: the render");
        assert!((frozen(false, false, input_monitor) - CLIP).abs() < 1e-3, "playing, Input Monitoring {input_monitor}: the render");
    }
}
