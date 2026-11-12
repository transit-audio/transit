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

fn review_session() -> (Session, TrackId, TrackId) {
    let mut s = Session::default();
    let a = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let b = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    (s, a, b)
}

fn review_confirms_first(s: &Session, a: TrackId) -> bool {
    let mut rig = PlanRig::new(64);
    rig.publish(s);
    for k in 0..8 {
        let _ = rig.render(k * 64);
    }
    rig.eng.parameter_delivery(a, 0) == ParameterDelivery::Ready
}

#[test]
fn review_a_deactivated_insert_on_another_track_does_not_block_confirmation() {
    let (mut s, a, b) = review_session();
    s.track_mut(b).unwrap().mixer.inserts[0].as_mut().unwrap().active = false;
    assert!(review_confirms_first(&s, a), "one deactivated insert blocks every header job in the session");
}

#[test]
fn review_an_inactive_track_does_not_block_confirmation() {
    let (mut s, a, b) = review_session();
    s.track_mut(b).unwrap().inactive = true;
    assert!(review_confirms_first(&s, a), "one inactive track blocks every header job in the session");
}

#[test]
fn own_inactive_slot_confirms_stored_values_but_failed_slot_refuses() {
    for inactive_track in [false, true] {
        let (mut s, a, b) = review_session();
        if inactive_track {
            s.track_mut(b).unwrap().inactive = true;
        } else {
            s.track_mut(b).unwrap().mixer.inserts[0].as_mut().unwrap().active = false;
        }
        let mut rig = PlanRig::new(64);
        rig.publish(&s);
        let _ = rig.render(0);
        assert_eq!(rig.eng.parameter_delivery(b, 0), ParameterDelivery::Ready);
        let p = rig.eng.plan.as_mut().unwrap();
        let i = p.tables.strip_of(b).unwrap();
        p.tables.strips[i].plugins[0].as_mut().unwrap().failed = true;
        assert_eq!(rig.eng.parameter_delivery(b, 0), ParameterDelivery::Failed);
        assert_eq!(rig.eng.parameter_delivery(a, 0), ParameterDelivery::Ready);
    }
}

#[test]
fn own_bypassed_edits_wait_while_other_instance_confirms_without_allocation() {
    let (mut s, a, b) = review_session();
    let mut rig = PlanRig::new(64).fake(
        b,
        0,
        Queued { queued: Arc::new(AtomicU32::new(0)), seen: Arc::new(AtomicU32::new(0)), calls: Arc::new(AtomicUsize::new(0)), pending: None },
    );
    rig.publish(&s);
    let _ = rig.render(0);
    let ins = s.track_mut(b).unwrap().mixer.inserts[0].as_mut().unwrap();
    ins.bypass = true;
    ins.params.insert("gain".into(), -12.0);
    rig.publish(&s);
    let _ = rig.render(64);
    let ((first, second), heap) = measure(|| (rig.eng.parameter_delivery(a, 0), rig.eng.parameter_delivery(b, 0)));
    assert_eq!(first, ParameterDelivery::Ready);
    assert_eq!(second, ParameterDelivery::Pending);
    assert!(heap.is_zero(), "per-instance acknowledgment: {heap:?}");
}

/// #296: a hosted insert's map starts empty and holds only parameters set statically. A lane on
/// a parameter the map never held must still reach the plug-in, with a map that fits the inline
/// cache and with one that needs the compiler's prepared storage, allocation-free.
#[test]
fn a_lane_on_a_parameter_absent_from_the_stored_map_reaches_the_plugin() {
    for stored in [0, APPLIED_INLINE] {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let mut ins = Insert::new("clap:test.queued");
        for k in 0..stored {
            ins.params.insert(format!("p{k:03}"), -3.0);
        }
        let tr = s.track_mut(t).unwrap();
        tr.mixer.inserts[0] = Some(ins);
        tr.mixer.automation_mode = AutomationMode::Read;
        let mut lane = AutomationLane::new(AutoParam::Plugin { slot: 0, param: "gain".into() });
        lane.set_point(0, -12.0);
        tr.automation.push(lane);
        let seen = Arc::new(AtomicU32::new(0));
        let fake = Queued { queued: Arc::new(AtomicU32::new(0)), seen: Arc::clone(&seen), calls: Arc::new(AtomicUsize::new(0)), pending: None };
        let mut rig = PlanRig::new(64).fake(t, 0, fake);
        rig.publish(&s);
        let mut out = vec![vec![0.0; 64]; 2];
        let (_, heap) = measure(|| rig.eng.render_active(0, 64, &mut out));
        assert!(heap.is_zero(), "lane-only parameter delivery: {heap:?}");
        assert_eq!(f32::from_bits(seen.load(Relaxed)), -12.0, "the lane reaches the plug-in ({stored} stored values)");
        s.track_mut(t).unwrap().automation[0].set_point(0, -6.0);
        rig.publish(&s);
        let (_, heap) = measure(|| rig.eng.render_active(64, 64, &mut out));
        assert!(heap.is_zero(), "lane-only parameter delivery: {heap:?}");
        assert_eq!(f32::from_bits(seen.load(Relaxed)), -6.0, "a lane edit reaches the plug-in ({stored} stored values)");
        assert_eq!(rig.eng.status().applied_mismatch, 0, "the prepared cache covers the lane ({stored} stored values)");
    }
}
