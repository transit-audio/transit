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
    initializing: bool,
}
impl Plugin for IdleProbe {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::texture::TEXTURE_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn snap_initial_params(&mut self) {
        self.initializing = false;
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        // Stored values are initial state on the compiler thread (#280). Once preparation ends,
        // the idle audio-thread hook must still leave every pending edit except Freeze alone.
        if !self.initializing {
            assert_eq!(id, "freeze", "idle service must leave other pending edits alone");
        }
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
        assert!(!self.initializing, "preparation must finish before idle service");
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
        ins.params.insert("quality".into(), 0.0);
        ins.params.insert("buffer".into(), 4.0);
        tr.mixer.inserts[0] = Some(ins);
        let seen = Arc::new(AtomicU32::new(0));
        let mut rig = PlanRig::new(64).fake(t, 0, IdleProbe { seen: seen.clone(), freeze: false, initializing: true });
        rig.publish(&s);
        rig.render(0);
        assert_eq!(seen.load(Ordering::Relaxed), 1, "mode {mode}");
    }
}

#[test]
fn bypassed_freeze_follows_automation_and_macros_without_applying_other_edits() {
    for mapped in [false, true] {
        let mut s = Session::default();
        // Rack macros address MIDI/instrument members, never audio tracks (Session::rack_of).
        let kind = if mapped { TrackKind::Instrument } else { TrackKind::Audio };
        let t = s.add_track(kind, ChannelFormat::Stereo, None);
        let tr = s.track_mut(t).unwrap();
        let mut ins = Insert::new("texture");
        ins.bypass = true;
        ins.params.insert("freeze".into(), 0.0);
        ins.params.insert("quality".into(), 0.0);
        tr.mixer.inserts[0] = Some(ins);
        tr.mixer.automation_mode = transit_model::AutomationMode::Read;
        let mut lane = transit_model::AutomationLane::new(AutoParam::Plugin { slot: 0, param: "freeze".into() });
        lane.set_point(0, 1.0);
        tr.automation.push(lane);
        if mapped {
            use transit_model::{MACRO_SLOT, MacroMapping, Rack, RackMember, macro_param};
            s.track_mut(t).unwrap().automation.clear();
            let folder = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, None);
            s.track_mut(t).unwrap().folder = Some(folder);
            let rack = s.track_mut(folder).unwrap();
            rack.rack = Some(Rack { members: vec![RackMember::new(t)], ..Rack::default() });
            rack.rack.as_mut().unwrap().macros[0].mappings.push(MacroMapping {
                track: t,
                slot: 0,
                param: "freeze".into(),
                max: 1.0,
                ..Default::default()
            });
            rack.lane_mut(&AutoParam::Plugin { slot: MACRO_SLOT, param: macro_param(0) }).set_point(0, 1.0);
            let member = s.track(t).unwrap();
            assert!(s.rack_of(member).is_some(), "the macro fixture must be a real rack member");
            assert_eq!(crate::rack::MacroLanes::of(&s, member, 0).unwrap().value("freeze", 0), Some(1.0));
        }
        let seen = Arc::new(AtomicU32::new(0));
        let mut rig = PlanRig::new(64).fake(t, 0, IdleProbe { seen: seen.clone(), freeze: false, initializing: true });
        rig.publish(&s);
        rig.render(0);
        assert_eq!(seen.load(Ordering::Relaxed), 1);
    }
}

/// An instrument that answers idle capture like the Texture Synth: Freeze must already be applied.
struct IdleInstrument {
    seen: Arc<AtomicU32>,
    freeze: bool,
    initializing: bool,
}
impl Plugin for IdleInstrument {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::texture_synth::TEXTURE_SYNTH_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn snap_initial_params(&mut self) {
        self.initializing = false;
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if !self.initializing {
            assert_eq!(id, "freeze", "idle service must leave other pending edits alone");
        }
        if id == "freeze" {
            self.freeze = v >= 0.5;
        }
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn service_idle_state(&mut self) {
        assert!(!self.initializing, "preparation must finish before idle service");
        assert!(self.freeze, "a Freeze toggled on an inactive track must reach the capture");
        self.seen.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn an_inactive_tracks_instrument_follows_freeze_before_answering_state() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.inactive = true;
    let mut ins = Insert::new(transit_dsp::plugins::texture_synth::TEXTURE_SYNTH_INFO.id);
    ins.params.insert("freeze".into(), 1.0);
    ins.params.insert("quality".into(), 0.0);
    tr.instrument = Some(ins);
    let seen = Arc::new(AtomicU32::new(0));
    let mut rig = PlanRig::new(64).fake(t, INSTRUMENT_SLOT, IdleInstrument { seen: seen.clone(), freeze: false, initializing: true });
    rig.publish(&s);
    rig.render(0);
    assert_eq!(seen.load(Ordering::Relaxed), 1);
}
