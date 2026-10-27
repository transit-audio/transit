#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The real-time render (ADR 0027) through the real Player: a fake duplex device and a fake MIDI
//! port. The session has a software track (an impulse at `AT`) and an External Instrument track
//! (a note at `AT`) whose fake synth answers `C` plan frames after the note's render frame, on
//! input 1. The test pumps capture and render in lockstep and feeds the synth's answer at the
//! render frame the plan reads, so nothing depends on wall-clock time. No hardware audio or MIDI.
use super::*;
use crate::realtime_render::{FileSink, RealtimeError, RealtimeSpec, RenderSink, StemSpec};
use ab::fake::{SliceSink, SliceSource};
use std::time::{Duration, Instant};
use transit_midi_io::fake::Fake;
use transit_model::{ChannelFormat, Clip, MidiOutChannel, MidiOutPort, MidiOutput, TrackKind};

const BLOCK: usize = 192;
const SR: u32 = 48_000;
/// The External Instrument's latency (20.0 ms at 48 kHz).
const C: usize = 960;
/// The session sample of the software impulse and of the synth's note (a whole tick).
const AT: usize = 9_000;
/// The External Instrument's input gain: the stem and the mix carry it once.
const GAIN_DB: f32 = -6.0;
/// The render range.
const START: i64 = 4_800;
const LEN: u64 = 9_600;

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    // timing: stall-bound ten seconds catches a stuck worker; success always requires the named state
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "did not reach: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Rig {
    player: Player,
    audio: ab::fake::FakeBackend,
    midi: Fake,
    key: transit_midi_io::PortKey,
    synth: TrackId,
    session: Session,
    _recorder: record::Recorder,
    dir: std::path::PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn session(fake: &Fake, click: bool) -> (Session, TrackId, transit_midi_io::PortKey) {
    let key = fake.add_output("MODEL D");
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.click = click;
    // The software track: one impulse at AT.
    let len = 4 * SR as usize;
    let mut sig = vec![0.0f32; len];
    sig[AT] = 0.5;
    let src = transit_model::SourceId(s.next_id + 10_000);
    let buffer = transit_audio_io::AudioBuffer { sample_rate: SR, channels: vec![sig.clone(), sig] };
    s.pool.insert(src, Arc::new(transit_model::SourceAudio::new(buffer)));
    let click = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Click"));
    let cid = s.new_clip_id();
    s.track_mut(click).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "imp", src, 0, 0, len as i64));
    // The External Instrument: a note at AT, held long past the range, to MODEL D channel 3.
    let synth = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    let ppq = transit_time::TICKS_PER_QUARTER;
    let quarter = 24_000i64;
    assert_eq!(AT as i64 * ppq % quarter, 0, "AT is a whole tick");
    let seq = serde_json::from_value(serde_json::json!({"notes": [
        {"pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 0, "start": AT as i64 * ppq / quarter, "length": 8 * ppq},
    ], "ctrls": []}))
    .unwrap();
    let cid = s.new_clip_id();
    let t = s.track_mut(synth).unwrap();
    t.playlist_mut().unwrap().clips.push(Clip::midi(cid, "phrase", 0, len as i64, seq));
    t.midi_output = MidiOutput::External {
        port: MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() },
        channel: MidiOutChannel::Channel(2),
        offset_ms: 0.0,
    };
    let mut ins = transit_model::Insert::external("In 1");
    ins.external.as_mut().unwrap().manual_ms = Some(C as f32 * 1000.0 / SR as f32);
    ins.external.as_mut().unwrap().gain_db = GAIN_DB;
    t.instrument = Some(ins);
    (s, synth, key)
}

fn rig() -> Rig {
    rig_with(false)
}

/// [`rig`], with Options > Click on or off.
fn rig_with(click: bool) -> Rig {
    let midi = Fake::default();
    let (s, synth, key) = session(&midi, click);
    let audio = ab::fake::FakeBackend::new(ab::Format::new(SR, 2, ab::SampleFormat::F32)).with_capture(2, true);
    let fallback = ab::fake::FakeBackend::new(ab::Format::new(SR, 2, ab::SampleFormat::F32));
    let cfg = AudioConfig::default();
    let mut player = Player::open_native_or_cpal(Arc::new(s.clone()), &cfg, &audio, &fallback).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(midi.io());
    player.set_io(IoMap::for_device("test", "fake duplex", 2, 2)).unwrap();
    let recorder = player.open_input(&cfg).expect("the duplex input");
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    let dir = std::env::temp_dir().join(format!("transit-rt-render-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rig = Rig { player, audio, midi, key, synth, session: s, _recorder: recorder, dir };
    let (mut output, input) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK * 2]);
    let format = ab::Format::new(SR, 2, ab::SampleFormat::F32);
    until("the destination opens and the plan reports the device's latency", || {
        rig.audio.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 2 });
        rig.audio.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 });
        rig.player.service().unwrap();
        rig.player.midi_status().unwrap().output.track(synth) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    rig
}

fn spec(rig: &Rig, len: u64) -> RealtimeSpec {
    RealtimeSpec {
        start: START,
        len,
        main_channels: 2,
        preroll: C.max(BLOCK) as i64,
        stems: vec![StemSpec { track: rig.synth, channels: vec![0], latency: C as u32, gain: transit_dsp::db_to_gain(GAIN_DB) }],
    }
}

fn opts() -> transit_audio_io::EncodeOptions {
    transit_audio_io::EncodeOptions {
        format: transit_audio_io::FileFormat::Wav,
        bit_depth: transit_audio_io::BitDepth::Float32,
        dither: false,
        bwf: None,
    }
}

fn file_sink(rig: &Rig, len: u64) -> (FileSink, std::path::PathBuf, std::path::PathBuf) {
    let (mix, stem) = (rig.dir.join("mix.wav"), rig.dir.join("mix - Model D.wav"));
    let sink = FileSink::create(&mix, 2, None, &[Some((stem.clone(), 1))], &opts(), SR, len).unwrap();
    (sink, mix, stem)
}

/// Pump capture and render in lockstep for up to `blocks` blocks. The synth's answer to the note
/// at `AT` is on input 1 at render frame `AT + C`: the block a capture feeds is the block the plan
/// renders next (the plan drops a backlog when it adopts the input, K22), whose first frame is
/// the transport's position before it. `stop(block, player)` ends the pumping.
fn pump(rig: &Rig, blocks: usize, mut stop: impl FnMut(usize, &Player) -> bool) {
    let format = ab::Format::new(SR, 2, ab::SampleFormat::F32);
    let (mut output, mut input) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK * 2]);
    for i in 0..blocks {
        input.fill(0.0);
        if rig.player.is_playing() {
            let at = rig.player.position();
            for f in 0..BLOCK {
                if at + f as i64 == (AT + C) as i64 {
                    input[f * 2] = 0.5;
                }
            }
        }
        assert!(rig.audio.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 2 }));
        if !rig.audio.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 }) {
            break;
        }
        rig.player.service().unwrap();
        if stop(i, &rig.player) {
            break;
        }
    }
}

fn read(path: &std::path::Path) -> Vec<Vec<f32>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    transit_audio_io::decode(&bytes, Some("wav")).unwrap().1.channels
}

fn hits(x: &[f32]) -> Vec<usize> {
    x.iter().enumerate().filter(|(_, v)| v.abs() > 1e-3).map(|(i, _)| i).collect()
}

/// The software track rendered offline over the range (the External Instrument is silent there:
/// an offline plan has no live input).
fn offline(s: &Session, len: u64) -> Vec<Vec<f32>> {
    let mut r = transit_mix::RangeRender::try_new(s, None, transit_time::Range::new(START, START + len as i64), 1024).unwrap();
    let mut out = vec![Vec::new(); 2];
    while let Some((chunk, n)) = r.next_block() {
        for (o, c) in out.iter_mut().zip(chunk) {
            o.extend_from_slice(&c[..n]);
        }
    }
    out
}

fn sent(rig: &Rig) -> Vec<Vec<u8>> {
    rig.midi.sent(&rig.key)
}

/// Whether the note-on is followed by its note-off and All Notes Off on channel 3.
fn released(sent: &[Vec<u8>]) -> bool {
    let Some(on) = sent.iter().rposition(|m| *m == vec![0x92, 60, 100]) else { return false };
    let after = &sent[on..];
    after.iter().any(|m| m[..2] == [0x82, 60]) && after.contains(&vec![0xB2, 123, 0])
}

fn render(rig: &Rig, len: u64, sink: Box<dyn RenderSink>) -> Option<Result<crate::realtime_render::Rendered, RealtimeError>> {
    rig.player.start_realtime_render(&spec(rig, len), sink).unwrap_or_else(|e| panic!("{e}"));
    let mut result = None;
    let mut heard = false;
    // Keep the fake device running until the asynchronous writer finishes. A fixed callback
    // count says nothing about the writer's progress, and a callback-free wait cannot apply
    // the Stop queued by realtime_render_result after that budget is exhausted (E49, #106).
    until("the render completes while the device runs", || {
        pump(rig, 1, |_, _| false);
        let p = &rig.player;
        if !heard && p.is_playing() && p.position() > AT as i64 {
            // The pump runs ahead of real time and the output thread sends at each note's stamp:
            // let the note-on reach the port before the render can end and stop.
            until("the note-on is on the port", || sent(rig).contains(&vec![0x92, 60, 100]));
            heard = true;
        }
        result = p.realtime_render_result();
        result.is_some()
    });
    until("the stop is applied by the device", || {
        pump(rig, 1, |_, _| false);
        !rig.player.is_playing()
    });
    result
}

/// ADR 0027 decisions 1 and 4: the bounce is what live playback plays, the software render plus
/// the External Instrument's return on the grid; the stem holds the return at plan position - C.
#[test]
fn the_bounce_is_the_software_render_plus_the_return_on_the_grid() {
    let rig = rig();
    let (sink, mix, stem) = file_sink(&rig, LEN);
    let done = render(&rig, LEN, Box::new(sink)).unwrap().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(done.frames, LEN);
    assert_eq!(done.latency, C as u64, "the External Instrument is the plan's slowest path: A = C");
    let bounce = read(&mix);
    assert_eq!(bounce.len(), 2);
    assert_eq!(bounce[0].len(), LEN as usize);
    let grid = AT - START as usize;
    let software = offline(&rig.session, LEN);
    assert_eq!(hits(&software[0]), [grid], "the software impulse, rendered offline");
    let ret: Vec<f32> = bounce[0].iter().zip(&software[0]).map(|(b, s)| b - s).collect();
    let on = hits(&ret);
    assert_eq!(on.len(), 1, "the return is the only difference: {on:?}");
    assert!(on[0].abs_diff(grid) <= 1, "the return lands on the grid within one frame: {} against {grid}", on[0]);
    let stem = read(&stem);
    assert_eq!(stem.len(), 1, "the return's own width");
    assert_eq!(hits(&stem[0]), [grid], "the stem holds the return at plan position - C");
    let ret = 0.5 * transit_dsp::db_to_gain(GAIN_DB);
    assert!((stem[0][grid] - ret).abs() < 1e-6, "the stem carries the device gain once: {} against {ret}", stem[0][grid]);
    // Levels: the mix is the software render plus the return, sample for sample.
    for c in 0..2 {
        for (i, (b, sw)) in bounce[c].iter().zip(&software[c]).enumerate() {
            let want = if i == grid { sw + ret } else { *sw };
            assert!((b - want).abs() < 1e-5, "channel {c} sample {i}: {b} against {want}");
        }
    }
    // The note went out on the port, and the stop at the end released it.
    assert!(sent(&rig).contains(&vec![0x92, 60, 100]), "{:x?}", sent(&rig));
    until("the stop at the end releases the note", || released(&sent(&rig)));
    assert!(!rig.player.realtime_render_running());
    assert!(!rig.player.is_playing() || rig.player.position() >= START + LEN as i64);
    let parts: Vec<_> =
        std::fs::read_dir(&rig.dir).unwrap().filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "part")).collect();
    assert!(parts.is_empty(), "no part file is left: {parts:?}");
}

/// ADR 0027 decision 3: Cancel mid-render stops playback, releases the held note on the port
/// (note-off and All Notes Off, through the output thread) and leaves no file.
#[test]
fn cancel_mid_render_releases_the_held_note_and_leaves_no_file() {
    let rig = rig();
    let (sink, mix, stem) = file_sink(&rig, LEN);
    rig.player.start_realtime_render(&spec(&rig, LEN), Box::new(sink)).unwrap();
    assert!(rig.player.realtime_render_running());
    // Into the range, past the note-on at AT (the note is held long past the range).
    pump(&rig, 400, |_, p| p.is_playing() && p.position() > (AT + C) as i64);
    until("the note-on is on the port", || sent(&rig).contains(&vec![0x92, 60, 100]));
    assert!(!released(&sent(&rig)), "the note is held: {:x?}", sent(&rig));
    rig.player.cancel_realtime_render();
    assert!(!rig.player.realtime_render_running());
    assert!(rig.player.realtime_render_result().is_none(), "nothing to collect");
    until("the note-off and All Notes Off after the note-on", || released(&sent(&rig)));
    assert!(!mix.exists() && !stem.exists(), "no file");
    assert_eq!(std::fs::read_dir(&rig.dir).unwrap().count(), 0, "no part file either");
    // The transport stopped: the next blocks do not play on.
    pump(&rig, 4, |_, _| false);
    assert!(!rig.player.is_playing());
}

/// A sink whose first mix write waits until the test lets it go: the writer falls behind.
struct Stalled {
    gate: std::sync::mpsc::Receiver<()>,
    aborted: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl RenderSink for Stalled {
    fn main(&mut self, _: &[Vec<f32>], _: usize) -> Result<(), String> {
        let _ = self.gate.recv();
        Ok(())
    }
    fn stem(&mut self, _: usize, _: &[Vec<f32>], _: usize) -> Result<(), String> {
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        self.finished.store(true, Ordering::Release);
        Ok(())
    }
    fn abort(&mut self) {
        self.aborted.store(true, Ordering::Release);
    }
}

/// ADR 0027 decision 3: a writer that falls behind fills the ring; the audio thread drops the
/// block, counts it, and the render fails with the message; nothing is written and the synth is
/// released.
#[test]
fn an_overrun_fails_the_render_with_the_message() {
    let rig = rig();
    let (tx, gate) = std::sync::mpsc::channel();
    let (aborted, finished) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let sink = Stalled { gate, aborted: Arc::clone(&aborted), finished: Arc::clone(&finished) };
    // Longer than the ring holds (16 channel-seconds over 3 channels: 5.3 s).
    let len = u64::from(SR) * 6;
    rig.player.start_realtime_render(&spec(&rig, len), Box::new(sink)).unwrap();
    let (mut started, mut failed) = (false, false);
    pump(&rig, (len as usize) / BLOCK + 100, |_, p| {
        started |= p.is_playing();
        failed = p.realtime_render_result().is_some() || (started && !p.is_playing());
        failed
    });
    assert!(failed, "the overrun stopped the render");
    drop(tx);
    let mut result = None;
    until("the writer gives up", || {
        result = rig.player.realtime_render_result();
        result.is_some()
    });
    let err = result.unwrap().unwrap_err();
    assert!(matches!(err, RealtimeError::Overrun { blocks: 1 }), "{err:?}");
    assert_eq!(err.to_string(), "Render failed: the file writer fell behind and 1 block of audio was dropped. Nothing was written: render again.");
    assert!(aborted.load(Ordering::Acquire) && !finished.load(Ordering::Acquire), "the sink was aborted, not finished");
    until("the note is released", || released(&sent(&rig)));
}

/// ADR 0027 decision 2, the Player's refusals: playing, a measurement running, a render running,
/// a return channel the device did not open, and a device that is not duplex.
#[test]
fn the_player_refuses_what_cannot_render() {
    let rig = rig();
    let sink = || -> Box<dyn RenderSink> { Box::new(file_sink(&rig, LEN).0) };
    let mut bad = spec(&rig, LEN);
    bad.stems[0].channels = vec![5];
    assert!(rig.player.start_realtime_render(&bad, sink()).unwrap_err().contains("channel the device did not open"));
    rig.player.start_external_measure(rig.synth, &[0]).unwrap();
    assert_eq!(rig.player.start_realtime_render(&spec(&rig, LEN), sink()).unwrap_err(), "A measurement is running: wait for it or cancel it.");
    rig.player.cancel_external_measure();
    pump(&rig, 400, |_, p| !p.external_measure_running());
    rig.player.start_realtime_render(&spec(&rig, LEN), sink()).unwrap();
    assert_eq!(rig.player.start_realtime_render(&spec(&rig, LEN), sink()).unwrap_err(), "A real-time render is already running.");
    rig.player.cancel_realtime_render();
    rig.player.play(0, None, None).unwrap();
    pump(&rig, 400, |_, p| p.is_playing());
    assert_eq!(rig.player.start_realtime_render(&spec(&rig, LEN), sink()).unwrap_err(), "Stop the transport to render in real time.");
    // An output-only device.
    let audio = ab::fake::FakeBackend::new(ab::Format::new(SR, 2, ab::SampleFormat::F32));
    let player = Player::open_native_or_cpal(Arc::new(rig.session.clone()), &AudioConfig::default(), &audio, &audio).unwrap();
    assert_eq!(player.start_realtime_render(&spec(&rig, LEN), sink()).unwrap_err(), "Needs a duplex device");
    assert_eq!(std::fs::read_dir(&rig.dir).unwrap().count(), 0, "a refusal leaves no file");
}

/// ADR 0027 decision 3: a device lost mid-render ends it like a Cancel, and says so.
#[test]
fn a_device_lost_mid_render_releases_and_says_so() {
    let rig = rig();
    let (sink, mix, _) = file_sink(&rig, LEN);
    rig.player.start_realtime_render(&spec(&rig, LEN), Box::new(sink)).unwrap();
    pump(&rig, 400, |_, p| p.is_playing() && p.position() > (AT + C) as i64);
    until("the note-on is on the port", || sent(&rig).contains(&vec![0x92, 60, 100]));
    assert!(rig.audio.inject_error(ab::Direction::Output, ab::StreamError::DeviceNotAvailable));
    let mut result = None;
    until("the render ends", || {
        result = rig.player.realtime_render_result();
        result.is_some()
    });
    assert_eq!(result.unwrap().unwrap_err(), RealtimeError::DeviceLost);
    until("the output thread releases the track", || released(&sent(&rig)));
    assert!(!mix.exists());
}

/// ADR 0027 decision 1: the bounce leaves the metronome out, as an offline render does, even with
/// Click on across a beat (the click is not delay-compensated and is never part of a bounce).
#[test]
fn the_bounce_leaves_the_metronome_out_with_click_on() {
    let rig = rig_with(true);
    // From 0.1 s over 0.6 s: the beat at 0.5 s is inside the range.
    let len = 28_800;
    let (sink, mix, _) = file_sink(&rig, len);
    let done = render(&rig, len, Box::new(sink)).unwrap().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(done.frames, len);
    let bounce = read(&mix);
    assert_eq!(bounce.len(), 2);
    assert_eq!(bounce[0].len(), len as usize);
    assert_eq!(bounce[1].len(), len as usize);
    let software = offline(&rig.session, len);
    let grid = AT - START as usize;
    for c in 0..2 {
        let extra: Vec<usize> =
            bounce[c].iter().zip(&software[c]).enumerate().filter(|(i, (b, s))| *i != grid && (*b - *s).abs() > 1e-5).map(|(i, _)| i).collect();
        assert!(extra.is_empty(), "channel {c}: click energy at {} samples, first {:?}", extra.len(), extra.first());
    }
}
