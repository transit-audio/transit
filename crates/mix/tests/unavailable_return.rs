//! Review of 652509f3, must-fix 1: a hardware insert's return path that the open input cannot
//! deliver (saved on channel 6, the input reopened with two channels) is unavailable as a whole:
//! the insert runs the flagged dry bypass and never reads another input channel, whether the plan
//! was built before the capture attached (`in_channels` 0, a separately opened input) or after.
//! The live input carries a loud constant on channel 2, which a wrapped read (6 % 2) would hear.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{
    Adoption, DeviceGeneration, HardwareState, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, hardware_report,
};
use transit_model::{ChannelFormat, Clip, InputMap, Insert, IoDirection, IoMap, IoPath, Route, Session, SourceAudio, SourceId, TrackKind};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;
const AT: usize = 1_000;
const LEN: usize = 4_096;

/// An output-only device record with the input record of a two-channel input that keeps "Far" on
/// channel 6 (unavailable) and "In 2" on channel 2.
fn io() -> IoMap {
    let path = |name: &str, channels: Vec<u16>| IoPath {
        live: false,
        name: name.into(),
        direction: IoDirection::Input,
        channels,
        format: ChannelFormat::Mono,
    };
    let record = InputMap {
        system: "test".into(),
        device: "duplex".into(),
        inputs: 2,
        defaults_for: 6,
        paths: vec![path("Far", vec![5]), path("In 2", vec![1])],
    };
    IoMap::for_device("test", "duplex", 0, 4).compose(Some(&record))
}

/// A mono audio track playing a 0.5 impulse at `AT`, through a hardware insert sending on
/// "Out 3-4" and returning on `ret`.
fn session(ret: &str) -> Session {
    let mut s = Session::default();
    s.edit.click = false;
    let mut sig = vec![0.0f32; LEN];
    sig[AT] = 0.5;
    let src = SourceId(s.next_id + 10_000);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![sig] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Outboard"));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "imp", src, 0, 0, LEN as i64));
    tr.mixer.inserts[0] = Some(Insert::hardware("Out 3-4", ret));
    s
}

/// Main's first channel over `LEN` frames, the plan compiled for an input `in_channels` wide, the
/// live input two channels with 0.25 on channel 2.
fn render(s: &Session, io: &IoMap, in_channels: u16) -> Vec<f32> {
    MixEngine::force_serial(true);
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: io.outputs, in_channels };
    let mut mix = MixEngine::realtime(target);
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    compiler.set_io(Arc::new(io.clone()));
    let mut maker = |spec: &InstanceSpec, _: transit_mix::SlotGen| {
        transit_mix::create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
            .map(|n| transit_mix::NewInstance { plugin: n.plugin, refused_state: false })
    };
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let sinks = transit_mix::plan_sinks(s, io, io.outputs);
    let mut out = vec![vec![0.0f32; BLOCK]; transit_mix::main_channels(s) + sinks.channels()];
    let mut main = Vec::new();
    let mut pos = 0usize;
    while main.len() < LEN {
        mix.input = vec![vec![0.0; BLOCK], vec![0.25; BLOCK]];
        let (_, allocations) = measure(|| mix.render_active(pos as i64, BLOCK, &mut out));
        assert_eq!(allocations, AllocStats::default(), "rendering allocates and frees nothing");
        main.extend_from_slice(&out[0]);
        pos += BLOCK;
    }
    main
}

fn loud(main: &[f32]) -> Vec<usize> {
    main.iter().enumerate().filter(|(_, v)| v.abs() > 1e-3).map(|(i, _)| i).collect()
}

#[test]
fn an_unavailable_return_is_a_dry_bypass_and_never_wraps_to_another_input() {
    let io = io();
    let s = session("Far");
    for in_channels in [0, 2] {
        let report = hardware_report(&s, &io, 4, in_channels, true, SR);
        assert_eq!(report[0].state, HardwareState::MissingReturn, "flagged (in_channels {in_channels})");
        assert_eq!(loud(&render(&s, &io, in_channels)), [AT], "dry: the strip's own impulse only, never channel 2 (in_channels {in_channels})");
    }
    // Negative space: a return the input delivers is read (the strip hears channel 2's constant).
    let available = session("In 2");
    assert_eq!(hardware_report(&available, &io, 4, 2, true, SR)[0].state, HardwareState::Unmeasured);
    assert!(loud(&render(&available, &io, 2)).len() > LEN / 2, "an available return is heard");
}

/// Review of 652509f3, must-fix 5, and the ruling of 2026-11-20: monitoring reads the channels the
/// compiler resolved (the render has no access to the path table: `LiveInput` carries none). An
/// available path is heard; an unavailable path, a label past the input and a name the table does
/// not hold are silent, never channel 1 or a wrap; the render allocates nothing.
#[test]
fn monitoring_reads_the_compiled_route_and_unavailable_names_are_silent() {
    let io = io();
    let monitored = |input: &str| {
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mon"));
        let tr = s.track_mut(t).unwrap();
        tr.mixer.input = Route::Hardware(input.into());
        tr.mixer.input_monitor = true;
        loud(&render(&s, &io, 2)).len()
    };
    assert!(monitored("In 2") > LEN / 2, "a path the input delivers is heard");
    for name in ["Far", "In 3", "In 6", "Bass DI", "In 1-3"] {
        assert_eq!(monitored(name), 0, "{name}: silent, never channel 1 or 2");
    }
}
