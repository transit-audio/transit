//! The Cross Modulator insert (#200) in a session: its second input arrives over the sidechain
//! route (the key picker's, as the compressor's) in both render modes, serial and through the task
//! graph, and at 96 kHz (no conversion) the main mix is the C++ oracle rendered with the same two
//! signals, bit for bit on the codec's int16 grid; its parameters and key survive a save and load
//! and the loaded session renders the same bits; an insert saved without parameters plays at the
//! defaults.
//!
//! The graph's switch and the serial switch are process-wide, so the renders take turns; they use
//! the test's own clock (blocks back to back) and a far strips' deadline, so no outcome depends on
//! scheduling (E49). The rig is `crates/mix/tests/gain_reduction.rs`'s (#179).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_audio_io::AudioBuffer;
use transit_engine::Engine;
use transit_macro_osc_oracle::Session as Oracle;
use transit_macro_osc_oracle::modulator::{self as oracle, MODULATOR_BLOCK_SIZE};
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, create_instance,
};
use transit_model::{ChannelFormat, Clip, Insert, KeyFrom, KeySource, Route, Session, SourceAudio, SourceId, Tap, TrackId, TrackKind};
use transit_time::{Range, SampleRate};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const SR: u32 = 96_000;
const BLOCK: usize = 256;
/// The measured span (samples): a whole number of the firmware's 60-frame blocks.
const LEN: usize = 24_000;
const BLOCKS: usize = (LEN + 1_024).div_ceil(BLOCK);
const FAR: Duration = Duration::from_secs(60);

/// Two channels of an integer-generated signal on the int16 grid (`period` sets its pitch; the
/// key's is a sawtooth, the track's a triangle under an envelope).
fn grid(frames: usize, key: bool) -> [Vec<f32>; 2] {
    let mut seed = if key { 0x0200_4B45_u32 } else { 0x0200_7E57 };
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for n in 0..frames {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = i32::from((seed >> 16) as u16 as i16) >> 5;
        let n32 = n as u32;
        for (c, o) in out.iter_mut().enumerate() {
            let p = [293u32, 311][c] + if key { 144 } else { 0 };
            let ramp = (n32 % p) as i32 * 65_536 / p as i32 - 32_768;
            let v = if key {
                ramp * 3 / 8 + noise / 4
            } else {
                let env = ((n32 % 9600) as i32).min(9600 - (n32 % 9600) as i32) * 8;
                ((ramp.abs() * 2 - 32_768) / 2 + noise) * env / 38_400
            };
            o.push(v.clamp(-32_768, 32_767) as f32 / 32_768.0);
        }
    }
    out
}

fn audio(s: &mut Session, name: &str, sig: [Vec<f32>; 2], output: Route) -> TrackId {
    let src = SourceId(s.next_id + 10_000);
    let [l, r] = sig;
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![l, r] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "c", src, 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    tr.mixer.output = output;
    t
}

const PARAMS: [(&str, f32); 4] = [("algorithm", 2.6), ("timbre", 65.0), ("carrier_level", 85.0), ("modulator_level", 70.0)];

/// A key track outside the mix keying the Cross Modulator on a voice track; four quiet fillers
/// outside the mix so phase 1 is wide enough for the render workers.
fn session() -> (Session, Insert) {
    let mut s = Session::default();
    s.sample_rate = SampleRate::new(SR).unwrap();
    s.edit.delay_compensation = true;
    s.edit.click = false;
    for i in 0..4 {
        let sig: Vec<f32> = (0..LEN).map(|k| 0.01 * ((k + 97 * i) as f32 * 0.003).sin()).collect();
        audio(&mut s, &format!("Filler {i}"), [sig.clone(), sig], Route::None);
    }
    let key = audio(&mut s, "Key", grid(LEN, true), Route::None);
    let voice = audio(&mut s, "Voice", grid(LEN, false), Route::Main);
    let mut ins = Insert::new("cross_modulator");
    for (k, v) in PARAMS {
        ins.params.insert(k.into(), v);
    }
    ins.key = Some(KeySource { from: KeyFrom::Track(key), tap: Tap::Pre });
    s.track_mut(voice).unwrap().mixer.inserts[0] = Some(ins.clone());
    (s, ins)
}

#[derive(Clone, Copy, Debug)]
enum Path {
    Serial,
    Graph(usize),
}

/// `BLOCKS` blocks of `s` through `render_active` on a thread marked as the audio thread, along
/// `path`: main's channels interleaved per block, main's left and right, and the plan's latency.
fn render(s: &Session, path: Path) -> (Vec<f32>, [Vec<f32>; 2], usize) {
    let t = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let mut mix = MixEngine::realtime(t);
    let mut compiler = PlanCompiler::new(t, PlanLimits::default());
    let mut maker = |spec: &InstanceSpec, _: transit_mix::SlotGen| {
        create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch).map(|n| NewInstance { plugin: n.plugin, refused_state: false })
    };
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    if let Path::Graph(w) = path {
        assert_eq!(mix.start_render_workers_with(64, w), w);
        if w == 0 {
            mix.seed_strip_costs(Duration::from_nanos(1));
        }
        mix.set_strip_deadline(Some(FAR));
    }
    let main = transit_mix::main_channels(s).max(2);
    MixEngine::force_serial(matches!(path, Path::Serial));
    MixEngine::force_graph(matches!(path, Path::Graph(_)));
    let (mix, all, lr) = std::thread::spawn(move || {
        let mut mix = mix;
        transit_mix::mark_audio_thread();
        let mut out = vec![vec![0.0f32; BLOCK]; main];
        let mut all = Vec::with_capacity(BLOCKS * BLOCK * main);
        let mut lr = [Vec::with_capacity(BLOCKS * BLOCK), Vec::with_capacity(BLOCKS * BLOCK)];
        for b in 0..BLOCKS {
            mix.render_active((b * BLOCK) as i64, BLOCK, &mut out);
            for c in &out {
                all.extend_from_slice(c);
            }
            lr[0].extend_from_slice(&out[0]);
            lr[1].extend_from_slice(&out[1]);
        }
        transit_mix::unmark_audio_thread();
        (mix, all, lr)
    })
    .join()
    .unwrap();
    MixEngine::force_graph(false);
    MixEngine::force_serial(true);
    let latency = mix.latency;
    if let Path::Graph(_) = path {
        let f = mix.strip_faults().unwrap().snapshot();
        assert_eq!((f.overruns(), f.upstream_missed, f.conflicts, f.faulted), (0, 0, 0, 0), "{path:?}: {f:?}");
    }
    drop(mix.into_retired());
    (all, lr, latency)
}

/// The oracle over one channel: the key as input 1 (the carrier), the voice as input 2, at the
/// device's firmware parameters, seeded as the device's channel `c`.
fn oracle_main(key: &[f32], voice: &[f32], c: usize) -> Vec<i16> {
    let mut device = transit_dsp::plugins::modulator::CrossModulator::new();
    for (k, v) in PARAMS {
        assert!(transit_dsp::Plugin::set_param(&mut device, k, v));
    }
    let p = device.firmware_parameters(true);
    assert_eq!(p.carrier.index(), 0, "keyed with Carrier: Auto, the key carries");
    let q = oracle::Parameters { drive: p.drive, algorithm: p.algorithm, timbre: p.timbre, note: p.note, carrier: oracle::CarrierShape::External };
    let session = Oracle::lock();
    session.seed(transit_dsp::plugins::modulator::SEEDS[c]);
    let mut m = session.modulator().unwrap();
    let to16 = |x: f32| (x * 32_768.0) as i16;
    let mut out = Vec::with_capacity(LEN);
    for (a, b) in key.chunks(MODULATOR_BLOCK_SIZE).zip(voice.chunks(MODULATOR_BLOCK_SIZE)) {
        let frames: Vec<[i16; 2]> = a.iter().zip(b).map(|(x, y)| [to16(*x), to16(*y)]).collect();
        let mut o = vec![[0_i16; 2]; frames.len()];
        m.process(&q, &frames, &mut o).unwrap();
        out.extend(o.iter().map(|f| f[0]));
    }
    out
}

#[test]
fn the_keyed_second_input_is_the_oracle_in_both_render_modes() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, _) = session();
    let serial = render(&s, Path::Serial);
    let graphs = [render(&s, Path::Graph(2)), render(&s, Path::Graph(0))];
    for (w, g) in graphs.iter().enumerate() {
        assert_eq!(g.2, serial.2, "graph {w}: the latency");
        assert!(
            g.0.len() == serial.0.len() && g.0.iter().zip(&serial.0).all(|(a, b)| a.to_bits() == b.to_bits()),
            "graph {w} and serial are bit-exact"
        );
    }
    let latency = serial.2;
    assert_eq!(latency, MODULATOR_BLOCK_SIZE - 1, "the insert's latency, 59 frames at 96 kHz");
    let key = grid(LEN, true);
    let voice = grid(LEN, false);
    for c in 0..2 {
        let want = oracle_main(&key[c], &voice[c], c);
        let got = &serial.1[c][latency..latency + LEN];
        let codec = |y: f32| ((y * 32_768.0) as i32).clamp(-32_768, 32_767) as i16;
        let bad = got.iter().zip(&want).position(|(y, w)| codec(*y) != *w);
        assert!(bad.is_none(), "channel {c}: the main mix differs from the oracle first at timeline sample {bad:?}");
        assert!(want.iter().any(|w| w.unsigned_abs() > 1000), "channel {c}: it sounds");
    }
}

#[test]
fn the_parameters_and_key_round_trip_through_a_saved_session_and_render_the_same() {
    let mut e = Engine::default();
    let (s, _) = session();
    *e.session_mut() = s;
    let voice = e.session().tracks.iter().find(|t| t.name == "Voice").unwrap().id;
    for (param, value) in
        [("algorithm", 6.7), ("timbre", 20.0), ("shape", 2.0), ("frequency", 523.25), ("swap", 1.0), ("dry_wet", 80.0), ("output", -3.0)]
    {
        e.execute("mix.insert_param", &json!({"track": voice.0, "slot": 0, "param": param, "value": value})).unwrap();
    }
    let ins = e.session().track(voice).unwrap().mixer.inserts[0].clone().unwrap();
    assert_eq!(ins.plugin, "cross_modulator");
    let text = serde_json::to_string(e.session()).unwrap();
    let mut restored: Session = serde_json::from_str(&text).unwrap();
    // The audio pool is not part of the session file (the clips' audio files are); hand it over.
    restored.pool = e.session().pool.clone();
    let back = restored.track(voice).unwrap().mixer.inserts[0].clone().unwrap();
    assert_eq!(back.params, ins.params);
    assert_eq!(back.key, ins.key, "the key survives");
    let range = Range::new(0, 48_000);
    let a = transit_mix::render_range(e.session(), range, 256);
    let b = transit_mix::render_range(&restored, range, 256);
    assert!(a.iter().flatten().any(|v| v.abs() > 0.01), "the track sounds");
    assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()), "the loaded session renders the same bits");
}

/// An insert saved without parameters (an older or hand-written session) loads and plays at the
/// defaults: the same bits as one listing every default.
#[test]
fn an_insert_without_parameters_plays_at_the_defaults() {
    let (mut s, _) = session();
    let voice = s.tracks.iter().find(|t| t.name == "Voice").unwrap().id;
    let mut text = serde_json::to_value(&s).unwrap();
    let explicit = {
        let ins = s.track_mut(voice).unwrap().mixer.inserts[0].as_mut().unwrap();
        ins.params.clear();
        for p in transit_dsp::plugin_info("cross_modulator").unwrap().params {
            ins.params.insert(p.id.to_string(), p.default);
        }
        s.clone()
    };
    // The same session with the insert's parameters removed from the JSON.
    for t in text["tracks"].as_array_mut().unwrap() {
        if let Some(ins) = t["mixer"]["inserts"][0].as_object_mut() {
            ins.remove("params");
        }
    }
    let mut bare: Session = serde_json::from_value(text).unwrap();
    // The audio pool is not part of the session file (the clips' audio files are); hand it over.
    bare.pool = explicit.pool.clone();
    assert!(bare.track(voice).unwrap().mixer.inserts[0].as_ref().unwrap().params.is_empty());
    let range = Range::new(0, 24_000);
    let a = transit_mix::render_range(&explicit, range, 256);
    let b = transit_mix::render_range(&bare, range, 256);
    assert!(a.iter().flatten().any(|v| v.abs() > 0.01));
    assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()));
}
