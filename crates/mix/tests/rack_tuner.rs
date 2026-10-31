//! A Tuner on a rack member gets its sample tap like one on any track (#266): the tap is made on
//! the compiler thread with the instance, and the member's strip fills it while it is enabled.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::float_cmp)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, SlotGen, create_instance,
    mark_audio_thread,
};
use transit_model::{ChannelFormat, Clip, Insert, Rack, RackMember, Route, Session, SourceAudio, SourceId, TrackId, TrackKind, Zone};

const SR: u32 = 48_000;
const BLOCK: usize = 256;

#[test]
fn a_tuner_on_a_rack_member_gets_a_sample_tap_and_hears_its_member() {
    let target = RenderTarget { device: DeviceGeneration(3), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let mut s = Session::default();
    let bus = s.add_bus("Rack Bus", ChannelFormat::Stereo);
    let folder = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Rack"));
    s.track_mut(folder).unwrap().mixer.input = Route::Bus(bus);
    let member = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Member"));
    let len = SR as usize * 2;
    let sine: Vec<f32> = (0..len).map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / SR as f32).sin() * 0.5).collect();
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![sine.clone(), sine] })));
    let id = s.new_clip_id();
    let tuner = Insert::new("tuner");
    let uid = tuner.uid.get();
    let rack_tuner = Insert::new("tuner");
    let rack_uid = rack_tuner.uid.get();
    s.track_mut(folder).unwrap().mixer.inserts[0] = Some(rack_tuner);
    {
        let tr = s.track_mut(member).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "sine", SourceId(1), 0, 0, len as i64));
        tr.mixer.inserts[0] = Some(tuner);
        tr.folder = Some(folder);
        tr.mixer.output = Route::Bus(bus);
    }
    let rm = RackMember { track: member, key_zone: Zone::FULL, vel_zone: Zone::FULL };
    s.track_mut(folder).unwrap().rack = Some(Rack { members: vec![rm], ..Rack::default() });
    assert_ne!(member, TrackId(0));

    mark_audio_thread();
    MixEngine::force_serial(true);
    let mut mix = MixEngine::realtime(target);
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch);
    let mut env = comp.compile(Arc::new(s.clone()), comp.active(), PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    let hub = comp.scope_hub();
    assert!(hub.get_samples(member, uid).is_some(), "the member's Tuner has a sample tap");
    assert!(hub.set_enabled(member, uid, true));
    assert!(hub.get_samples(folder, rack_uid).is_some(), "the rack's own Tuner has a sample tap");
    assert!(hub.set_enabled(folder, rack_uid, true));
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut pos = 0i64;
    for _ in 0..40 {
        mix.render_active(pos, BLOCK, &mut out);
        pos += BLOCK as i64;
    }
    let mut got = vec![0.0f32; 2048];
    assert!(hub.samples(member, uid, &mut got) > 0);
    assert!(got.iter().any(|x| *x > 0.1) && got.iter().any(|x| *x < -0.1), "the member's sine reaches the tap");
    assert!(hub.samples(folder, rack_uid, &mut got) > 0);
    assert!(got.iter().any(|x| *x > 0.1) && got.iter().any(|x| *x < -0.1), "the rack's sum reaches the rack's tap");
}
