//! Clip events of tracks with an external MIDI destination (#61), pushed into the MIDI output
//! thread's ring as each block is rendered. Audio thread: no allocation, no lock, no call into
//! `transit-midi-io`; a full ring drops (and counts) the event, with a reserve for releases.
//!
//! Each event is stamped with the host time H at which its sample is presented: the block's
//! first render frame mapped through the converter phase to an output-device frame, and that
//! frame through the device clock map (or, while the map is unavailable, the host time at
//! callback entry plus the frames between). See `docs/contributing/knowledge/midi-output.md`.
use crate::player::midi_out::{Kind, OutEvent, Producer};
use transit_model::Session;
use transit_time::Samples;

#[cfg(test)]
std::thread_local! {
    /// Test seam (#226): on the calling thread (the test's device callback), the callback-entry
    /// host time is frame-derived, `base + output_frame / device rate`, instead of the real clock,
    /// so external MIDI stamps follow the frames rendered, not how fast the test thread runs.
    pub(in crate::player) static FRAME_CLOCK_BASE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// The callback-entry host time of `output_frame` at `device_rate` under [`FRAME_CLOCK_BASE`],
/// when a test set it on this thread.
#[cfg(test)]
pub(in crate::player) fn frame_clock_now(output_frame: u64, device_rate: u32) -> Option<u64> {
    let base = FRAME_CLOCK_BASE.get()?;
    let ns = u128::from(output_frame) * 1_000_000_000 / u128::from(device_rate.max(1));
    Some(base.saturating_add(u64::try_from(ns).unwrap_or(u64::MAX)))
}

pub(in crate::player) struct External {
    out: Producer,
    was_playing: bool,
    /// Where the previous block ended; another start is a locate or a loop wrap.
    next: Option<Samples>,
    /// Host time at callback entry and the output frame the callback started filling.
    anchor: Option<(u64, u64)>,
}

/// The block about to render, as the scheduler needs it.
#[derive(Clone, Copy, Debug)]
pub(in crate::player) struct Block {
    pub playing: bool,
    pub position: Samples,
    pub frames: usize,
    /// Host time P(R) of the block's first sample at the output, if known, before the mix's
    /// delay compensation.
    pub host_ns: Option<u64>,
    /// The mix's delay compensation A as time: what a bare MIDI-out track adds to P(R).
    pub delay_ns: u64,
    /// Nanoseconds per session sample at the current speed.
    pub ns_per_frame: f64,
}

impl Block {
    /// The host time a track's notes for the block's first sample are stamped with: P(R) + A / r
    /// for a bare MIDI-out track (#61), so the synth stays with the delayed mix; P(R) for a track
    /// that holds an External Instrument, whose return the plan compensates (ADR 0020 decision 2).
    pub(in crate::player) fn host_for(&self, external_instrument: bool) -> Option<u64> {
        if external_instrument { self.host_ns } else { self.host_ns.map(|h| h.saturating_add(self.delay_ns)) }
    }
}

impl External {
    pub(in crate::player) fn new(out: Producer) -> Self {
        Self { out, was_playing: false, next: None, anchor: None }
    }

    /// At callback entry: the host time now and the output frame about to be filled.
    pub(in crate::player) fn callback(&mut self, host_now: Option<u64>, output_frame: u64) {
        self.anchor = host_now.map(|now| (now, output_frame));
    }

    /// The fallback presentation estimate of `output_frame` when the clock map is unavailable:
    /// callback entry time plus the frames between. It ignores the device's output latency, so
    /// it is early by that latency (the destination offset can compensate).
    pub(in crate::player) fn fallback_host(&self, output_frame: u64, ns_per_output_frame: f64) -> Option<u64> {
        let (now, first) = self.anchor?;
        let ahead = output_frame.checked_sub(first)? as f64 * ns_per_output_frame;
        ahead.is_finite().then(|| now.saturating_add(ahead as u64))
    }

    /// An External Instrument Measure (ADR 0020 decision 3): the probe records the return of the
    /// block just rendered and hands over the messages due at its first frame, which go out
    /// stamped P(R) like the track's own notes (an External Instrument's are not delayed by A).
    pub(in crate::player) fn measure(
        &mut self,
        probe: &crate::external_probe::ExtProbe,
        block: &Block,
        input: &[Vec<f32>],
        frames: usize,
        backlog: Option<usize>,
    ) {
        probe.note_backlog(backlog, frames);
        let at_ns = block.host_for(true).unwrap_or(0);
        let out = &mut self.out;
        probe.block(input, frames, |e| {
            out.push(OutEvent { kind: Kind::Message, track: e.track, at_ns, bytes: e.bytes, len: 3, delay_ns: 0 });
        });
    }

    pub(in crate::player) fn block(&mut self, session: &Session, block: Block) {
        let at_from = |host_ns: Option<u64>, offset: usize| {
            host_ns.map_or(0, |host| {
                let delta = offset as f64 * block.ns_per_frame;
                if delta.is_finite() { host.saturating_add(delta as u64) } else { host }
            })
        };
        let at = |offset: usize| at_from(block.host_ns, offset);
        if !block.playing {
            if self.was_playing {
                self.out.push(OutEvent { kind: Kind::Stop, at_ns: at(0), ..OutEvent::default() });
            }
            self.was_playing = false;
            self.next = None;
            return;
        }
        let jump = self.was_playing && self.next != Some(block.position);
        if jump {
            // P(R) of the wrap: each destination adds A / r if its notes carry it.
            self.out.push(OutEvent { kind: Kind::Release, at_ns: at(0), delay_ns: block.delay_ns, ..OutEvent::default() });
        }
        let chase = jump || !self.was_playing;
        let any_solo = session.tracks.iter().any(|t| t.mixer.solo);
        for track in &session.tracks {
            if !track.kind.is_midi() || track.inactive || track.midi_output.is_internal() {
                continue;
            }
            let host = block.host_for(track.external_instrument().is_some());
            let silent = track.mixer.mute || (any_solo && !track.mixer.solo && !track.mixer.solo_safe);
            let out = &mut self.out;
            transit_mix::external_midi_events(session, track, block.position, block.frames, chase, |event| {
                // A muted track still ends its notes.
                if silent && !transit_midi_io::is_release(event.message()) {
                    return;
                }
                out.push(OutEvent {
                    kind: Kind::Message,
                    track: track.id.0,
                    at_ns: at_from(host, event.offset),
                    bytes: event.bytes,
                    len: event.len,
                    delay_ns: 0,
                });
            });
        }
        self.was_playing = true;
        self.next = block.position.checked_add(block.frames as i64);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use transit_model::{ChannelFormat, Clip, ClipId, Insert, MidiOutChannel, MidiOutPort, MidiOutput, TrackKind};

    /// A bare MIDI-out track and an External Instrument track, each with a note at frame 0.
    fn session() -> (Session, u64, u64) {
        let mut s = Session::default();
        let mut add = |kind: TrackKind, device: bool, clip: u64| {
            let t = s.add_track(kind, ChannelFormat::Stereo, None);
            // Playback does not depend on the sequence crate; the clip's sequence comes from its JSON.
            let seq = serde_json::from_value(serde_json::json!({"notes": [
                {"pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 0, "start": 0, "length": transit_time::TICKS_PER_QUARTER},
            ], "ctrls": []}))
            .unwrap();
            let track = s.track_mut(t).unwrap();
            track.playlist_mut().unwrap().clips.push(Clip::midi(ClipId(clip), "phrase", 0, 4 * 24_000, seq));
            track.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::Channel(0), offset_ms: 0.0 };
            if device {
                track.instrument = Some(Insert::external("In 1"));
            }
            t.0
        };
        let bare = add(TrackKind::Midi, false, 1);
        let device = add(TrackKind::Instrument, true, 2);
        (s, bare, device)
    }

    /// ADR 0020 decision 2: a bare MIDI-out track's notes go out at P(R) + A / r, with the
    /// delayed mix; an External Instrument track's at P(R), offset 0, since its return comes back
    /// through the plan and is compensated there. A wrap's release carries P(R) and A apart.
    #[test]
    fn only_a_bare_midi_out_track_is_stamped_with_the_mixs_delay() {
        let (s, bare, device) = session();
        let (out, mut rx) = crate::player::midi_out::test_ring(1024);
        let mut external = External::new(out);
        let (host, delay) = (1_000_000_000u64, 22_000_000u64);
        let block = Block { playing: true, position: 0, frames: 64, host_ns: Some(host), delay_ns: delay, ns_per_frame: 1e9 / 48_000.0 };
        external.block(&s, block);
        let mut ons = Vec::new();
        while let Ok(e) = rx.pop() {
            if e.kind == Kind::Message && e.bytes[0] & 0xF0 == 0x90 {
                ons.push((e.track, e.at_ns));
            }
        }
        ons.sort_unstable();
        assert_eq!(ons, [(bare, host + delay), (device, host)], "the device's note is not delayed by A");
        // A locate: the release names P(R) and A separately so each destination ends on its clock.
        external.block(&s, Block { position: 48_000, ..block });
        let release = std::iter::from_fn(|| rx.pop().ok()).find(|e| e.kind == Kind::Release).unwrap();
        assert_eq!((release.at_ns, release.delay_ns), (host, delay));
    }

    /// ADR 0020 decision 2 for the Measure (N1 of the slice review): a probe note is stamped
    /// P(R0), the presentation time of the block it goes out on, exactly as an External Instrument
    /// track's clip note is (offset 0, no A / r, no other shift), because C is defined against it.
    #[test]
    fn a_measure_note_is_stamped_p_of_its_block_start_like_a_clip_note() {
        let (out, mut rx) = crate::player::midi_out::test_ring(1024);
        let mut external = External::new(out);
        let probe = crate::external_probe::ExtProbe::new(48_000);
        assert!(probe.arm(7, 2, 60, 100, &[0]));
        let (host, delay, ns) = (1_000_000_000u64, 22_000_000u64, 1e9 / 48_000.0);
        let input = vec![vec![0.0f32; 64]];
        let mut ons = Vec::new();
        for i in 0..120u64 {
            let block = Block {
                playing: false,
                position: 0,
                frames: 64,
                host_ns: Some(host + (i as f64 * 64.0 * ns) as u64),
                delay_ns: delay,
                ns_per_frame: ns,
            };
            external.measure(&probe, &block, &input, 64, None);
            while let Ok(e) = rx.pop() {
                if e.kind == Kind::Message && e.bytes[0] & 0xF0 == 0x90 {
                    ons.push((e.track, e.at_ns, block.host_for(true).unwrap()));
                }
            }
        }
        assert!(!ons.is_empty(), "the warm-up note went out");
        for (track, at_ns, p) in ons {
            assert_eq!(track, 7);
            assert_eq!(at_ns, p, "P(R0) of the block, without the mix's delay");
        }
    }

    // The send chained to the plan: the stamps External pushes, read back as render frames, feed
    // a fake duplex device's input, so the fake return can never disagree with the real stamp.
    const SR: u32 = 48_000;
    const BLOCK: usize = 64;
    /// The session sample of the note and of the software click (120 ticks at 120 bpm).
    const AT: usize = 3000;
    const LEN: usize = 8_192;
    /// The synth's true round trip from its send, in plan frames, and the device's latency C.
    const C: usize = 1061;
    const H0: u64 = 1_000_000_000;

    fn chain_session() -> Session {
        let mut s = Session::default();
        s.edit.delay_compensation = true;
        s.edit.click = false;
        let mut sig = vec![0.0f32; LEN];
        sig[AT] = 0.5;
        let src = transit_model::SourceId(s.next_id + 10_000);
        let buffer = transit_audio_io::AudioBuffer { sample_rate: SR, channels: vec![sig.clone(), sig] };
        s.pool.insert(src, std::sync::Arc::new(transit_model::SourceAudio::new(buffer)));
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Click"));
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "imp", src, 0, 0, LEN as i64));
        tr.mixer.pan = vec![-1.0, 1.0];
        s
    }

    /// A track playing a note at `AT` to an external port; with `device`, an External Instrument
    /// returning on In 1 with its latency set to `C`.
    fn note_track(s: &mut Session, device: bool) -> u64 {
        let kind = if device { TrackKind::Instrument } else { TrackKind::Midi };
        let t = s.add_track(kind, ChannelFormat::Stereo, None);
        let seq = serde_json::from_value(serde_json::json!({"notes": [
            {"pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 0, "start": AT / 25, "length": 480},
        ], "ctrls": []}))
        .unwrap();
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "phrase", 0, LEN as i64, seq));
        tr.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::Channel(0), offset_ms: 0.0 };
        if device {
            let mut ins = Insert::external("In 1");
            ins.external.as_mut().unwrap().manual_ms = Some(C as f32 * 1000.0 / SR as f32);
            tr.instrument = Some(ins);
            tr.mixer.pan = vec![-1.0, 1.0];
        }
        t.0
    }

    struct Chained {
        /// Main's first channel from render frame 0 (output frame f is presented at P(f)).
        main: Vec<f32>,
        /// Each note-on's send instant as a render frame: (at_ns - P(0)) / r.
        sends: Vec<(u64, usize)>,
        latency: usize,
    }

    /// Renders `s` through a realtime plan while External stamps each block as the audio thread
    /// does (P(R), and the plan's A from the previous render). The synth on `device` answers `C`
    /// frames after the instant its note-on was stamped, on input 1.
    fn chain(s: &Session, device: Option<u64>) -> Chained {
        use transit_mix::{Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget};
        let io = transit_model::IoMap::for_device("test", "fake duplex", 2, 2);
        let target =
            RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: io.outputs, in_channels: io.inputs };
        MixEngine::force_serial(true);
        let mut mix = MixEngine::realtime(target);
        let mut compiler = PlanCompiler::new(target, PlanLimits::default());
        compiler.set_io(std::sync::Arc::new(io.clone()));
        let mut maker = |spec: &InstanceSpec, _: transit_mix::SlotGen| {
            transit_mix::create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
                .map(|n| transit_mix::NewInstance { plugin: n.plugin, refused_state: false })
        };
        let mut env = compiler.compile(std::sync::Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
        assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
        compiler.reclaim(env);
        let (ring, mut rx) = crate::player::midi_out::test_ring(1024);
        let mut external = External::new(ring);
        let ns = 1e9 / f64::from(SR);
        let mut out = vec![vec![0.0f32; BLOCK]; transit_mix::main_channels(s)];
        let (mut main, mut sends, mut onset, mut pos) = (Vec::new(), Vec::new(), None, 0usize);
        while main.len() < LEN + 3 * C {
            let block = Block {
                playing: true,
                position: pos as i64,
                frames: BLOCK,
                host_ns: Some(H0 + (pos as f64 * ns) as u64),
                delay_ns: (mix.latency as f64 * ns) as u64,
                ns_per_frame: ns,
            };
            external.block(s, block);
            while let Ok(e) = rx.pop() {
                if e.kind == Kind::Message && e.bytes[0] & 0xF0 == 0x90 {
                    let frame = ((e.at_ns - H0) as f64 / ns).round() as usize;
                    sends.push((e.track, frame));
                    if Some(e.track) == device {
                        onset = Some(frame + C);
                    }
                }
            }
            mix.input =
                (0..usize::from(io.inputs)).map(|c| (0..BLOCK).map(|i| if c == 0 && Some(pos + i) == onset { 0.5 } else { 0.0 }).collect()).collect();
            mix.render_active(pos as i64, BLOCK, &mut out);
            main.extend_from_slice(&out[0]);
            pos += BLOCK;
        }
        Chained { main, sends, latency: mix.latency }
    }

    fn hits(x: &[f32]) -> Vec<usize> {
        x.iter().enumerate().filter(|(_, v)| v.abs() > 1e-3).map(|(i, _)| i).collect()
    }

    /// ADR 0020 decision 2 end to end (B1 of the slice review): the device is the slowest path
    /// (A = C), its note leaves at P(R(S)), the synth answers C later, and delay compensation puts
    /// the onset on the software click's Main frame. Stamped with +A, the onset would land A late.
    #[test]
    fn the_stamped_send_plus_the_round_trip_lands_on_the_click_at_main() {
        let mut s = chain_session();
        let synth = note_track(&mut s, true);
        let r = chain(&s, Some(synth));
        assert_eq!(r.latency, C, "the device is the slowest path");
        assert_eq!(r.sends, [(synth, AT)], "the device's note goes out at P(R(S)), offset 0");
        assert_eq!(hits(&r.main), [AT + C], "the onset and the click reach Main on the same frame");
    }

    /// A bare MIDI-out track with a latent insert on the master: A includes the master insert, the
    /// click is heard at P(AT) + A, and the bare track's note goes out then too.
    #[test]
    fn a_bare_midi_out_track_stays_with_the_click_behind_a_latent_master_insert() {
        let mut s = chain_session();
        let bare = note_track(&mut s, false);
        let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        s.track_mut(master).unwrap().mixer.inserts[0] = Some(Insert::new("maximizer"));
        let i = transit_dsp::create("maximizer").unwrap().latency();
        assert!(i > 0);
        let r = chain(&s, None);
        assert_eq!(r.latency, i, "A is the master insert's latency");
        assert_eq!(hits(&r.main).first().copied(), Some(AT + i), "the click is heard A late");
        assert_eq!(r.sends, [(bare, AT + i)], "the bare track's note goes out when the click is heard");
    }
}
