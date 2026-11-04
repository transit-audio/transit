//! ADR 0015 slice (d): hardware inserts. A fake duplex device plays the sinks back on its input a
//! known number of frames later, so a hardware insert is a delay of that many frames. With the
//! round trip measured the insert reports it and delay compensation lines the strip up with a
//! parallel dry track, sample for sample. Unmeasured, the insert reports nothing and the strip is
//! late by the round trip (documented). With no capture stream the insert passes the signal
//! through dry.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{
    Adoption, DeviceGeneration, HardwareState, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, hardware_report,
    plan_sinks, unresolved_routes,
};
use transit_model::{ChannelFormat, Clip, HardwareLatency, Insert, IoMap, Session, SourceAudio, SourceId, TrackId, TrackKind};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;
const AT: usize = 2400;
const LEN: usize = 8_192;
/// The fake device's round trip: more than a block, not a multiple of one.
const D: usize = 700;
const SEND: &str = "Out 3-4";
const RET: &str = "In 3-4";

fn session() -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.click = false;
    s
}

/// A stereo audio track playing a 0.5 impulse at `AT` on both channels.
fn impulse_track(s: &mut Session, name: &str) -> TrackId {
    let mut sig = vec![0.0f32; LEN];
    sig[AT] = 0.5;
    let src = SourceId(s.next_id + 10_000);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![sig.clone(), sig] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "imp", src, 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    t
}

fn target(io: &IoMap) -> RenderTarget {
    RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: io.outputs, in_channels: io.inputs }
}

fn io(measured: Option<usize>) -> IoMap {
    let mut io = IoMap::for_device("test", "fake duplex", 4, 4);
    if let Some(frames) = measured {
        io.set_hardware_latency(HardwareLatency { send: SEND.into(), ret: RET.into(), round_trip_frames: frames as u32, rate: SR, measured_at: 1 });
    }
    io
}

struct Rendered {
    /// Main's first channel from sample 0 of the render.
    main: Vec<f32>,
    latency: usize,
    /// What the device played on its channel 2 (the first channel of "Out 3-4").
    sent: Vec<f32>,
}

/// Renders `s` on the fake device. `duplex` feeds the input from the device output `delay` frames
/// earlier; otherwise the mix gets no input at all.
fn render(s: &Session, io: &IoMap, duplex: bool, delay: usize) -> Rendered {
    render_on(s, io, duplex, delay, 2, Some)
}

/// [`render`] on a device as wide as `io`: input channel `c` is cabled from output `cable(c)`, and
/// `Rendered::sent` is what output `sent` played.
fn render_on(s: &Session, io: &IoMap, duplex: bool, delay: usize, sent: usize, cable: impl Fn(usize) -> Option<usize>) -> Rendered {
    let (outs, ins) = (usize::from(io.outputs), usize::from(io.inputs));
    MixEngine::force_serial(true);
    let mut mix = MixEngine::realtime(target(io));
    let mut compiler = PlanCompiler::new(target(io), PlanLimits::default());
    compiler.set_io(Arc::new(io.clone()));
    let mut maker = |spec: &InstanceSpec, _: transit_mix::SlotGen| {
        transit_mix::create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
            .map(|n| transit_mix::NewInstance { plugin: n.plugin, refused_state: false })
    };
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let sinks = plan_sinks(s, io, io.outputs);
    let dest = sinks.destinations();
    let main_w = transit_mix::main_channels(s);
    let mut out = vec![vec![0.0f32; BLOCK]; main_w + sinks.channels()];
    // The device's output stream, per device channel, from sample 0.
    let mut device: Vec<Vec<f32>> = vec![Vec::new(); outs];
    let (mut main, mut pos, mut latency) = (Vec::new(), 0usize, 0usize);
    while main.len() < LEN + latency + 2 * D {
        mix.input = if duplex {
            (0..ins)
                .map(|c| (0..BLOCK).map(|i| (pos + i).checked_sub(delay).and_then(|k| device[cable(c)?].get(k)).copied().unwrap_or(0.0)).collect())
                .collect()
        } else {
            Vec::new()
        };
        let (_, allocations) = measure(|| mix.render_active(pos as i64, BLOCK, &mut out));
        assert_eq!(allocations, AllocStats::default(), "hardware rendering allocates and frees nothing");
        latency = mix.latency;
        main.extend_from_slice(&out[0]);
        for c in &mut device {
            c.extend(std::iter::repeat_n(0.0, BLOCK));
        }
        for (k, &d) in dest.iter().enumerate() {
            for i in 0..BLOCK {
                device[usize::from(d)][pos + i] += out[main_w + k][i];
            }
        }
        pos += BLOCK;
    }
    assert_eq!(mix.status().capacity_exceeded, 0, "the plan's delay lines hold the round trip");
    Rendered { main, latency, sent: device.swap_remove(sent) }
}

fn hits(x: &[f32]) -> Vec<usize> {
    x.iter().enumerate().filter(|(_, v)| v.abs() > 1e-3).map(|(i, _)| i).collect()
}

fn dry_baseline() -> (f32, usize) {
    let mut s = session();
    impulse_track(&mut s, "Dry");
    let r = render(&s, &io(None), false, 0);
    let h = hits(&r.main);
    (r.main[h[0]], h[0])
}

#[test]
fn the_insert_delays_the_strip_by_the_round_trip_and_sends_the_signal_out() {
    let (level, at) = dry_baseline();
    assert_eq!(at, AT);
    let mut s = session();
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!(r.latency, D, "the insert reports the measured round trip");
    assert_eq!(hits(&r.main), [AT + D], "the strip output is its input delayed by the round trip");
    assert_eq!(r.main[AT + D], level, "and nothing else changed: unity through the send and the return");
    assert_eq!(hits(&r.sent), [AT], "the signal left on the send path at its own time");
    assert_eq!(r.sent[AT], 0.5);
}

#[test]
fn compensation_lines_the_insert_up_with_a_parallel_dry_track() {
    let mut s = session();
    impulse_track(&mut s, "Dry");
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!(hits(&r.main), [AT + D], "both tracks land on the same sample");
    // The two sum: a dry track alone would be half as loud.
    let (level, _) = dry_baseline();
    assert!((r.main[AT + D] - 2.0 * level).abs() < 1e-6);
    // Compensation off: no alignment and no latency reported.
    s.edit.delay_compensation = false;
    let off = render(&s, &io(Some(D)), true, D);
    assert_eq!(hits(&off.main), [AT, AT + D]);
}

#[test]
fn an_unmeasured_insert_reports_nothing_and_the_strip_is_late() {
    let mut s = session();
    impulse_track(&mut s, "Dry");
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let r = render(&s, &io(None), true, D);
    assert_eq!(r.latency, 0);
    assert_eq!(hits(&r.main), [AT, AT + D], "uncompensated: the outboard track is late by the round trip");
    let report = hardware_report(&s, &io(None), 4, 4, true, SR);
    assert_eq!(report.len(), 1);
    assert_eq!((report[0].state.clone(), report[0].latency), (HardwareState::Unmeasured, None));
    let report = hardware_report(&s, &io(Some(D)), 4, 4, true, SR);
    assert_eq!((report[0].state.clone(), report[0].latency), (HardwareState::Ready, Some(D as u32)));
}

#[test]
fn without_a_capture_stream_the_insert_passes_the_signal_through_dry() {
    let mut s = session();
    impulse_track(&mut s, "Dry");
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let r = render(&s, &io(Some(D)), false, 0);
    assert_eq!(r.latency, 0, "an inactive insert adds no latency");
    assert_eq!(hits(&r.main), [AT]);
    assert!(hits(&r.sent).is_empty(), "nothing is sent while the insert is inactive");
    let report = hardware_report(&s, &io(Some(D)), 4, 0, false, SR);
    assert_eq!(report[0].state, HardwareState::NoInput);
}

#[test]
fn bypass_and_missing_paths_pass_the_signal_through_dry() {
    let mut s = session();
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().bypass = true;
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!((hits(&r.main), r.latency), (vec![AT], 0));
    assert!(hits(&r.sent).is_empty());
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware("Nowhere", RET));
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!(hits(&r.main), [AT]);
    assert_eq!(unresolved_routes(&s, &io(None), 4), ["Nowhere"]);
    assert_eq!(hardware_report(&s, &io(None), 4, 4, true, SR)[0].state, HardwareState::MissingSend);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, "In 9"));
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!(hits(&r.main), [AT]);
    assert_eq!(hardware_report(&s, &io(None), 4, 4, true, SR)[0].state, HardwareState::MissingReturn);
}

#[test]
fn the_send_path_is_a_sink_of_the_plan() {
    let mut s = session();
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let p = plan_sinks(&s, &io(None), 4);
    assert_eq!(p.sinks.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), [SEND]);
    assert_eq!(p.destinations(), [2, 3]);
}

#[test]
fn a_disabled_insert_passes_the_signal_through_dry_claims_no_sink_and_reports_why() {
    let mut s = session();
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().hardware.as_mut().unwrap().disabled = Some("send path in use by Other".into());
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!((hits(&r.main), r.latency), (vec![AT], 0), "dry, no latency");
    assert!(hits(&r.sent).is_empty(), "nothing is sent");
    assert!(plan_sinks(&s, &io(None), 4).sinks.is_empty(), "a disabled insert's send path is no sink");
    let report = hardware_report(&s, &io(Some(D)), 4, 4, true, SR);
    assert_eq!((report[0].state.clone(), report[0].latency), (HardwareState::Disabled("send path in use by Other".into()), None));
}

/// The TASCAM Model 12 over native ASIO opens 12 inputs and 10 outputs: an insert on its last pairs
/// ("Out 9-10" cabled back to "In 11-12") sends on device channels 8 and 9, returns on 10 and 11,
/// and is compensated like one on the first channels.
#[test]
fn an_insert_on_the_last_channels_of_a_12_in_10_out_device_sends_and_returns_there() {
    let mut io = IoMap::for_device("ASIO", "Model Mixer ASIO Driver", 12, 10);
    io.set_hardware_latency(HardwareLatency {
        send: "Out 9-10".into(),
        ret: "In 11-12".into(),
        round_trip_frames: D as u32,
        rate: SR,
        measured_at: 1,
    });
    let mut s = session();
    let dry = impulse_track(&mut s, "Dry");
    let t = impulse_track(&mut s, "Outboard");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::hardware("Out 9-10", "In 11-12"));
    let cable = |input: usize| (input >= 10).then(|| input - 2);
    let r = render_on(&s, &io, true, D, 8, cable);
    assert_eq!(r.latency, D, "the measured round trip is reported");
    assert_eq!(hits(&r.main), [AT + D], "the dry track is compensated to the outboard one");
    assert_eq!(hits(&r.sent), [AT], "the signal left on output 9");
    assert_eq!(plan_sinks(&s, &io, 10).destinations(), [8, 9]);
    let report = hardware_report(&s, &io, 10, 12, true, SR);
    assert_eq!((report[0].state.clone(), report[0].latency), (HardwareState::Ready, Some(D as u32)));
    // A stem on the last pair leaves on the same channels (ADR 0015 output paths).
    s.track_mut(t).unwrap().mixer.inserts[0] = None;
    s.track_mut(dry).unwrap().mixer.output = transit_model::Route::Hardware("Out 9-10".into());
    assert_eq!(plan_sinks(&s, &io, 10).destinations(), [8, 9]);
}

/// Master hardware uses the same fake duplex cable as the ordinary-track regressions above.
#[test]
fn master_hardware_sends_returns_and_counts_in_shared_latency_without_editing_the_session() {
    let mut s = session();
    impulse_track(&mut s, "Main source");
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    s.track_mut(master).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let before = s.to_json().unwrap();
    let r = render(&s, &io(Some(D)), true, D);
    assert_eq!(hits(&r.sent), [AT], "the master's hardware send leaves at Main's arrival");
    assert_eq!(hits(&r.main), [AT + D], "Main is the actual hardware return");
    assert_eq!(r.main[AT + D], r.sent[AT], "unity round trip");
    assert_eq!(r.latency, D, "M includes the master hardware round trip");
    let report = hardware_report(&s, &io(Some(D)), 4, 4, true, SR);
    assert_eq!(report.len(), 1);
    assert_eq!((report[0].track, report[0].state.clone(), report[0].latency), (master, HardwareState::Ready, Some(D as u32)));
    assert_eq!(s.to_json().unwrap(), before);
    let loaded = Session::from_json(&before).unwrap();
    assert_eq!(loaded.to_json().unwrap(), before, "the complete saved session survives load unchanged");

    // A direct output skips the master but waits for its round trip: proves the M term in L.
    let direct = impulse_track(&mut s, "Direct");
    s.track_mut(direct).unwrap().mixer.output = transit_model::Route::Hardware("Out 5-6".into());
    let mut map = IoMap::for_device("test", "fake duplex", 4, 6);
    map.hardware_latency = io(Some(D)).hardware_latency;
    let direct = render_on(&s, &map, true, D, 4, Some);
    assert_eq!(hits(&direct.sent), [AT + D]);
    assert_eq!(hits(&direct.main), [AT + D]);
    assert_eq!(direct.latency, D);
}

#[test]
fn master_hardware_missing_paths_and_no_input_are_reported_and_dry() {
    let mut s = session();
    impulse_track(&mut s, "Source");
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    for (send, ret, duplex, state) in [
        ("Nowhere", RET, true, HardwareState::MissingSend),
        (SEND, "In 9", true, HardwareState::MissingReturn),
        (SEND, RET, false, HardwareState::NoInput),
    ] {
        s.track_mut(master).unwrap().mixer.inserts[0] = Some(Insert::hardware(send, ret));
        let report = hardware_report(&s, &io(Some(D)), 4, 4, duplex, SR);
        assert_eq!(report.len(), 1, "master hardware has a report even when unavailable");
        assert_eq!(report[0].state, state);
        let r = render(&s, &io(Some(D)), duplex, D);
        assert_eq!((hits(&r.main), r.latency), (vec![AT], 0));
        assert!(hits(&r.sent).is_empty());
        if send == "Nowhere" {
            assert_eq!(unresolved_routes(&s, &io(Some(D)), 4), ["Nowhere"]);
        }
    }
}

#[test]
fn master_hardware_is_dry_in_offline_deliverables() {
    let mut s = session();
    impulse_track(&mut s, "Source");
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    s.track_mut(master).unwrap().mixer.inserts[0] = Some(Insert::hardware(SEND, RET));
    let map = io(Some(D));
    assert_eq!(hits(&render(&s, &map, true, D).main), [AT + D], "first establish a working live loop");
    let mut offline = transit_mix::RangeRender::try_new_paths(&s, transit_time::Range { start: 0, end: LEN as i64 }, BLOCK, &map).unwrap();
    let (mut main, mut sent) = (Vec::new(), Vec::new());
    while let Some((out, n)) = offline.next_block() {
        main.extend_from_slice(&out[0][..n]);
        sent.extend_from_slice(&out[2][..n]);
    }
    assert_eq!(hits(&main), [AT]);
    assert!(hits(&sent).is_empty());
}
