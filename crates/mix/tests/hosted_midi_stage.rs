//! ADR 0064 slice HM2 through the engine (the plan compiler and a realtime mix engine) with the
//! real VST3 and CLAP adapters and their fixture plug-ins in the MIDI effect slot.
//!
//! The fixtures are built by `transit-test-support`'s helpers; each instance for the slot is made
//! by the test's factory, which enables note-output collection before `prepare` and scripts it
//! through its state (the fixtures' `AT`, `EMIT`, `ECHO` and `FAIL` commands). The fixtures' own
//! records (their state) show what the host called. The instrument is a recording fake that
//! writes down every event at its frame, except where a case says what sounds: there it is the
//! built-in Macro Oscillator, measured at the engine's output.
//!
//! Cases, per format: a generator drives the instrument at every scripted frame, identically at host
//! blocks of 1, 7, 64 and 512 (MIDI-byte 9n with velocity 0 as a release, CLAP); the zero-audio note
//! effect transforms live input at exact frames at those sizes; an off written in a second native
//! call is released at that call's start (blocks of 64, a native maximum of 16); a processing error
//! in the second of four native calls fails the block whole and the stage is silent until Stop; a
//! state load through the plug-in's state reader is a barrier at offset 0 and the next call runs the
//! plug-in's reset first. And with the Macro Oscillator: after the failing block nothing sounds.
//! Expected values are written from the record.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::{Plugin, PluginStateReader};
use transit_mix::{
    Adoption, Arrival, CompileMode, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent,
    MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderFrame, RenderTarget, SlotGen, Source, midi_fx_of_slot,
};
use transit_model::{ChannelFormat, Insert, MidiEffectSlot, Session, TrackId, TrackKind};

const SR: u32 = 48_000;
const MAX_BLOCK: usize = 512;
const SIZES: [usize; 4] = [1, 7, 64, 512];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Vst3,
    Clap,
}

const FORMATS: [Format; 2] = [Format::Vst3, Format::Clap];

fn tmp() -> &'static Path {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
}

/// Installs a fixture build (`feature`) once per binary and returns the ids of its classes by name.
fn ids(format: Format, feature: &str) -> Vec<(String, String)> {
    static CACHE: OnceLock<Mutex<Vec<(Format, String, Vec<(String, String)>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    let mut c = cache.lock().unwrap();
    if let Some((_, _, v)) = c.iter().find(|(f, n, _)| *f == format && n == feature) {
        return v.clone();
    }
    let dir: PathBuf = tmp().join(format!("hm2-{format:?}-{feature}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let found: Vec<(String, String)> = match format {
        Format::Vst3 => {
            transit_test_support::fixtures::install_vst3_named(tmp(), &["--features", feature], &format!("hm2-vst3-{feature}-target"), &dir, "Fixture");
            // Registered first: an id the registry does not know would make `create` scan the machine.
            assert!(transit_vst3_host::add_search_dir(&dir) > 0);
            transit_vst3_host::scan_paths(&[dir]).into_iter().map(|d| (d.name, d.id)).collect()
        }
        Format::Clap => {
            transit_clap_host::set_scan_extra_dirs_only(true);
            let name = if feature == "notes" { "Notes" } else { "NoteEffects" };
            transit_test_support::fixtures::install_clap_named(tmp(), &["--features", feature], &format!("hm2-clap-{feature}-target"), &dir, name);
            assert!(transit_clap_host::add_search_dir(&dir) > 0);
            transit_clap_host::scan_paths(&[dir]).into_iter().map(|d| (d.name, d.id)).collect()
        }
    };
    c.push((format, feature.to_string(), found.clone()));
    found
}

fn class(format: Format, feature: &str, name: &str) -> String {
    ids(format, feature).into_iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{format:?} {name}")).1
}

fn generator(format: Format) -> String {
    class(format, "notes", "Fixture Notes")
}

fn note_effect(format: Format) -> String {
    class(format, "note-effects", "Fixture Note Effect A")
}

/// The tests of this binary run one at a time (the hosts' registries and instances are shared).
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

type Heard = Arc<Mutex<Vec<(u64, PluginEvent)>>>;

/// The instrument: writes down every event at its frame and writes `dc` to its outputs.
struct Recorder {
    frame: u64,
    heard: Heard,
    dc: f32,
}

impl Plugin for Recorder {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = self.dc);
        }
        self.frame += frames as u64;
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.heard.lock().unwrap().push((self.frame + u64::from(offset), *ev));
        true
    }
}

/// How the slot's instance is made: the plug-in id, its native maximum block and its script.
#[derive(Clone)]
struct Hosted {
    id: String,
    native: usize,
    script: String,
}

/// The instrument: the recorder, or the built-in Macro Oscillator.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Instrument {
    Recorder,
    MacroOsc,
}

struct Rig {
    mix: MixEngine,
    comp: PlanCompiler,
    generation: PlanGeneration,
    session: Session,
    track: TrackId,
    heard: Heard,
    readers: Arc<Mutex<Vec<Box<dyn PluginStateReader>>>>,
    hosted: Option<Hosted>,
    instrument: Instrument,
    pos: i64,
    now: u64,
    out: Vec<Vec<f32>>,
}

fn create(id: &str) -> Box<dyn Plugin> {
    if id.starts_with(transit_vst3_host::ID_PREFIX) { transit_vst3_host::create(id) } else { transit_clap_host::create(id) }.unwrap()
}

impl Rig {
    fn new(hosted: Option<Hosted>, instrument: Instrument) -> Rig {
        let mut session = Session::default();
        session.edit.click = false;
        let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        session.track_mut(track).unwrap().instrument = Some(Insert::new("macro_oscillator"));
        if let Some(h) = &hosted {
            session.track_mut(track).unwrap().midi_effects.push(MidiEffectSlot::new(h.id.clone()));
        }
        let target = RenderTarget { device: DeviceGeneration(5), sample_rate: SR, max_block: MAX_BLOCK as u32, out_channels: 0, in_channels: 0 };
        let mut rig = Rig {
            mix: MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default()),
            comp: PlanCompiler::new(target, PlanLimits::default()),
            generation: PlanGeneration::EMPTY,
            session: session.clone(),
            track,
            heard: Arc::default(),
            readers: Arc::default(),
            hosted,
            instrument,
            pos: 0,
            now: 0,
            out: vec![vec![0.0; MAX_BLOCK]; 2],
        };
        rig.publish(session);
        rig.now = rig.mix.next_frame().0;
        rig
    }

    fn publish(&mut self, s: Session) {
        self.generation = self.generation.checked_next().unwrap();
        let (heard, readers, hosted, instrument) = (Arc::clone(&self.heard), Arc::clone(&self.readers), self.hosted.clone(), self.instrument);
        let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            let plugin: Box<dyn Plugin> = if midi_fx_of_slot(spec.slot).is_some() {
                let h = hosted.as_ref().unwrap();
                let mut p = create(&spec.id);
                // The card: instance creation enables collection before `prepare`.
                p.collect_note_output(true);
                p.prepare(SR as f32, h.native, spec.ch);
                assert!(p.load_state(h.script.as_bytes()), "{}", h.script);
                let _ = p.save_state();
                if let Some(r) = p.state_reader() {
                    readers.lock().unwrap().push(r);
                }
                p
            } else if spec.slot == INSTRUMENT_SLOT && instrument == Instrument::Recorder {
                Box::new(Recorder { frame: 0, heard: Arc::clone(&heard), dc: 0.25 })
            } else {
                let mut p = transit_dsp::create(&spec.id)?;
                p.prepare(SR as f32, MAX_BLOCK, spec.ch);
                p
            };
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = self.comp.compile_with_mode(Arc::new(s.clone()), self.comp.active(), self.generation, CompileMode::Update, &mut make).unwrap();
        assert_eq!(self.mix.adopt_plan(&mut env), Adoption::Adopted);
        self.comp.reclaim(env);
        self.session = s;
    }

    /// A live event on channel 0, due `at` frames after the rig's start.
    fn live(&mut self, at: u64, message: Message) {
        let target = self.comp.active_bindings().into_iter().find(|b| b.0 == self.track && b.1 == INSTRUMENT_SLOT).unwrap();
        let ev = InstrumentEvent {
            engine: EngineEpoch::FIRST,
            target,
            due: Due::Frame(RenderFrame(self.now + at)),
            transport: None,
            source: Source { port: 1, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: 0 },
        };
        assert_eq!(self.mix.queue_event(&ev), EventOutcome::Queued, "{message:?} at {at}");
    }

    /// Renders blocks of `size` (the last one shorter) until `end`; the output's peak.
    fn run_until(&mut self, end: u64, size: usize) -> f32 {
        let mut peak = 0.0f32;
        while (self.pos as u64) < end {
            let n = usize::try_from(end - self.pos as u64).unwrap().min(size);
            self.mix.render_active(self.pos, n, &mut self.out);
            peak = self.out.iter().flat_map(|c| c.iter().take(n)).fold(peak, |m, x| m.max(x.abs()));
            self.pos += n as i64;
        }
        peak
    }

    /// The instrument's notes: (frame, kind, channel, key, velocity, note id).
    fn heard(&self) -> Vec<(u64, &'static str, u8, u8, u8, i32)> {
        self.heard
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(f, e)| match e.message {
                Message::NoteOn { key, velocity, .. } => Some((*f, "on", e.channel, key, velocity, e.note_id)),
                Message::NoteOff { key, velocity, .. } => Some((*f, "off", e.channel, key, velocity, e.note_id)),
                _ => None,
            })
            .collect()
    }

    /// The fixture's record since the last read (its state through its reader).
    fn log(&self) -> Vec<String> {
        let blob = self.readers.lock().unwrap()[0].save_state().unwrap_or_default();
        let text = match transit_vst3_host::preset::split_state(&blob) {
            Some((component, _)) if self.hosted.as_ref().is_some_and(|h| h.id.starts_with(transit_vst3_host::ID_PREFIX)) => component.to_vec(),
            _ => blob,
        };
        String::from_utf8(text).unwrap().lines().map(str::to_string).collect()
    }
}

fn hosted(id: String, native: usize, script: &str) -> Option<Hosted> {
    Some(Hosted { id, native, script: script.to_string() })
}

#[test]
fn a_fixture_generator_drives_the_instrument_at_every_scripted_frame_identically_at_every_block_size() {
    let _s = serial();
    for format in FORMATS {
        // Decision 3 after the ledger: host ids from 0; velocities round(127 v) half away from zero.
        let (script, want) = match format {
            Format::Vst3 => (
                "CLEAR;AT 10 on 0 0 60 7 0.5;AT 100 on 0 1 64 -1 1.0;AT 300 off 0 0 60 7 0;AT 700 off 0 1 64 -1 0;AT 1000 on 0 3 72 9 1.0;AT 1020 off 0 3 72 9 0.25",
                vec![(10, "on", 0, 60, 64, 0), (100, "on", 1, 64, 127, 1), (300, "off", 0, 60, 0, 0), (700, "off", 1, 64, 0, 1), (1000, "on", 3, 72, 127, 2), (1020, "off", 3, 72, 32, 2)],
            ),
            // The CLAP generator's readable output is port 1. A MIDI-byte 9n of velocity 0 is a
            // release before the ledger matches it (5.3).
            Format::Clap => (
                "CLEAR;AT 10 on 1 0 60 7 0.5;AT 100 on 1 1 64 -1 1.0;AT 200 midi 1 145 62 90 0;AT 300 off 1 0 60 7 0;AT 400 midi 1 145 62 0 0;\
                 AT 700 off 1 1 64 -1 0;AT 1000 on 1 3 72 9 1.0;AT 1020 off 1 3 72 9 0.25",
                vec![
                    (10, "on", 0, 60, 64, 0),
                    (100, "on", 1, 64, 127, 1),
                    (200, "on", 1, 62, 90, 2),
                    (300, "off", 0, 60, 0, 0),
                    (400, "off", 1, 62, 0, 2),
                    (700, "off", 1, 64, 0, 1),
                    (1000, "on", 3, 72, 127, 3),
                    (1020, "off", 3, 72, 32, 3),
                ],
            ),
        };
        for size in SIZES {
            let mut r = Rig::new(hosted(generator(format), MAX_BLOCK, script), Instrument::Recorder);
            r.run_until(1024, size);
            assert_eq!(r.heard(), want, "{format:?} block {size}");
            assert!(r.log().iter().any(|l| l.starts_with("wrote at")), "{format:?}: the fixture wrote its script");
        }
    }
}

#[test]
fn the_zero_audio_note_effect_transforms_live_input_at_exact_frames_at_every_block_size() {
    let _s = serial();
    for format in FORMATS {
        for size in SIZES {
            let mut r = Rig::new(hosted(note_effect(format), MAX_BLOCK, "CLEAR;ECHO 12"), Instrument::Recorder);
            r.live(25, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE });
            r.live(300, Message::NoteOn { key: 64, velocity: 90, id: NoteId::NONE });
            r.live(600, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE });
            r.live(900, Message::NoteOff { key: 64, velocity: 0, id: NoteId::NONE });
            r.run_until(1024, size);
            let want = vec![(25, "on", 0, 72, 100, 0), (300, "on", 0, 76, 90, 1), (600, "off", 0, 72, 0, 0), (900, "off", 0, 76, 0, 1)];
            assert_eq!(r.heard(), want, "{format:?} block {size}");
            // The fixture had no audio bus or port in its calls.
            let log = r.log();
            let calls: Vec<&String> = log.iter().filter(|l| l.starts_with("process")).collect();
            assert!(!calls.is_empty());
            let zero = |l: &&String| match format {
                Format::Vst3 => l.contains("in=0 out=0"),
                Format::Clap => l.contains("ain=0 aout=0"),
            };
            assert!(calls.iter().all(zero), "{format:?}: no audio in any call: {calls:?}");
        }
    }
}

#[test]
fn an_off_written_in_a_second_native_call_is_released_at_that_calls_start() {
    let _s = serial();
    for format in FORMATS {
        // Host blocks of 64, a native maximum of 16: on at local 10 of the first call, off at local 0
        // of the second (block offsets 10 and 16).
        let script = match format {
            Format::Vst3 => "CLEAR;EMIT on 0 10 0 0 60 7 1.0;EMIT off 1 0 0 0 60 7 0",
            Format::Clap => "CLEAR;EMIT on 0 10 1 0 60 7 1.0;EMIT off 1 0 1 0 60 7 0",
        };
        let mut r = Rig::new(hosted(generator(format), 16, script), Instrument::Recorder);
        r.run_until(64, 64);
        assert_eq!(r.heard(), vec![(10, "on", 0, 60, 127, 0), (16, "off", 0, 60, 0, 0)], "{format:?}");
        assert_eq!(r.log().iter().filter(|l| l.starts_with("process")).count(), 4, "{format:?}: four native calls");
    }
}

#[test]
fn a_processing_error_in_the_second_of_four_calls_fails_the_block_whole_until_stop() {
    let _s = serial();
    for format in FORMATS {
        // Calls 4 to 7 make the block at 64; call 5 reports the error. The note at frame 10 sounds;
        // call 4 writes a note-on at block offset 2, discarded with the block; after Stop the
        // fixture's frame 128 (it was processed for 96 frames: six calls of 16) is the host's 544.
        let script = match format {
            Format::Vst3 => "CLEAR;AT 10 on 0 0 60 7 1.0;EMIT on 4 2 0 0 62 8 1.0;FAIL 5;AT 128 on 0 0 65 9 1.0;AT 158 off 0 0 65 9 0",
            Format::Clap => "CLEAR;AT 10 on 1 0 60 7 1.0;EMIT on 4 2 1 0 62 8 1.0;FAIL 5;AT 128 on 1 0 65 9 1.0;AT 158 off 1 0 65 9 0",
        };
        let mut r = Rig::new(hosted(generator(format), 16, script), Instrument::Recorder);
        r.live(70, Message::NoteOn { key: 50, velocity: 100, id: NoteId::NONE });
        r.live(200, Message::NoteOn { key: 51, velocity: 100, id: NoteId::NONE });
        r.run_until(512, 64);
        assert_eq!(r.heard(), vec![(10, "on", 0, 60, 127, 0), (64, "off", 0, 60, 0, 0)], "{format:?}: the block discarded whole, the barrier at its offset 0");
        let log = r.log();
        assert!(log.iter().any(|l| l == "fail 5"), "{format:?}: the fixture failed call 5: {log:?}");
        assert!(log.iter().any(|l| l.starts_with("wrote emit") || l.starts_with("wrote on 2")), "{format:?}: call 4 wrote its note-on: {log:?}");
        assert_eq!(log.iter().filter(|l| l.starts_with("process")).count(), 6, "{format:?}: not called after the failing block");
        // (Controllers reach the CLAP fixture as MIDI, so only note input is counted.)
        let notes_in: Vec<&String> = log.iter().filter(|l| l.starts_with("in on")).collect();
        assert_eq!(notes_in.len(), 1, "{format:?}: the failing block's input reached it, later input did not: {log:?}");
        // The engine's Stop ends the failure: the stage runs again from the next block, its reset
        // before the first call.
        r.mix.reset();
        r.run_until(1024, 64);
        assert_eq!(r.heard().into_iter().filter(|h| h.0 >= 512).collect::<Vec<_>>(), vec![(544, "on", 0, 65, 127, 1), (574, "off", 0, 65, 0, 1)], "{format:?}");
        let log = r.log();
        let first = log.iter().position(|l| l.starts_with("process")).unwrap();
        let reset = match format {
            Format::Vst3 => log.iter().position(|l| l.starts_with("setProcessing 1")),
            Format::Clap => log.iter().position(|l| l == "reset"),
        };
        assert!(reset.is_some_and(|i| i < first), "{format:?}: the reset before the first call after Stop: {log:?}");
    }
}

#[test]
fn after_a_failing_block_nothing_sounds_at_a_real_instrument() {
    let _s = serial();
    // The VST3 generator holds a note from frame 10 with no release; call 65 (the second call of the
    // block at 1024) fails. The Macro Oscillator's default release (300 ms to -60 dB) has
    // decayed to exact silence within 3 s (measured: about 2.6 s from a note-off).
    let script = "CLEAR;AT 10 on 0 0 60 7 1.0;FAIL 65";
    let mut r = Rig::new(hosted(generator(Format::Vst3), 16, script), Instrument::MacroOsc);
    assert!(r.run_until(1024, 64) > 0.0, "the note sounds");
    r.run_until(1024 + 144_000, 64);
    let after = r.run_until(1024 + 144_000 + 4096, 64);
    assert_eq!(after, 0.0, "after the failing block nothing sounds");
    assert!(r.log().iter().any(|l| l == "fail 65"));
}

#[test]
fn a_state_load_is_a_barrier_at_offset_0_and_the_next_call_runs_the_reset_first() {
    let _s = serial();
    for format in FORMATS {
        let script = match format {
            Format::Vst3 => "CLEAR;AT 10 on 0 0 60 7 1.0;AT 600 on 0 0 62 8 1.0;AT 650 off 0 0 62 8 0",
            Format::Clap => "CLEAR;AT 10 on 1 0 60 7 1.0;AT 600 on 1 0 62 8 1.0;AT 650 off 1 0 62 8 0",
        };
        for size in SIZES {
            let mut r = Rig::new(hosted(generator(format), MAX_BLOCK, script), Instrument::Recorder);
            r.run_until(512, size);
            let _ = r.log();
            // A state load through the state reader (a preset recall, an undo): the plug-in's remote
            // load path; an unknown command, so the script stays.
            assert!(r.readers.lock().unwrap()[0].load_state(b"NOP").is_some(), "{format:?}: the load reached the plug-in");
            r.run_until(1024, size);
            let want = vec![(10, "on", 0, 60, 127, 0), (512, "off", 0, 60, 0, 0), (600, "on", 0, 62, 127, 1), (650, "off", 0, 62, 0, 1)];
            assert_eq!(r.heard(), want, "{format:?} block {size}");
            let log = r.log();
            let first = log.iter().position(|l| l.starts_with("process")).unwrap();
            let reset = match format {
                Format::Vst3 => log.iter().position(|l| l.starts_with("setProcessing 1")),
                Format::Clap => log.iter().position(|l| l == "reset"),
            };
            assert!(reset.is_some_and(|i| i < first), "{format:?} block {size}: the reset before the next call: {log:?}");
        }
    }
}

