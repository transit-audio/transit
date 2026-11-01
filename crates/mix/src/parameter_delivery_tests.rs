//! #219: bypass queues values without processing; the first active block consumes them.
use super::*;
use crate::tests::PlanRig;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};
use transit_model::{AutomationLane, AutomationMode, Insert};
use transit_rt::testing::measure;

struct Queued {
    queued: Arc<AtomicU32>,
    seen: Arc<AtomicU32>,
    calls: Arc<AtomicUsize>,
    pending: Option<f32>,
}

impl Plugin for Queued {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        transit_dsp::plugin_info("gain").unwrap()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, value: f32) -> bool {
        self.pending = Some(value);
        self.queued.store(value.to_bits(), Relaxed);
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        Some(f32::from_bits(self.seen.load(Relaxed)))
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls.fetch_add(1, Relaxed);
        if let Some(value) = self.pending.take() {
            self.seen.store(value.to_bits(), Relaxed);
        }
        let value = f32::from_bits(self.seen.load(Relaxed));
        for channel in io {
            channel.iter_mut().take(frames).for_each(|v| *v = value);
        }
    }
    fn params_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[test]
fn bypass_receives_automation_and_first_active_block_uses_it_without_another_lane_change() {
    for kind in [TrackKind::Audio, TrackKind::Master] {
        let mut s = Session::default();
        let t = s.add_track(kind, ChannelFormat::Stereo, None);
        let mut ins = Insert::new("clap:test.queued");
        ins.params.insert("gain".into(), -3.0);
        let tr = s.track_mut(t).unwrap();
        tr.mixer.inserts[0] = Some(ins);
        tr.mixer.automation_mode = AutomationMode::Read;
        let mut lane = AutomationLane::new(AutoParam::Plugin { slot: 0, param: "gain".into() });
        lane.set_point(0, -3.0);
        tr.automation.push(lane);
        let queued = Arc::new(AtomicU32::new(0));
        let seen = Arc::new(AtomicU32::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut rig =
            PlanRig::new(64).fake(t, 0, Queued { queued: Arc::clone(&queued), seen: Arc::clone(&seen), calls: Arc::clone(&calls), pending: None });
        rig.publish(&s);
        let mut out = vec![vec![0.0; 64]; 2];
        rig.eng.render_active(0, 64, &mut out);
        let first = out.clone();
        assert_eq!(f32::from_bits(seen.load(Relaxed)), -3.0);
        s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().bypass = true;
        rig.publish(&s);
        for value in [-6.0, -12.0] {
            s.track_mut(t).unwrap().automation[0].set_point(0, value);
            rig.publish(&s);
            let (_, heap) = measure(|| rig.eng.render_active(0, 64, &mut out));
            assert!(heap.is_zero(), "bypassed parameter delivery: {heap:?}");
            assert_eq!(f32::from_bits(queued.load(Relaxed)), value, "the bypassed host receives the lane value");
            assert_eq!(f32::from_bits(seen.load(Relaxed)), -3.0);
            assert_eq!(calls.load(Relaxed), 1, "bypass stays dry");
        }
        // Only bypass changes. The lane is left untouched from this point onward.
        s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().bypass = false;
        rig.publish(&s);
        let (_, heap) = measure(|| rig.eng.render_active(0, 64, &mut out));
        assert!(heap.is_zero(), "reactivation: {heap:?}");
        assert_eq!(calls.load(Relaxed), 2);
        assert_eq!(f32::from_bits(seen.load(Relaxed)), -12.0, "first active process call");
        assert!(out.iter().flatten().zip(first.iter().flatten()).all(|(v, before)| *v == before * 4.0), "no stale first sample");
    }
}
