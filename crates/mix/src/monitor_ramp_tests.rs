//! The monitoring switch ramps (#730): toggling Audio > Monitor Armed Tracks or a track's Input
//! Monitoring button while audio sounds crossfades the strip between its input and its clips (or
//! silence while stopped) over the 5 ms switch ramp, instead of a one-sample switch. Transport
//! switches ramp too, except a clip at playback start. A track frozen while it monitors stops
//! reading its input at once, and clip effects run for the samples their clip is heard, whatever
//! the block size.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::tests::{PlanRig, session_with_dc};
use super::*;
use crate::monitor_gate::switch_samples;
use transit_audio_io::AudioBuffer;
use transit_model::{SourceAudio, SourceId};

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

/// Switches that come with the transport ramp like any other (ADR 0058 decision 15 keeps the
/// gate across Stop), except one: playback starting on a track that rested on its input plays its
/// clip at full level from the first sample.
#[test]
fn transport_switches_ramp_except_the_clip_at_playback_start() {
    let (mut rig, _, _) = rig(512, false);
    rig.eng.monitor_only = true;
    let _ = rig.render(0);
    rig.eng.monitor_only = false;
    assert!(rig.render(0)[0].iter().all(|x| (x - 0.5 * PAN).abs() < 1e-6), "playback starts: the clip from its first sample");
    rig.eng.recording = true;
    assert_ramp(&rig.render(512)[0], 0.5, 0.25, "recording starts: clip to input");
    rig.eng.recording = false;
    assert_ramp(&rig.render(1024)[0], 0.25, 0.5, "recording stops, still playing: input to clip");
    rig.eng.monitor_only = true;
    assert_ramp(&rig.render(1536)[0], 0.0, 0.25, "stopped: the input fades in (the clip stops with the transport)");
}

/// A ramp in progress when the transport stops continues from where it is.
#[test]
fn a_ramp_in_progress_continues_across_stop() {
    let (mut rig, _, _) = rig(64, false);
    rig.eng.recording = true;
    rig.eng.monitor_armed = false;
    let _ = rig.render(0);
    rig.eng.monitor_armed = true;
    let _ = rig.render(64);
    rig.eng.recording = false;
    rig.eng.monitor_only = true;
    for (i, x) in rig.render(128)[0].iter().enumerate() {
        let want = 0.25 * PAN * (64 + i + 1) as f32 / N as f32;
        assert!((x - want).abs() < 1e-6, "sample {i}: {x}, want {want}");
    }
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

/// A track frozen while it monitors its input (armed, stopped, Monitor Armed Tracks on) stops
/// reading the input on the first sample the freeze reaches it: the retained gate does not fade the
/// input out through a frozen track.
#[test]
fn a_track_frozen_while_monitoring_drops_its_input_at_once() {
    let (mut rig, mut s, t) = rig(512, false);
    rig.eng.monitor_only = true;
    let first = rig.render(0);
    assert!(first[0].iter().all(|x| (x - 0.25 * PAN).abs() < 1e-6), "monitoring before the freeze");
    let tr = s.track_mut(t).unwrap();
    tr.freeze = Some(transit_model::Frozen { source: SourceId(900), start: 0, length: 4800, hash: tr.freeze_hash(), revision: 0, latency: 0 });
    rig.publish(&s);
    let out = rig.render(0);
    assert!(out[0].iter().all(|x| *x == 0.0), "frozen, stopped: silence from the first sample, got {}", out[0][0]);
}

/// The armed track of the review's counterexample: a clip silent but for one impulse at sample
/// 1023, through a ringing clip EQ, and a live input of 0.25, rendered recording in blocks of
/// `block` with Monitor Armed Tracks off before 512, on from 512 and off again from 1536.
fn clip_fx_render(block: usize) -> Vec<u32> {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut sig = vec![0.0f32; 4096];
    sig[1023] = 1.0;
    s.pool.insert(SourceId(950), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![sig] })));
    let id = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = true;
    tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "impulse", SourceId(950), 0, 0, 4096));
    for (k, v) in [("mid_freq", 100.0), ("mid_gain", 24.0), ("mid_q", 10.0)] {
        s.edit.values.insert(format!("clip_fx.{}.eq.{k}", id.0), v);
    }
    let mut rig = PlanRig::new(block);
    rig.publish(&s);
    rig.eng.recording = true;
    rig.eng.input = vec![vec![0.25; block]];
    let mut out = Vec::new();
    for b in 0..(3072 / block) {
        let start = b * block;
        rig.eng.monitor_armed = (512..1536).contains(&start);
        out.extend(rig.render(start as i64)[0].iter().map(|x| x.to_bits()));
    }
    out
}

/// Clip effects run for exactly the samples their clip is heard: a block that starts a ramp toward
/// the input stops the clip render on the sample its weight reaches zero, so the impulse at 1023
/// never enters the clip EQ at any block size, and the output after the ramp back is the same.
#[test]
fn clip_effects_run_for_the_samples_their_clip_is_heard_at_any_block_size() {
    let reference = clip_fx_render(512);
    assert_eq!(clip_fx_render(64), reference, "64-sample blocks");
}
