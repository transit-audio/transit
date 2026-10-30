//! Touch mode (#188): a held control's lane is not read; on release the parameter returns to the
//! lane over `TOUCH_RETURN_S`, measured in rendered samples. Block-driven, no clock.

use super::*;
use crate::tests::{PlanRig, session_with_dc};
use crate::touch::{param_key, return_samples};
use transit_model::{AutomationLane, AutomationMode, Insert};

/// 480 frames at 48 kHz: the 9600-sample return is exactly 20 blocks, so its end lands on a block's
/// first sample, where the fader reads its curve exactly.
const BLOCK: usize = 480;
const RAMP: usize = 9_600;

/// A DC track at `static_db` in `mode` with a Volume lane at `lane_db` over the whole range, and
/// a point ahead of the playhead that would move it (`ahead_db` at 4800).
fn session(mode: AutomationMode, static_db: f32, lane_db: f32, ahead_db: f32) -> (Session, TrackId) {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.automation_mode = mode;
    tr.mixer.volume_db = static_db;
    let mut lane = AutomationLane::new(AutoParam::Volume);
    lane.set_point(0, lane_db);
    lane.set_point(4_800, ahead_db);
    lane.set_point(40_000, ahead_db);
    tr.automation.push(lane);
    (s, t)
}

fn hold(s: &mut Session, t: TrackId, ps: &[AutoParam]) {
    s.track_mut(t).unwrap().mixer.touched = ps.to_vec();
}

/// The output of the DC track at a steady `db` (no automation): what the fader's gain scales.
fn level(db: f32) -> f32 {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    s.track_mut(t).unwrap().mixer.volume_db = db;
    s.track_mut(t).unwrap().mixer.automation_mode = AutomationMode::Off;
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    rig.render(0)[0][0]
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1e-3)
}

#[test]
fn the_return_is_exactly_twenty_blocks_at_48k() {
    assert_eq!(return_samples(48_000), RAMP as u32);
    assert_eq!(RAMP % BLOCK, 0);
}

#[test]
fn a_held_volume_ignores_the_lane_and_its_points_ahead() {
    let (mut s, t) = session(AutomationMode::Touch, -6.0, 0.0, -30.0);
    hold(&mut s, t, &[AutoParam::Volume]);
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    let want = level(-6.0);
    // Through the point at 4800 and on: every sample at the held (static) value.
    for b in 0..30 {
        let out = rig.render((b * BLOCK) as i64);
        for (i, x) in out[0].iter().enumerate() {
            assert!(close(*x, want), "block {b} sample {i}: {x} vs held {want}");
        }
    }
    // Not held: the same session reads the lane (-30 dB past 4800).
    hold(&mut s, t, &[]);
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    assert!(close(rig.render(14_400)[0][0], level(-30.0)), "unheld Touch reads the lane");
}

#[test]
fn a_release_returns_to_the_lane_over_the_ramp_measured_in_samples() {
    // Lane flat at -20 dB from 4800 on; held at -6 dB.
    let (mut s, t) = session(AutomationMode::Touch, -6.0, -20.0, -20.0);
    hold(&mut s, t, &[AutoParam::Volume]);
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    let release = 10 * BLOCK;
    for b in 0..10 {
        rig.render((b * BLOCK) as i64);
    }
    // Released between blocks: the next plan carries it, as a release does in playback.
    hold(&mut s, t, &[]);
    rig.publish(&s);
    let (held, lane) = (-6.0f32, -20.0f32);
    let at = |k: usize| level(held + (lane - held) * (k as f32 / RAMP as f32));
    let mut rendered = Vec::new();
    for b in 0..(RAMP / BLOCK + 3) {
        rendered.extend_from_slice(&rig.render((release + b * BLOCK) as i64)[0]);
    }
    // The fader reads each block's first and last sample exactly; check those along the ramp.
    for b in 0..(RAMP / BLOCK) {
        for k in [b * BLOCK, b * BLOCK + BLOCK - 1] {
            assert!(close(rendered[k], at(k)), "{k} samples after the release: {} vs {}", rendered[k], at(k));
        }
    }
    let on_lane = level(lane);
    assert!(close(rendered[0], level(held)), "no jump at the release");
    assert!(!close(rendered[RAMP - 1], on_lane), "one sample before the end it is still returning");
    for (k, x) in rendered.iter().enumerate().skip(RAMP) {
        assert!(close(*x, on_lane), "{k} samples after the release: on the lane");
    }
}

#[test]
fn holding_again_during_the_return_holds_at_once() {
    let (mut s, t) = session(AutomationMode::Touch, -6.0, -20.0, -20.0);
    hold(&mut s, t, &[AutoParam::Volume]);
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    rig.render(0);
    hold(&mut s, t, &[]);
    rig.publish(&s);
    rig.render(BLOCK as i64);
    rig.render(2 * BLOCK as i64);
    hold(&mut s, t, &[AutoParam::Volume]);
    rig.publish(&s);
    let out = rig.render(3 * BLOCK as i64);
    assert!(out[0].iter().all(|x| close(*x, level(-6.0))), "held again: the static value from the block's first sample");
}

#[test]
fn latch_read_and_write_ignore_the_held_flag() {
    for (mode, db) in [(AutomationMode::Latch, -30.0), (AutomationMode::Read, -30.0), (AutomationMode::Write, -6.0)] {
        let (mut s, t) = session(mode, -6.0, 0.0, -30.0);
        hold(&mut s, t, &[AutoParam::Volume]);
        let mut rig = PlanRig::new(BLOCK);
        rig.publish(&s);
        assert!(close(rig.render(14_400)[0][0], level(db)), "{mode:?}: as before #188");
    }
}

#[test]
fn a_held_pan_ignores_its_lane() {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.automation_mode = AutomationMode::Touch;
    tr.mixer.pan = vec![0.0];
    let mut lane = AutomationLane::new(AutoParam::Pan(0));
    lane.set_point(0, -1.0);
    tr.automation.push(lane);
    let render = |s: &Session| {
        let mut rig = PlanRig::new(BLOCK);
        rig.publish(s);
        let out = rig.render(0);
        (out[0][0], out[1][0])
    };
    let (l, r) = render(&s);
    assert!(l > 0.0 && r.abs() < 1e-6, "the lane pans hard left: {l} {r}");
    hold(&mut s, t, &[AutoParam::Pan(0)]);
    let (l, r) = render(&s);
    assert!(close(l, r), "held: the static center: {l} {r}");
}

/// A built-in gain behind a third-party id that takes timed points, logging (offset, value):
/// `None` for an untimed `set_param`.
struct Timed {
    inner: Box<dyn Plugin>,
    log: Arc<std::sync::Mutex<Vec<(Option<u32>, f32)>>>,
}

impl Plugin for Timed {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.log.lock().unwrap().push((None, v));
        self.inner.set_param(id, v)
    }
    fn set_param_at(&mut self, offset: u32, id: &str, v: f32) -> bool {
        self.log.lock().unwrap().push((Some(offset), v));
        self.inner.set_param(id, v)
    }
    fn sample_accurate_params(&self) -> bool {
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
}

#[test]
fn a_held_plugin_parameter_gets_no_lane_values_and_returns_in_block_steps() {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let mut ins = Insert::new("clap:test.timed");
    ins.params.insert("gain".into(), -6.0);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.inserts[0] = Some(ins);
    tr.mixer.automation_mode = AutomationMode::Touch;
    let gain = AutoParam::Plugin { slot: 0, param: "gain".into() };
    let mut lane = AutomationLane::new(gain.clone());
    // Flat at -20 dB, with breakpoints inside many blocks.
    for k in 0..100 {
        lane.set_point(k * 1_000 + 7, -20.0);
    }
    tr.automation.push(lane);
    tr.mixer.touched = vec![gain];
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let fake = Timed { inner: transit_dsp::create("gain").unwrap(), log: Arc::clone(&log) };
    let mut rig = PlanRig::new(BLOCK).fake(t, 0, fake);
    rig.publish(&s);
    for b in 0..10 {
        rig.render((b * BLOCK) as i64);
    }
    let held = std::mem::take(&mut *log.lock().unwrap());
    assert!(held.iter().all(|c| *c == (None, -6.0)), "held: the static value only, no lane points: {held:?}");
    hold(&mut s, t, &[]);
    rig.publish(&s);
    let release = 10 * BLOCK;
    for b in 0..(RAMP / BLOCK + 2) {
        rig.render((release + b * BLOCK) as i64);
    }
    let after = std::mem::take(&mut *log.lock().unwrap());
    let untimed: Vec<f32> = after.iter().filter(|c| c.0.is_none()).map(|c| c.1).collect();
    // Block b of the return sends -6 + (-14) * (b * 480 / 9600) at its start; the first equals the
    // held value, so it is not sent again.
    for (b, v) in untimed.iter().take(RAMP / BLOCK - 1).enumerate() {
        let want = -6.0 - 14.0 * ((b + 1) * BLOCK) as f32 / RAMP as f32;
        assert!((v - want).abs() < 1e-4, "return block {}: {v} vs {want}", b + 1);
    }
    assert_eq!(untimed.last().copied(), Some(-20.0), "on the lane after the ramp: {untimed:?}");
    // No breakpoint goes out before the ramp's end (14400); the first after it is 15007, in the
    // block at 14880.
    let timed: Vec<(Option<u32>, f32)> = after.iter().filter(|c| c.0.is_some()).copied().collect();
    assert_eq!(timed.first().copied(), Some((Some(15_007 - 14_880), -20.0)), "{timed:?}");
}

#[test]
fn parameter_keys_tell_parameters_apart() {
    let ps = [
        AutoParam::Volume,
        AutoParam::Pan(0),
        AutoParam::Pan(1),
        AutoParam::SendLevel(0),
        AutoParam::Plugin { slot: 0, param: "gain".into() },
        AutoParam::Plugin { slot: 1, param: "gain".into() },
        AutoParam::Plugin { slot: 0, param: "freq".into() },
    ];
    for (i, a) in ps.iter().enumerate() {
        assert_ne!(param_key(a), 0);
        for b in ps.iter().skip(i + 1) {
            assert_ne!(param_key(a), param_key(b), "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn a_release_that_comes_with_new_plan_tables_still_returns() {
    // A track added in the same publish: the plan brings new tables and the strip moves into them.
    let (mut s, t) = session(AutomationMode::Touch, -6.0, -20.0, -20.0);
    hold(&mut s, t, &[AutoParam::Volume]);
    let mut rig = PlanRig::new(BLOCK);
    rig.publish(&s);
    rig.render(0);
    hold(&mut s, t, &[]);
    let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.track_mut(other).unwrap().mixer.mute = true;
    rig.publish(&s);
    let out = rig.render(BLOCK as i64);
    assert!(close(out[0][0], level(-6.0)), "the return starts at the held value: {} vs {}", out[0][0], level(-6.0));
}
