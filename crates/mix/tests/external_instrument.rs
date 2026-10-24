//! ADR 0020 slice 1: an External Instrument in a plan. A fake duplex device's input carries the
//! synth's onset C frames after the render frame of the note that played it; with the device's
//! latency set to C, delay compensation lines the return up with a software click at the same
//! session sample, on the same Main frame, bit exact. A missing return, port or capture stream
//! leaves the strip silent with latency 0, so compensation does not move.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{
    Adoption, DeviceGeneration, ExternalState, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, external_report,
};
use soundcraft_model::{
    ChannelFormat, Clip, Insert, IoMap, MidiOutChannel, MidiOutPort, MidiOutput, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use std::sync::Arc;

const SR: u32 = 48_000;
const BLOCK: usize = 64;
/// The session sample of the note and of the software click.
const AT: usize = 2400;
const LEN: usize = 8_192;
/// The fake chain's latency in plan frames (the ADR's example; more than a block, not a multiple).
const C: usize = 1061;

fn session() -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.click = false;
    s
}

/// A stereo audio track playing a 0.5 impulse at `AT` on both channels: the software click.
fn click_track(s: &mut Session) -> TrackId {
    let mut sig = vec![0.0f32; LEN];
    sig[AT] = 0.5;
    let src = SourceId(s.next_id + 10_000);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![sig.clone(), sig] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Click"));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "imp", src, 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    t
}

/// An instrument track holding an External Instrument returning on `ret`, with its latency set to
/// `C` frames by hand, sending to a MIDI port.
fn synth_track(s: &mut Session, ret: &str) -> TrackId {
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    let tr = s.track_mut(t).unwrap();
    let mut ins = Insert::external(ret);
    ins.external.as_mut().unwrap().manual_ms = Some(C as f32 * 1000.0 / SR as f32);
    tr.instrument = Some(ins);
    tr.midi_output = MidiOutput::External {
        port: MidiOutPort { backend: "fake".into(), name: "MODEL D".into(), ordinal: 0, id: String::new() },
        channel: MidiOutChannel::Channel(0),
        offset_ms: 0.0,
    };
    tr.mixer.pan = vec![-1.0, 1.0];
    t
}

fn io() -> IoMap {
    IoMap::for_device("test", "fake duplex", 2, 2)
}

fn target(io: &IoMap) -> RenderTarget {
    RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: io.outputs, in_channels: io.inputs }
}

struct Rendered {
    /// Main's first channel from render frame 0.
    main: Vec<f32>,
    latency: usize,
}

/// Renders `s` from frame 0. With `duplex`, input 1 carries the synth's onset (0.5) at render frame
/// `AT + C`: the note at session sample `AT` is rendered at frame `AT` and comes back `C` later.
/// `no_port` names tracks whose MIDI port the playback layer reports missing.
fn render(s: &Session, duplex: bool, no_port: &[TrackId]) -> Rendered {
    let io = io();
    MixEngine::force_serial(true);
    let mut mix = MixEngine::realtime(target(&io));
    let mut compiler = PlanCompiler::new(target(&io), PlanLimits::default());
    compiler.set_io(Arc::new(io.clone()));
    compiler.set_external_unavailable(Arc::from(no_port.to_vec()));
    let mut maker = |spec: &InstanceSpec, _: soundcraft_mix::SlotGen| {
        soundcraft_mix::create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
            .map(|n| soundcraft_mix::NewInstance { plugin: n.plugin, refused_state: false })
    };
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let main_w = soundcraft_mix::main_channels(s);
    let mut out = vec![vec![0.0f32; BLOCK]; main_w];
    let (mut main, mut pos, mut latency) = (Vec::new(), 0usize, 0usize);
    while main.len() < LEN + 2 * C {
        mix.input = if duplex {
            (0..usize::from(io.inputs)).map(|c| (0..BLOCK).map(|i| if c == 0 && pos + i == AT + C { 0.5 } else { 0.0 }).collect()).collect()
        } else {
            Vec::new()
        };
        mix.render_active(pos as i64, BLOCK, &mut out);
        latency = mix.latency;
        main.extend_from_slice(&out[0]);
        pos += BLOCK;
    }
    assert_eq!(mix.status().capacity_exceeded, 0, "the plan's delay lines hold the device's latency");
    Rendered { main, latency }
}

fn hits(x: &[f32]) -> Vec<usize> {
    x.iter().enumerate().filter(|(_, v)| v.abs() > 1e-3).map(|(i, _)| i).collect()
}

#[test]
fn the_return_lands_on_main_with_a_software_click_at_the_same_sample_bit_exact() {
    let mut s = session();
    let click = click_track(&mut s);
    let synth = synth_track(&mut s, "In 1");
    assert_eq!(s.track(synth).unwrap().external_instrument().unwrap().manual_frames(SR), Some(C as u32));
    // The strip reports C (inserts add none here), and Main's arrival A is C.
    let both = render(&s, true, &[]);
    assert_eq!(both.latency, C, "the device's latency is the plan's");
    assert_eq!(hits(&both.main), [AT + C], "the onset and the click reach Main on the same frame");
    // Each alone, through the same plan: the same frame, the same value, bit for bit.
    s.track_mut(click).unwrap().mixer.mute = true;
    let synth_alone = render(&s, true, &[]);
    s.track_mut(click).unwrap().mixer.mute = false;
    s.track_mut(synth).unwrap().mixer.mute = true;
    let click_alone = render(&s, true, &[]);
    assert_eq!(hits(&synth_alone.main), [AT + C]);
    assert_eq!(hits(&click_alone.main), [AT + C], "the click is delayed by A - 0 = C");
    assert_eq!(synth_alone.main[AT + C].to_bits(), click_alone.main[AT + C].to_bits());
    assert_eq!(both.main[AT + C].to_bits(), (synth_alone.main[AT + C] + click_alone.main[AT + C]).to_bits());
    let report = external_report(&s, &io(), 2, true, SR, &[]);
    assert_eq!((report[0].state, report[0].latency), (ExternalState::Ready, Some(C as u32)));
}

#[test]
fn an_insert_on_the_strip_adds_to_the_device_latency() {
    // Strip latency is C + I: a latent insert after the device delays the click by C + I too.
    let mut s = session();
    click_track(&mut s);
    let synth = synth_track(&mut s, "In 1");
    s.track_mut(synth).unwrap().mixer.inserts[0] = Some(Insert::new("maximizer"));
    let i = soundcraft_dsp::create("maximizer").unwrap().latency();
    assert!(i > 0);
    let r = render(&s, true, &[]);
    assert_eq!(r.latency, C + i);
    let h = hits(&r.main);
    assert_eq!(h.first(), Some(&(AT + C + i)), "{h:?}");
}

#[test]
fn the_return_gain_applies_before_the_inserts() {
    let mut s = session();
    let synth = synth_track(&mut s, "In 1");
    let unity = render(&s, true, &[]);
    s.track_mut(synth).unwrap().instrument.as_mut().unwrap().external.as_mut().unwrap().gain_db = -6.0;
    let quiet = render(&s, true, &[]);
    let ratio = quiet.main[AT + C] / unity.main[AT + C];
    assert!((ratio - soundcraft_dsp::db_to_gain(-6.0)).abs() < 1e-6, "{ratio}");
}

#[test]
fn a_missing_return_port_or_capture_stream_leaves_the_strip_silent_with_latency_zero() {
    let check = |s: &Session, duplex: bool, no_port: &[TrackId], state: ExternalState| {
        let r = render(s, duplex, no_port);
        assert_eq!(r.latency, 0, "{state:?}: compensation does not move");
        assert_eq!(hits(&r.main), [AT], "{state:?}: only the click sounds, undelayed");
        let in_channels = if duplex { 2 } else { 0 };
        assert_eq!(external_report(s, &io(), in_channels, duplex, SR, no_port)[0].state, state);
    };
    let mut s = session();
    click_track(&mut s);
    let synth = synth_track(&mut s, "In 9");
    check(&s, true, &[], ExternalState::MissingReturn);
    s.track_mut(synth).unwrap().instrument.as_mut().unwrap().external.as_mut().unwrap().ret = "In 1".into();
    check(&s, true, &[synth], ExternalState::NoPort);
    check(&s, false, &[], ExternalState::NoInput);
    s.track_mut(synth).unwrap().midi_output = MidiOutput::Internal;
    check(&s, true, &[], ExternalState::NoPort);
}

#[test]
fn an_offline_render_has_no_live_input_and_leaves_the_device_out() {
    let mut s = session();
    click_track(&mut s);
    synth_track(&mut s, "In 1");
    let out = soundcraft_mix::render_range(&s, soundcraft_time::Range::new(0, LEN as i64), BLOCK);
    assert_eq!(hits(&out[0]), [AT], "no latency and no return offline");
}
