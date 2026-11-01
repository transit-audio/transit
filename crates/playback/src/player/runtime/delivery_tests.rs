//! #219: a deterministic skipped host block cannot confirm a live state read.
#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use transit_dsp::{JobPoll, Plugin, PluginInfo};
use transit_model::{ChannelFormat, Insert, TrackKind};
use transit_rt::testing::measure;

struct Host {
    reject: Arc<AtomicBool>,
    skip: Arc<AtomicBool>,
    value: Arc<AtomicU32>,
    queued: Option<f32>,
}
impl Plugin for Host {
    fn info(&self) -> &'static PluginInfo {
        transit_dsp::plugin_info("gain").unwrap()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, v: f32) -> bool {
        if self.reject.load(Relaxed) {
            return false;
        }
        self.queued = Some(v);
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        Some(f32::from_bits(self.value.load(Relaxed)))
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
        if !self.skip.load(Relaxed)
            && let Some(v) = self.queued.take()
        {
            self.value.store(v.to_bits(), Relaxed);
        }
    }
    fn params_pending(&self) -> bool {
        self.queued.is_some()
    }
    fn state_reader(&mut self) -> Option<Box<dyn transit_dsp::PluginStateReader>> {
        let value = Arc::clone(&self.value);
        Some(Box::new(move || Some(value.load(Relaxed).to_le_bytes().to_vec())))
    }
}

#[test]
fn confirmation_waits_for_the_host_to_process_the_value_without_blocking() {
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let mut ins = Insert::new("gain");
    ins.params.insert("gain".into(), -12.0);
    session.track_mut(track).unwrap().mixer.inserts[0] = Some(ins);
    let session = Arc::new(session);
    let target = RenderTarget { device: transit_mix::DeviceGeneration(1), sample_rate: 48_000, max_block: 64, out_channels: 0, in_channels: 0 };
    let (mut runtime, mut ports) = RuntimeControl::<u8, u8>::new(Arc::clone(&session), None, target, PlanLimits::default()).unwrap();
    let skip = Arc::new(AtomicBool::new(true));
    let reject = Arc::new(AtomicBool::new(true));
    let value = Arc::new(AtomicU32::new(0.0f32.to_bits()));
    let handles = &mut runtime.handles;
    runtime.control.service(&mut |spec, generation| {
        handles.capture(
            (spec.track, spec.slot, generation),
            NewInstance {
                plugin: Box::new(Host { reject: Arc::clone(&reject), skip: Arc::clone(&skip), value: Arc::clone(&value), queued: None }),
                refused_state: false,
            },
        )
    });
    runtime.handles.refresh(&mut runtime.control.sessions);
    ports.plans.service(&mut plan_bridge::callback_scope(), true);
    runtime.service();
    assert!(!runtime.plays_processed(&session));
    ports.plans.render(0, 64);
    runtime.service();
    assert!(!runtime.plays_processed(&session), "a rejected setter is not delivery even with an empty queue");
    reject.store(false, Relaxed);
    ports.plans.render(0, 64);
    runtime.service();
    assert!(runtime.plays(&session), "the old API still confirms a rendered document");
    let (ready, heap) = measure(|| runtime.plays_processed(&session));
    assert!(!ready, "the host still owns the undelivered value");
    assert!(heap.is_zero());
    // The next call is reachable while the host is held: confirmation did not wait or spin.
    skip.store(false, Relaxed);
    assert!(!runtime.plays_processed(&session), "release alone is not delivery");
    ports.plans.render(0, 64);
    runtime.service();
    assert!(runtime.plays_processed(&session));
    let mut save = runtime.begin_save_state(track, 0).unwrap();
    assert!(matches!(save.poll(), JobPoll::Ready(Some(data)) if data == (-12.0f32).to_bits().to_le_bytes()));
    runtime.retire(ports);
}
