//! Idle state service must reach inserts that the render loop deliberately skips.
use super::*;
use crate::tests::PlanRig;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use transit_model::Insert;

struct IdleProbe {
    seen: Arc<AtomicU32>,
    freeze: bool,
}
impl Plugin for IdleProbe {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::texture::TEXTURE_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if id == "freeze" {
            self.freeze = v >= 0.5;
        }
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
        panic!("idle insert must not process");
    }
    fn service_idle_state(&mut self) {
        assert!(self.freeze, "bypassed edits must reach the capture");
        self.seen.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn bypassed_and_inactive_inserts_and_tracks_service_state_at_the_block_boundary() {
    for mode in 0..3 {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let tr = s.track_mut(t).unwrap();
        tr.inactive = mode == 2;
        let mut ins = Insert::new("texture");
        ins.bypass = mode == 0;
        ins.active = mode != 1;
        ins.params.insert("freeze".into(), 1.0);
        tr.mixer.inserts[0] = Some(ins);
        let seen = Arc::new(AtomicU32::new(0));
        let mut rig = PlanRig::new(64).fake(t, 0, IdleProbe { seen: seen.clone(), freeze: false });
        rig.publish(&s);
        rig.render(0);
        assert_eq!(seen.load(Ordering::Relaxed), 1, "mode {mode}");
    }
}
