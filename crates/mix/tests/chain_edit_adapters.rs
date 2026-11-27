//! ADR 0057 slice P3a (decision 2.13) through the engine at real instruments.
//!
//! **The VST3 adapter.** The cases of decision 13's P3a evidence run through the real VST3 adapter's
//! admission, native translation and processing boundary: the instrument is the VST3 fixture's
//! events synth in record mode (as `crates/vst3-host/tests/delivery_e2e.rs` drives it), which logs
//! every note event and mapped controller point it receives, per call, so the frames the plug-in
//! sees are checked at block sizes 1, 7, 64 and 512. The voice state is evaluated on those native
//! records with the plug-in reading each frame's points before its note events and after them, and
//! both readings must give the stated result. The only delays allowed are ADR 0059 decision 3b's
//! separations (at one frame of one channel, a pedal edge is moved to the next frame behind a note
//! event or another edge, in arrival order). The fixture maps CC 64 on channel c to parameter
//! 100 + c, and CC 66, 67, 69, 120, 121, 123 to 200, 300, 400, 500, 600, 700 + c.
//!
//! **A built-in instrument.** The pinned limit of 2.13 (70 notes and a held pedal at an edit, every
//! release refused in its block) ends with the engine's Stop at a real Macro Oscillator: nothing
//! sounds afterwards.
//!
//! Expected results are written by hand from 2.13 and ADR 0059 3b.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_mix::{
    Adoption, Arrival, CompileMode, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent,
    MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderFrame, RenderTarget, SlotGen, Source,
};
use transit_model::{ChannelFormat, Insert, MidiEffectSlot, Session, TrackId, TrackKind};

const SR: u32 = 48_000;
const MAX_BLOCK: usize = 512;
const SIZES: [usize; 4] = [1, 7, 64, 512];
/// The edit's frame: a block start at every size.
const E: u64 = 3584;
const OWN: &str = "midi_test_owning";

// ---- the VST3 fixture ----

fn build_vst3_fixture() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vst3-host/tests/fixture/Cargo.toml");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mix-chain-edit-vst3-fixture-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("run cargo");
    assert!(st.success(), "building the VST3 fixture failed");
    let lib = if cfg!(target_os = "macos") {
        "libvst3_fixture.dylib"
    } else if cfg!(windows) {
        "vst3_fixture.dll"
    } else {
        "libvst3_fixture.so"
    };
    target.join("debug").join(lib)
}

/// The events synth's id, installed once per binary (tests share the registry).
fn vst3_id() -> String {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let built = build_vst3_fixture();
        let (folder, exe) = if cfg!(target_os = "macos") {
            ("MacOS", "Fixture")
        } else if cfg!(windows) {
            ("x86_64-win", "Fixture.vst3")
        } else {
            (if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Fixture.so")
        };
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mix-chain-edit-vst3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("vendor/Fixture.vst3/Contents").join(folder);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(&built, bin.join(exe)).unwrap();
        // Registered first: an id the registry does not know would make `create` scan every
        // plug-in on the machine.
        assert_eq!(transit_vst3_host::add_search_dir(&dir), 3);
        let found = transit_vst3_host::scan_paths(&[dir]);
        found.iter().find(|d| d.name == "Fixture Events").unwrap_or_else(|| panic!("{found:?}")).id.clone()
    })
    .clone()
}

/// One native record at its frame from the rig's start: 1 on, 2 off (a = key, b = note id);
/// 4 point (a = parameter id, b = value x 1e6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rec {
    frame: u64,
    code: u32,
    a: u32,
    b: i64,
}

type Recs = Arc<Mutex<Vec<Rec>>>;
type Offered = Arc<Mutex<Vec<(u64, PluginEvent)>>>;

/// The real adapter, read after each call: its record channel, and what `event` accepted.
struct Native {
    inner: Box<dyn Plugin>,
    recs: Recs,
    offered: Offered,
    frame: u64,
}

impl Native {
    fn read_log(&mut self) {
        let state = self.inner.save_state().unwrap();
        let at = state.windows(4).position(|w| w == b"REC1").expect("the record channel");
        let n = u32::from_le_bytes(state[at + 4..at + 8].try_into().unwrap()) as usize;
        let mut recs = self.recs.lock().unwrap();
        let mut call = self.frame;
        let mut base = self.frame;
        for k in 0..n {
            let o = at + 8 + 16 * k;
            let f = |i: usize| f32::from_le_bytes(state[o + 4 * i..o + 4 * i + 4].try_into().unwrap());
            let (code, x, a, b) = (f(0) as u32, f(1) as u64, f(2) as u32, f(3) as i64);
            if code == 9 {
                call = base;
                base += x;
            } else {
                recs.push(Rec { frame: call + x, code, a: a % 1000, b });
            }
        }
    }
}

impl Plugin for Native {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
        self.read_log();
        self.frame += frames as u64;
    }
    fn latency(&self) -> usize {
        self.inner.latency()
    }
    fn process_errors(&self) -> u64 {
        self.inner.process_errors()
    }
    fn params_pending(&self) -> bool {
        self.inner.params_pending()
    }
    fn set_transport(&mut self, t: &transit_dsp::Transport) {
        self.inner.set_transport(t);
    }
    fn all_notes_off(&mut self) {
        self.inner.all_notes_off();
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        let ok = self.inner.event(offset, ev);
        if ok {
            self.offered.lock().unwrap().push((self.frame + u64::from(offset), *ev));
        }
        ok
    }
    fn set_param_at(&mut self, offset: u32, id: &str, value: f32) -> bool {
        self.inner.set_param_at(offset, id, value)
    }
    fn sample_accurate_params(&self) -> bool {
        self.inner.sample_accurate_params()
    }
}

/// A built-in instrument whose `event` refuses every release while `refuse` is set (the case the
/// record pins: a load that makes a built-in refuse in an edit's block).
struct Refusing {
    inner: Box<dyn Plugin>,
    refuse: Arc<AtomicBool>,
}

impl Plugin for Refusing {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn all_notes_off(&mut self) {
        self.inner.all_notes_off();
    }
    fn set_transport(&mut self, t: &transit_dsp::Transport) {
        self.inner.set_transport(t);
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if ev.is_release() && self.refuse.load(Ordering::Relaxed) {
            return false;
        }
        self.inner.event(offset, ev)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    First,
    Later,
}

const ATS: [At; 2] = [At::First, At::Later];

enum Kind {
    Vst3,
    Builtin(Arc<AtomicBool>),
}

struct Rig {
    mix: MixEngine,
    comp: PlanCompiler,
    generation: PlanGeneration,
    session: Session,
    track: TrackId,
    kind: Kind,
    recs: Recs,
    offered: Offered,
    pos: i64,
    now: u64,
    out: Vec<Vec<f32>>,
}

impl Rig {
    fn new(kind: Kind, chain: &[&str]) -> Rig {
        let id = match kind {
            Kind::Vst3 => vst3_id(),
            Kind::Builtin(_) => "macro_oscillator".to_string(),
        };
        let mut session = Session::default();
        session.edit.click = false;
        let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        session.track_mut(track).unwrap().instrument = Some(Insert::new(&id));
        let target = RenderTarget { device: DeviceGeneration(3), sample_rate: SR, max_block: MAX_BLOCK as u32, out_channels: 0, in_channels: 0 };
        let mut rig = Rig {
            mix: MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default()),
            comp: PlanCompiler::new(target, PlanLimits::default()),
            generation: PlanGeneration::EMPTY,
            session,
            track,
            kind,
            recs: Arc::default(),
            offered: Arc::default(),
            pos: 0,
            now: 0,
            out: vec![vec![0.0; MAX_BLOCK]; 2],
        };
        rig.chain(chain);
        rig.now = rig.mix.next_frame().0;
        rig
    }

    /// Publishes the session with `chain` as the track's MIDI effects (kept slots keep their ids).
    fn chain(&mut self, chain: &[&str]) {
        let mut s = self.session.clone();
        let t = s.track_mut(self.track).unwrap();
        t.midi_effects.truncate(chain.len());
        for id in chain.iter().skip(t.midi_effects.len()) {
            t.midi_effects.push(MidiEffectSlot::new(*id));
        }
        self.publish(s);
    }

    fn publish(&mut self, s: Session) {
        self.generation = self.generation.checked_next().unwrap();
        let (recs, offered) = (Arc::clone(&self.recs), Arc::clone(&self.offered));
        let refuse = match &self.kind {
            Kind::Builtin(r) => Some(Arc::clone(r)),
            Kind::Vst3 => None,
        };
        let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            let plugin: Box<dyn Plugin> = match &refuse {
                None => {
                    let mut inner = transit_vst3_host::create(&spec.id).unwrap();
                    inner.prepare(SR as f32, MAX_BLOCK, spec.ch);
                    assert!(inner.load_state(b"REC"), "record mode");
                    let _ = inner.save_state(); // empty the log of the creation's calls
                    Box::new(Native { inner, recs: Arc::clone(&recs), offered: Arc::clone(&offered), frame: 0 })
                }
                Some(r) => {
                    let mut inner = transit_dsp::create(&spec.id)?;
                    inner.prepare(SR as f32, MAX_BLOCK, spec.ch);
                    Box::new(Refusing { inner, refuse: Arc::clone(r) })
                }
            };
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = self.comp.compile_with_mode(Arc::new(s.clone()), self.comp.active(), self.generation, CompileMode::Update, &mut make).unwrap();
        assert_eq!(self.mix.adopt_plan(&mut env), Adoption::Adopted);
        self.comp.reclaim(env);
        self.session = s;
    }

    /// A live event from `port` (channel 0), due `at` frames after the rig's start.
    fn live(&mut self, port: u16, at: u64, message: Message) {
        let target = self.comp.active_bindings().into_iter().find(|b| b.0 == self.track && b.1 == INSTRUMENT_SLOT).unwrap();
        let ev = InstrumentEvent {
            engine: EngineEpoch::FIRST,
            target,
            due: Due::Frame(RenderFrame(self.now + at)),
            transport: None,
            source: Source { port, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: 0 },
        };
        assert_eq!(self.mix.queue_event(&ev), EventOutcome::Queued, "{message:?} at {at}");
    }

    /// A clip whose CC 64 is `value` at `tick` (25 frames a tick at 120 BPM and 48 kHz).
    fn clip_pedal(&mut self, tick: i64, value: i32) {
        let mut s = self.session.clone();
        let mut seq = transit_midi::Sequence::default();
        seq.ctrls.push(transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(64), value });
        let cid = s.new_clip_id();
        s.track_mut(self.track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "pedal", 0, 96_000, seq));
        self.publish(s);
    }

    /// Renders blocks of `size` (the last one shorter) until `end` frames after the start.
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

    fn recs(&self) -> Vec<Rec> {
        self.recs.lock().unwrap().clone()
    }

    /// The native records from frame `from` on, sorted (the fixture lists a call's notes, then its
    /// points per parameter queue), with parameter ids reduced to channel 0's.
    fn native_from(&self, from: u64) -> Vec<(u64, &'static str, u32, i64)> {
        let mut r: Vec<Rec> = self.recs().into_iter().filter(|r| r.frame >= from).collect();
        r.sort_unstable();
        r.iter()
            .map(|r| match r.code {
                1 => (r.frame, "on", r.a, 0),
                2 => (r.frame, "off", r.a, 0),
                _ => (r.frame, "pt", r.a, r.b),
            })
            .collect()
    }

    /// What the adapter accepted from frame `from` on: (frame, kind, key or controller, value).
    fn offered_from(&self, from: u64) -> Vec<(u64, &'static str, u8, u16)> {
        self.offered
            .lock()
            .unwrap()
            .iter()
            .filter(|(f, _)| *f >= from)
            .map(|(f, e)| match e.message {
                Message::NoteOn { key, .. } => (*f, "on", key, 0),
                Message::NoteOff { key, .. } => (*f, "off", key, 0),
                Message::Control { cc, value } => (*f, "cc", cc, u16::from(value)),
                Message::PitchBend { value } => (*f, "bend", 0, value),
                _ => (*f, "other", 0, 0),
            })
            .collect()
    }
}

// ---- the plug-in's voices, read from the native records ----

/// Held voices and voices a pedal keeps (by note id), the sostenuto catch, and channel 0's pedals.
#[derive(Default, Debug, Clone)]
struct Voices {
    held: BTreeSet<i64>,
    sustained: BTreeSet<i64>,
    caught: BTreeSet<i64>,
    pedal: BTreeMap<u32, bool>,
}

impl Voices {
    fn down(&self, cc: u32) -> bool {
        self.pedal.get(&cc).copied().unwrap_or(false)
    }
    fn let_go(&mut self) {
        let (sus, sost) = (self.down(64) || self.down(69), self.down(66));
        let caught = self.caught.clone();
        self.sustained.retain(|k| sus || (sost && caught.contains(k)));
    }
    fn release(&mut self, id: i64) {
        if self.held.remove(&id) && (self.down(64) || self.down(69) || (self.down(66) && self.caught.contains(&id))) {
            self.sustained.insert(id);
        }
    }
    fn apply(&mut self, r: &Rec) {
        match (r.code, r.a / 100) {
            (1, _) => {
                self.held.insert(r.b);
            }
            (2, _) => self.release(r.b),
            (4, 1..=4) => {
                let cc = [0, 64, 66, 67, 69][(r.a / 100) as usize];
                let down = r.b >= 500_000;
                if cc == 66 && down && !self.down(66) {
                    self.caught = self.held.clone();
                }
                self.pedal.insert(cc, down);
                if cc == 66 && !down {
                    self.caught.clear();
                }
                self.let_go();
            }
            (4, 5) => {
                self.held.clear();
                self.sustained.clear();
            }
            (4, 6) => {
                for cc in [64, 66, 67, 69] {
                    self.pedal.insert(cc, false);
                }
                self.caught.clear();
                self.let_go();
            }
            (4, 7) => {
                for id in self.held.clone() {
                    self.release(id);
                }
            }
            _ => {}
        }
    }
}

/// Plays the records frame by frame, reading each frame's points before (or after) its notes;
/// calls `each(frame, voices)` after each frame.
fn replay(recs: &[Rec], points_first: bool, mut each: impl FnMut(u64, &Voices)) -> Voices {
    let mut frames: BTreeMap<u64, Vec<Rec>> = BTreeMap::new();
    for r in recs {
        frames.entry(r.frame).or_default().push(*r);
    }
    let mut v = Voices::default();
    for (f, rs) in frames {
        let (points, notes): (Vec<Rec>, Vec<Rec>) = rs.iter().partition(|r| r.code == 4);
        let (a, b) = if points_first { (points, notes) } else { (notes, points) };
        for r in a.iter().chain(b.iter()) {
            v.apply(r);
        }
        each(f, &v);
    }
    v
}

/// The frame each voice of `key` stops sounding, in both readings (`None`: still sounding).
fn ends(recs: &[Rec], key: u32) -> [Vec<Option<u64>>; 2] {
    let ids: Vec<i64> = recs.iter().filter(|r| r.code == 1 && r.a == key).map(|r| r.b).collect();
    [true, false].map(|points_first| {
        let mut end: Vec<Option<u64>> = vec![None; ids.len()];
        let mut started = vec![false; ids.len()];
        replay(recs, points_first, |f, v| {
            for (k, id) in ids.iter().enumerate() {
                let sounding = v.held.contains(id) || v.sustained.contains(id);
                if sounding {
                    started[k] = true;
                } else if started[k] && end[k].is_none() {
                    end[k] = Some(f);
                }
            }
        });
        end
    })
}

/// In both readings at the end: nothing sounds, and channel 0's pedal `cc` is down or up.
fn settled(recs: &[Rec], cc: u32, down: bool, what: &str) {
    for points_first in [true, false] {
        let v = replay(recs, points_first, |_, _| {});
        assert!(v.held.is_empty() && v.sustained.is_empty(), "{what}: nothing sounds (points first: {points_first}): {v:?}");
        assert_eq!(v.down(cc), down, "{what}: CC {cc} (points first: {points_first})");
    }
}

/// The chains before and after the edit: the owning pass-through added, onto nothing (the first
/// insertion) or onto an owning pass-through already there (a later edit).
fn rig_at(at: At) -> Rig {
    Rig::new(Kind::Vst3, if at == At::First { &[] } else { &[OWN] })
}

fn edit(r: &mut Rig, at: At, size: usize) {
    r.run_until(E, size);
    r.chain(if at == At::First { &[OWN] } else { &[OWN, OWN] });
}

// ---- the cases through the real VST3 adapter ----

#[test]
fn vst3_a_held_identified_key_under_the_pedal_ends_one_frame_after_the_edit() {
    for at in ATS {
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            r.live(1, 5, Message::Control { cc: 64, value: 127 });
            r.live(1, 10, Message::NoteOn { key: 60, velocity: 100, id: NoteId(42) });
            r.live(1, E + 36, Message::NoteOff { key: 60, velocity: 0, id: NoteId(42) });
            edit(&mut r, at, size);
            r.run_until(E + 256, size);
            assert_eq!(r.offered_from(E), [(E, "off", 60, 0), (E, "cc", 64, 0)], "{ctx}: the note-off, then the pedal-up, at offset 0");
            // 3b: the pedal-up is separated from the note-off onto the next frame.
            assert_eq!(r.native_from(E), [(E, "off", 60, 0), (E + 1, "pt", 100, 0)], "{ctx}: native");
            assert_eq!(ends(&r.recs(), 60), [vec![Some(E + 1)], vec![Some(E + 1)]], "{ctx}");
            settled(&r.recs(), 64, false, &ctx);
        }
    }
}

#[test]
fn vst3_a_note_held_only_by_the_pedal_ends_at_the_edit() {
    for at in ATS {
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            r.live(1, 5, Message::Control { cc: 64, value: 127 });
            r.live(1, 10, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE });
            r.live(1, 20, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE });
            edit(&mut r, at, size);
            r.run_until(E + 256, size);
            assert_eq!(r.native_from(E), [(E, "pt", 100, 0)], "{ctx}: the pedal-up alone, at the edit");
            assert_eq!(ends(&r.recs(), 60), [vec![Some(E)], vec![Some(E)]], "{ctx}");
            settled(&r.recs(), 64, false, &ctx);
        }
    }
}

#[test]
fn vst3_after_a_lift_and_a_new_press_the_old_key_is_neither_sustained_nor_captured_again() {
    for (pedal, param) in [(64u8, 100u32), (66, 200)] {
        for at in ATS {
            for size in SIZES {
                let ctx = format!("CC {pedal} {at:?} block {size}");
                let mut r = rig_at(at);
                let cc = |value| Message::Control { cc: pedal, value };
                r.live(1, 5, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE });
                r.live(1, 8, cc(127));
                r.live(1, E + 36, cc(0));
                r.live(1, E + 56, cc(127));
                r.live(1, E + 86, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE });
                r.live(1, E + 96, Message::NoteOn { key: 62, velocity: 100, id: NoteId::NONE });
                r.live(1, E + 106, Message::NoteOff { key: 62, velocity: 0, id: NoteId::NONE });
                r.live(1, E + 136, cc(0));
                edit(&mut r, at, size);
                r.run_until(E + 256, size);
                assert_eq!(
                    r.native_from(E),
                    [
                        (E, "off", 60, 0),
                        (E + 1, "pt", param, 0),
                        (E + 56, "pt", param, 1_000_000),
                        (E + 96, "on", 62, 0),
                        (E + 106, "off", 62, 0),
                        (E + 136, "pt", param, 0)
                    ],
                    "{ctx}: the stale lift at E + 36 sends nothing; the press and its lift pass"
                );
                assert_eq!(ends(&r.recs(), 60), [vec![Some(E + 1)], vec![Some(E + 1)]], "{ctx}: 60 ends at the separated pedal-up");
                let end62 = if pedal == 64 { E + 136 } else { E + 106 };
                assert_eq!(ends(&r.recs(), 62), [vec![Some(end62)], vec![Some(end62)]], "{ctx}: 62");
                settled(&r.recs(), u32::from(pedal), false, &ctx);
            }
        }
    }
}

#[test]
fn vst3_a_pedal_held_through_an_edit_sustains_nothing_until_it_is_pressed_again() {
    for at in ATS {
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            let cc = |value| Message::Control { cc: 64, value };
            r.live(1, 5, cc(127));
            r.live(1, E + 16, Message::NoteOn { key: 62, velocity: 100, id: NoteId::NONE });
            r.live(1, E + 26, Message::NoteOff { key: 62, velocity: 0, id: NoteId::NONE });
            r.live(1, E + 36, cc(0));
            r.live(1, E + 56, cc(127));
            r.live(1, E + 66, Message::NoteOn { key: 64, velocity: 100, id: NoteId::NONE });
            r.live(1, E + 76, Message::NoteOff { key: 64, velocity: 0, id: NoteId::NONE });
            r.live(1, E + 136, cc(0));
            edit(&mut r, at, size);
            r.run_until(E + 256, size);
            assert_eq!(
                r.native_from(E),
                [
                    (E, "pt", 100, 0),
                    (E + 16, "on", 62, 0),
                    (E + 26, "off", 62, 0),
                    (E + 56, "pt", 100, 1_000_000),
                    (E + 66, "on", 64, 0),
                    (E + 76, "off", 64, 0),
                    (E + 136, "pt", 100, 0)
                ],
                "{ctx}"
            );
            assert_eq!(ends(&r.recs(), 62), [vec![Some(E + 26)], vec![Some(E + 26)]], "{ctx}: not sustained");
            assert_eq!(ends(&r.recs(), 64), [vec![Some(E + 136)], vec![Some(E + 136)]], "{ctx}: sustained by the new press");
        }
    }
}

#[test]
fn vst3_two_sources_holding_the_pedal_through_an_edit_send_nothing_at_their_releases() {
    for at in ATS {
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            let cc = |value| Message::Control { cc: 64, value };
            r.live(1, 5, cc(127));
            r.live(2, 6, cc(127));
            r.live(1, E + 36, cc(0));
            r.live(1, E + 38, Message::NoteOn { key: 62, velocity: 100, id: NoteId::NONE });
            r.live(1, E + 40, Message::NoteOff { key: 62, velocity: 0, id: NoteId::NONE });
            r.live(2, E + 46, cc(0));
            r.live(1, E + 56, cc(127));
            edit(&mut r, at, size);
            r.run_until(E + 256, size);
            assert_eq!(
                r.native_from(E),
                [(E, "pt", 100, 0), (E + 38, "on", 62, 0), (E + 40, "off", 62, 0), (E + 56, "pt", 100, 1_000_000)],
                "{ctx}: neither release sends anything; the next press passes"
            );
            assert_eq!(ends(&r.recs(), 62), [vec![Some(E + 40)], vec![Some(E + 40)]], "{ctx}");
            settled(&r.recs(), 64, true, &ctx);
        }
    }
}

#[test]
fn vst3_a_clip_pedal_down_one_frame_after_an_edit_follows_the_edits_pedal_up() {
    // The edit at 39,424 (a block start at every size); the clip's press at tick 1,577, frame 39,425.
    const EDIT: u64 = 39_424;
    for at in ATS {
        let mut renders = Vec::new();
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            r.clip_pedal(1577, 127);
            r.live(1, 5, Message::Control { cc: 64, value: 127 });
            r.live(1, 10, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE });
            r.run_until(EDIT, size);
            r.chain(if at == At::First { &[OWN] } else { &[OWN, OWN] });
            r.run_until(EDIT + 1024, size);
            assert_eq!(
                r.offered_from(EDIT),
                [(EDIT, "off", 60, 0), (EDIT, "cc", 64, 0), (EDIT + 1, "cc", 64, 127)],
                "{ctx}: the press is offered on its frame"
            );
            // 3b: the pedal-up waits behind the note-off, the press behind the pedal-up.
            let native = r.native_from(EDIT);
            assert_eq!(native, [(EDIT, "off", 60, 0), (EDIT + 1, "pt", 100, 0), (EDIT + 2, "pt", 100, 1_000_000)], "{ctx}");
            assert_eq!(ends(&r.recs(), 60), [vec![Some(EDIT + 1)], vec![Some(EDIT + 1)]], "{ctx}");
            settled(&r.recs(), 64, true, &ctx);
            renders.push(r.recs().into_iter().map(|x| (x.frame, x.code, x.a, x.b)).collect::<BTreeSet<_>>());
        }
        assert!(renders.windows(2).all(|w| w[0] == w[1]), "{at:?}: the same native records at every partition");
    }
}

#[test]
fn vst3_pitch_bend_and_the_mod_wheel_set_before_an_edit_are_kept() {
    for at in ATS {
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            r.live(1, 5, Message::PitchBend { value: 10_000 });
            r.live(1, 6, Message::Control { cc: 1, value: 90 });
            r.live(1, 10, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE });
            edit(&mut r, at, size);
            r.run_until(E + 256, size);
            assert_eq!(r.offered_from(E), [(E, "off", 60, 0)], "{ctx}: nothing offered for expression, no CC 120, 121 or 123");
            assert_eq!(r.native_from(E), [(E, "off", 60, 0)], "{ctx}: native");
            let before: Vec<_> = r.offered_from(0).into_iter().filter(|o| o.0 < E).collect();
            assert!(
                before.contains(&(5, "bend", 0, 10_000)) && before.contains(&(6, "cc", 1, 90)),
                "{ctx}: the wheels reached it before: {before:?}"
            );
        }
    }
}

#[test]
fn vst3_a_press_on_the_edits_first_frame_follows_the_pedal_up() {
    // The case the record's pre-R1-h sentence described (a press reaching the same call at the same
    // offset as the edit's pedal-up arrived as the press alone): a second source presses at the
    // edit's frame; the adapter keeps both edges, in arrival order, on successive frames.
    for at in ATS {
        for size in SIZES {
            let ctx = format!("{at:?} block {size}");
            let mut r = rig_at(at);
            r.live(1, 5, Message::Control { cc: 64, value: 127 });
            r.live(1, 10, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE });
            r.live(2, E, Message::Control { cc: 64, value: 127 });
            r.live(1, E + 100, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE });
            edit(&mut r, at, size);
            r.run_until(E + 256, size);
            assert_eq!(r.offered_from(E), [(E, "off", 60, 0), (E, "cc", 64, 0), (E, "cc", 64, 127)], "{ctx}");
            assert_eq!(r.native_from(E), [(E, "off", 60, 0), (E + 1, "pt", 100, 0), (E + 2, "pt", 100, 1_000_000)], "{ctx}");
            assert_eq!(ends(&r.recs(), 60), [vec![Some(E + 1)], vec![Some(E + 1)]], "{ctx}: the old voice is not sustained by the press");
            settled(&r.recs(), 64, true, &ctx);
        }
    }
}

// ---- the pinned limit at a real built-in instrument and the engine's Stop ----

#[test]
fn seventy_notes_and_a_pedal_refused_at_an_edit_are_silenced_by_stop_at_a_built_in() {
    let refuse = Arc::new(AtomicBool::new(false));
    let mut r = Rig::new(Kind::Builtin(Arc::clone(&refuse)), &[OWN]);
    r.live(1, 5, Message::Control { cc: 64, value: 127 });
    for k in 30u8..100 {
        r.live(1, 10, Message::NoteOn { key: k, velocity: 100, id: NoteId::NONE });
    }
    assert!(r.run_until(E, 64) > 0.0, "the notes sound");
    r.chain(&[OWN, OWN]);
    refuse.store(true, Ordering::Relaxed);
    r.run_until(E + 64, 64);
    refuse.store(false, Ordering::Relaxed);
    r.run_until(E + 128, 64);
    assert_eq!(r.mix.event_status().midi_fx.retry_overflow, 7, "six note-offs and the pedal-up lost (#600)");
    assert!(r.run_until(E + 1024, 64) > 0.0, "today's limit: voices still sound before Stop");
    // The engine's Stop.
    r.mix.reset();
    r.run_until(E + 1088, 64);
    let after = r.run_until(E + 4096, 64);
    assert_eq!(after, 0.0, "after Stop nothing sounds at the built-in instrument");
}
