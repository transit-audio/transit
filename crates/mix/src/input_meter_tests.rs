//! The armed track's input meter (#730): a record-armed audio track's meter shows its input level
//! whenever the track is not playing its own material (stopped or recording), monitored or not.
//! Display only: the mix is what it was.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::tests::{PlanRig, add_dc_track, session_with_dc};
use super::*;

/// The output level of the 0.5 clip on a mono track (centre pan) and of the 0.25 input.
const CLIP: f32 = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
const INPUT: f32 = 0.25;

/// One render of the mono dc track (a 0.5 clip) after `setup`, with a live input of 0.25 unless
/// `input` is false: the main output at sample 100 and the track's meter.
#[derive(Clone, Copy, Default)]
struct Case {
    armed: bool,
    input_monitor: bool,
    monitor_armed: bool,
    stopped: bool,
    recording: bool,
    volume_db: f32,
    no_input: bool,
}

fn render(c: Case) -> (f32, StripMeter) {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = c.armed;
    tr.mixer.input_monitor = c.input_monitor;
    tr.mixer.volume_db = c.volume_db;
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    if !c.no_input {
        rig.eng.input = vec![vec![INPUT; 512]];
    }
    rig.eng.monitor_only = c.stopped;
    rig.eng.recording = c.recording;
    rig.eng.monitor_armed = c.monitor_armed;
    let out = rig.render(0)[0][100];
    let meter = rig.eng.meters().find(|(id, _)| *id == t).map(|(_, m)| m.clone()).unwrap();
    (out, meter)
}

const OFF: Case = Case { armed: true, input_monitor: false, monitor_armed: false, stopped: false, recording: false, volume_db: 0.0, no_input: false };

#[test]
fn option_off_stopped_the_meter_shows_the_input_and_the_mix_has_none() {
    let (out, meter) = render(Case { stopped: true, ..OFF });
    assert!(out.abs() < 1e-6, "the input is not heard: {out}");
    assert_eq!(meter.peak, [INPUT; 2], "the meter reads the input");
    assert!(meter.rms[0] > 0.0, "and its RMS moves");
}

#[test]
fn option_off_recording_the_meter_shows_the_input_while_the_clip_still_plays() {
    let (out, meter) = render(Case { recording: true, ..OFF });
    assert!((out - CLIP).abs() < 1e-3, "the mix plays the existing clip as before: {out}");
    assert_eq!(meter.peak, [INPUT; 2], "the meter reads the input, not the clip");
}

#[test]
fn the_input_reading_is_before_the_fader() {
    let (_, meter) = render(Case { stopped: true, volume_db: -12.0, ..OFF });
    assert_eq!(meter.peak, [INPUT; 2], "the level the take records, whatever the fader says");
}

/// A track that hears its input meters exactly as it did: the option on (stopped and recording),
/// or Input Monitoring with the option off. The reference is an unarmed track with Input
/// Monitoring, a configuration the change cannot reach; the fader at -6 dB tells the strip's own
/// post-fader reading from the raw input's.
#[test]
fn a_monitored_armed_track_meters_exactly_as_before() {
    for (stopped, recording) in [(true, false), (false, true)] {
        let base = Case { stopped, recording, volume_db: -6.0, ..Case::default() };
        let (ref_out, reference) = render(Case { input_monitor: true, ..base });
        assert!((reference.peak[0] - INPUT * db_to_gain(-6.0)).abs() < 1e-6, "post-fader: {:?}", reference.peak);
        for monitored in [Case { armed: true, monitor_armed: true, ..base }, Case { armed: true, input_monitor: true, ..base }] {
            let (out, meter) = render(monitored);
            assert_eq!(meter, reference, "stopped {stopped}, recording {recording}");
            assert_eq!(out.to_bits(), ref_out.to_bits());
        }
    }
}

/// Playing without recording, an armed track plays its material and meters it, the option on or
/// off, input present: the same reading as an unarmed track with no input.
#[test]
fn an_armed_track_playing_its_material_meters_the_material() {
    let (_, reference) = render(Case { no_input: true, ..Case::default() });
    assert!((reference.peak[0] - 0.5).abs() < 1e-6);
    for monitor_armed in [true, false] {
        let (out, meter) = render(Case { armed: true, monitor_armed, ..Case::default() });
        assert!((out - CLIP).abs() < 1e-3);
        assert_eq!(meter, reference, "option {monitor_armed}");
    }
}

/// Neither armed nor monitored: input present or not, the meter reads the strip (silence when
/// stopped, the clip while recording another track).
#[test]
fn an_unarmed_unmonitored_track_meters_as_before() {
    for (stopped, recording) in [(true, false), (false, true), (false, false)] {
        for monitor_armed in [true, false] {
            let base = Case { stopped, recording, monitor_armed, ..Case::default() };
            let (_, meter) = render(base);
            let (_, reference) = render(Case { no_input: true, ..base });
            assert_eq!(meter, reference, "stopped {stopped}, recording {recording}, option {monitor_armed}");
            let want = if stopped { 0.0 } else { 0.5 };
            assert!((meter.peak[0] - want).abs() < 1e-6, "{:?}", meter.peak);
        }
    }
}

/// The render workers' tail graph commits strips on the callback (`strip_pool::tail`): its meters
/// read the input exactly as the serial path does. Four armed tracks, so the plan takes the tail.
#[test]
fn the_tail_graph_meters_the_input_like_the_serial_path() {
    let _switch = crate::strip_pool::SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::default();
    s.edit.click = false;
    let tracks: Vec<TrackId> = (0..4).map(|_| add_dc_track(&mut s, 0.5, 48_000)).collect();
    for t in &tracks {
        s.track_mut(*t).unwrap().mixer.record_arm = true;
    }
    let setup = |rig: &mut PlanRig| {
        rig.publish(&s);
        rig.eng.input = vec![vec![INPUT; 64]];
        rig.eng.monitor_only = true;
        rig.eng.monitor_armed = false;
    };
    let mut reference = PlanRig::new(64);
    setup(&mut reference);
    let mut rig = PlanRig::new(64);
    setup(&mut rig);
    rig.eng.start_render_workers_with(64, 0);
    rig.eng.seed_strip_costs(std::time::Duration::from_nanos(1));
    rig.eng.set_strip_deadline(Some(std::time::Duration::from_secs(60)));
    MixEngine::force_graph(true);
    MixEngine::force_tail(true);
    MixEngine::force_serial(false);
    crate::render_pool::mark_audio_thread();
    let out = rig.render(0);
    crate::render_pool::unmark_audio_thread();
    MixEngine::force_tail(false);
    MixEngine::force_serial(true);
    let serial = reference.render(0);
    assert_eq!(out, serial);
    assert!(out.iter().flatten().all(|x| *x == 0.0), "stopped, unmonitored: silence");
    assert_eq!(rig.eng.strip_counts().len(), rig.eng.strip_phases().1, "the tail graph rendered the strips");
    for t in &tracks {
        let meter = |r: &PlanRig| r.eng.meters().find(|(id, _)| id == t).map(|(_, m)| m.peak).unwrap();
        assert_eq!(meter(&rig), [INPUT; 2], "tail");
        assert_eq!(meter(&reference), [INPUT; 2], "serial");
    }
}
