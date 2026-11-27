//! The monitoring switch ramps (#730): toggling Audio > Monitor Armed Tracks or a track's Input
//! Monitoring button while audio sounds crossfades the strip between its input and its clips (or
//! silence while stopped) over the 5 ms switch ramp, instead of a one-sample switch. A switch that
//! comes with the transport (play, record, stop) is not ramped.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::tests::{PlanRig, session_with_dc};
use super::*;
use crate::monitor_gate::switch_samples;

/// The switch ramp at the rigs' 48 kHz: 5 ms.
const N: usize = 240;
/// Main's gain on the mono dc track (center pan, -3 dB law: 10^(-3/20)).
const PAN: f32 = 0.707_945_8;

/// The armed dc track (a 0.5 clip), published on a rig of `block` samples with a live input of 0.25.
fn rig(block: usize, input_monitor: bool) -> (PlanRig, Session, TrackId) {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = true;
    tr.mixer.input_monitor = input_monitor;
    let mut rig = PlanRig::new(block);
    rig.publish(&s);
    rig.eng.input = vec![vec![0.25; block]];
    (rig, s, t)
}

/// `out` follows the linear crossfade from `from` to `to` (source levels) that starts on its first
/// sample: weight `(i + 1) / N` toward `to` at sample `i`, `to` from sample `N - 1` on.
fn assert_ramp(out: &[f32], from: f32, to: f32, what: &str) {
    for (i, x) in out.iter().enumerate() {
        let w = ((i + 1).min(N) as f32) / N as f32;
        let want = (from * (1.0 - w) + to * w) * PAN;
        assert!((x - want).abs() < 1e-6, "{what}: sample {i} is {x}, want {want}");
    }
    // No step larger than one ramp increment: the switch is not a one-sample jump.
    let step = (to - from).abs() * PAN / N as f32 + 1e-6;
    for (i, w) in out.windows(2).enumerate() {
        assert!((w[1] - w[0]).abs() <= step, "{what}: step at {i}: {} -> {}", w[0], w[1]);
    }
}

#[test]
fn the_switch_ramp_is_five_milliseconds_rounded_half_up() {
    assert_eq!(switch_samples(48_000), N as u32);
    assert_eq!(switch_samples(44_100), 221);
    assert_eq!(switch_samples(96_000), 480);
    assert_eq!(switch_samples(768_000), 3_840);
}

/// Stopped, an armed track with the option on hears its input; turning the option off fades the
/// input out over 5 ms, and turning it back on fades it in.
#[test]
fn toggling_monitor_armed_tracks_while_stopped_ramps() {
    let (mut rig, _, _) = rig(512, false);
    rig.eng.monitor_only = true;
    let first = rig.render(0);
    assert!(first[0].iter().all(|x| (x - 0.25 * PAN).abs() < 1e-6), "at rest: the input, from the first block");
    rig.eng.monitor_armed = false;
    assert_ramp(&rig.render(0)[0], 0.25, 0.0, "option off");
    assert!(rig.render(0)[0].iter().all(|x| *x == 0.0), "then silence");
    rig.eng.monitor_armed = true;
    assert_ramp(&rig.render(0)[0], 0.0, 0.25, "option on");
}

/// Playing, turning a track's Input Monitoring on (a new published session) crossfades from its
/// clip to its input over 5 ms, and off again back to the clip: the ramp survives the new plan.
#[test]
fn toggling_input_monitoring_while_playing_ramps_across_the_new_plan() {
    let (mut rig, mut s, t) = rig(512, false);
    let first = rig.render(0);
    assert!(first[0].iter().all(|x| (x - 0.5 * PAN).abs() < 1e-6), "playing: the clip");
    s.track_mut(t).unwrap().mixer.input_monitor = true;
    rig.publish(&s);
    assert_ramp(&rig.render(512)[0], 0.5, 0.25, "Input Monitoring on");
    assert!(rig.render(1024)[0].iter().all(|x| (x - 0.25 * PAN).abs() < 1e-6), "then the input");
    s.track_mut(t).unwrap().mixer.input_monitor = false;
    rig.publish(&s);
    assert_ramp(&rig.render(1536)[0], 0.25, 0.5, "Input Monitoring off");
}

/// Input Monitoring turned on in the same publish as a structural edit (a new track: new plan
/// tables, so the strip's state moves into a new shell) still ramps from where the gate was.
#[test]
fn a_switch_published_with_new_tables_ramps_from_the_moved_gate() {
    let (mut rig, mut s, t) = rig(512, false);
    let _ = rig.render(0);
    s.track_mut(t).unwrap().mixer.input_monitor = true;
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    rig.publish(&s);
    assert_ramp(&rig.render(512)[0], 0.5, 0.25, "Input Monitoring on with a new track");
}

/// A switch that comes with the transport is not ramped: starting playback on an armed track
/// monitored while stopped plays its clip from the first sample, starting and stopping a recording
/// switch on the first sample, and stopping goes back to the input at once.
#[test]
fn a_switch_with_the_transport_is_not_ramped() {
    let (mut rig, _, _) = rig(512, false);
    rig.eng.monitor_only = true;
    let _ = rig.render(0);
    rig.eng.monitor_only = false;
    assert!(rig.render(0)[0].iter().all(|x| (x - 0.5 * PAN).abs() < 1e-6), "playback starts: the clip from its first sample");
    rig.eng.recording = true;
    assert!(rig.render(512)[0].iter().all(|x| (x - 0.25 * PAN).abs() < 1e-6), "recording starts: the input from the first sample");
    rig.eng.recording = false;
    assert!(rig.render(1024)[0].iter().all(|x| (x - 0.5 * PAN).abs() < 1e-6), "recording stops, still playing: the clip at once");
    rig.eng.monitor_only = true;
    assert!(rig.render(1536)[0].iter().all(|x| (x - 0.25 * PAN).abs() < 1e-6), "stopped: the input at once");
}

/// A switch reversed mid-ramp turns back from where the gate is, at the same slope.
#[test]
fn a_switch_reversed_mid_ramp_turns_back_from_where_it_is() {
    let (mut rig, _, _) = rig(64, false);
    rig.eng.monitor_only = true;
    let _ = rig.render(0);
    rig.eng.monitor_armed = false;
    let down = rig.render(0);
    rig.eng.monitor_armed = true;
    let up = rig.render(0);
    // 64 samples down (the gate at N - 64), then back up from there.
    assert!((down[0][63] - 0.25 * PAN * (N - 64) as f32 / N as f32).abs() < 1e-6);
    for (i, x) in up[0].iter().enumerate() {
        let want = 0.25 * PAN * (N - 64 + i + 1) as f32 / N as f32;
        assert!((x - want).abs() < 1e-6, "sample {i}: {x}, want {want}");
    }
}

/// The same switches at the same samples render the same bits whatever the block size: the gate's
/// weight depends on the timeline sample, not on where the blocks fall.
#[test]
fn the_ramp_does_not_depend_on_the_block_size() {
    let render = |block: usize| {
        let (mut rig, _, _) = rig(block, false);
        rig.eng.monitor_only = true;
        let mut out = Vec::new();
        for b in 0..(2048 / block) {
            let start = b * block;
            // A distinct input at every sample, so a shifted ramp cannot hide.
            rig.eng.input = vec![(start..start + block).map(|n| 0.25 + (n % 97) as f32 * 1e-3).collect()];
            rig.eng.monitor_armed = !(512..1536).contains(&start);
            out.extend(rig.render(0)[0].iter().map(|x| x.to_bits()));
        }
        out
    };
    let reference = render(512);
    assert_eq!(render(64), reference, "64-sample blocks");
    assert_eq!(render(256), reference, "256-sample blocks");
}
