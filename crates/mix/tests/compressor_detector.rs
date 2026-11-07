//! The Compressor's detector level reaches the panel through the scope hub (#326, the operating
//! point): the strip publishes what the plug-in's detector measured with the tap's other values,
//! and the hub reads it back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::float_cmp)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, SlotGen, create_instance,
    mark_audio_thread,
};
use transit_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};

const SR: u32 = 48_000;
const BLOCK: usize = 256;

#[test]
fn the_hub_reads_back_the_compressors_detector_level() {
    let target = RenderTarget { device: DeviceGeneration(3), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Drums"));
    let len = SR as usize * 2;
    // A constant 0.25: −12.04 dB at the detector.
    let dc = vec![0.25f32; len];
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![dc.clone(), dc] })));
    let id = s.new_clip_id();
    let comp = Insert::new("compressor");
    let uid = comp.uid.get();
    {
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "dc", SourceId(1), 0, 0, len as i64));
        tr.mixer.inserts[0] = Some(comp);
    }
    mark_audio_thread();
    MixEngine::force_serial(true);
    let mut mix = MixEngine::realtime(target);
    let mut pc = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch);
    let mut env = pc.compile(Arc::new(s.clone()), pc.active(), PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    let hub = pc.scope_hub();
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut pos = 0i64;
    let mut render = |mix: &mut MixEngine, n: usize| {
        for _ in 0..n {
            mix.render_active(pos, BLOCK, &mut out);
            pos += BLOCK as i64;
        }
    };
    render(&mut mix, 8);
    assert_eq!(hub.detector_level_db(t, uid), None, "a tap that is off publishes nothing");
    assert!(hub.set_enabled(t, uid, true));
    render(&mut mix, 20);
    let got = hub.detector_level_db(t, uid).expect("the strip publishes the detector level");
    let want = 20.0 * 0.25f32.log10();
    assert!((got - want).abs() < 0.1, "detector level {got}, want {want}");
    assert!(hub.gain_reduction_db(t, uid) > 0.0, "published with the tap's gain reduction");
}
