//! Allocation probe for the plug-in hosts' event paths (F10, ADR 0012 G5): with the VST3 and CLAP
//! fixtures' events plug-ins (which allocate nothing themselves), saturating the note and
//! controller queues and the timed parameter points, then processing the block (the VST3
//! parameter point slab included), allocates and frees nothing on the calling thread.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 1024;

/// Builds a fixture crate (outside the workspace) and returns its library.
fn build(fixture: &str, features: &[&str], target_dir: &str, lib: [&str; 3]) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../{fixture}/Cargo.toml"));
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join(target_dir);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .args(features)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("run cargo");
    assert!(st.success(), "building {fixture} failed");
    let name = if cfg!(target_os = "macos") {
        lib[0]
    } else if cfg!(windows) {
        lib[1]
    } else {
        lib[2]
    };
    target.join("debug").join(name)
}

fn vst3_events() -> Box<dyn Plugin> {
    // Installed once per binary: tests run in parallel and share the registry.
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    transit_vst3_host::create(ID.get_or_init(vst3_install)).unwrap()
}

fn vst3_install() -> String {
    let built = build("vst3-host/tests/fixture", &[], "mix-vst3-fixture-target", ["libvst3_fixture.dylib", "vst3_fixture.dll", "libvst3_fixture.so"]);
    let (folder, exe) = if cfg!(target_os = "macos") {
        ("MacOS", "Fixture")
    } else if cfg!(windows) {
        ("x86_64-win", "Fixture.vst3")
    } else {
        (if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Fixture.so")
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mix-vst3-events-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("vendor/Fixture.vst3/Contents").join(folder);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(&built, bin.join(exe)).unwrap();
    // Registered first: an id the registry does not know would make `create` scan every
    // plug-in on the machine.
    assert_eq!(transit_vst3_host::add_search_dir(&dir), 3);
    let found = transit_vst3_host::scan_paths(&[dir]);
    found.iter().find(|d| d.name == "Fixture Events").unwrap_or_else(|| panic!("{found:?}")).id.clone()
}

fn clap_events(midi: bool) -> Box<dyn Plugin> {
    static INSTALL: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALL.get_or_init(|| {
        let built = build(
            "clap-host/tests/fixture",
            &["--features", "events"],
            "mix-clap-events-target",
            ["libclap_fixture.dylib", "clap_fixture.dll", "libclap_fixture.so"],
        );
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mix-clap-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if cfg!(target_os = "macos") {
            let exe = dir.join("Events.clap/Contents/MacOS");
            std::fs::create_dir_all(&exe).unwrap();
            std::fs::copy(&built, exe.join("Events")).unwrap();
        } else {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(&built, dir.join("Events.clap")).unwrap();
        }
        assert_eq!(transit_clap_host::add_search_dir(&dir), 2);
    });
    transit_clap_host::create(if midi { "clap:org.soundcraft.test.events-midi" } else { "clap:org.soundcraft.test.events" }).unwrap()
}

/// Saturates every queue of `p`, then processes one block; returns what was refused.
fn saturate_and_process(p: &mut dyn Plugin, io: &mut [Vec<f32>]) -> (usize, usize) {
    let n = NoteId::NONE;
    let mut refused = (0, 0);
    let mut send = |p: &mut dyn Plugin, offset: u32, ev: PluginEvent| {
        if !p.event(offset, &ev) {
            if ev.is_release() {
                refused.1 += 1;
            } else {
                refused.0 += 1;
            }
        }
    };
    for i in 0..1200u32 {
        let (channel, key) = ((i % 16) as u8, (i % 128) as u8);
        send(p, i % BLOCK as u32, PluginEvent { channel, note_id: i as i32, message: Message::NoteOn { key, velocity: 90, id: n } });
        send(p, i % BLOCK as u32, PluginEvent { channel, note_id: -1, message: Message::Control { cc: 64, value: 127 } });
        p.set_param_at(i % BLOCK as u32, "5", (i % 100) as f32 / 100.0);
    }
    for i in 0..256u32 {
        let (channel, key) = ((i % 16) as u8, (i % 128) as u8);
        send(p, BLOCK as u32 - 1, PluginEvent { channel, note_id: i as i32, message: Message::NoteOff { key, velocity: 0, id: n } });
    }
    p.process(io, BLOCK);
    refused
}

#[test]
fn host_event_queues_and_the_vst3_point_slab_allocate_nothing_at_saturation() {
    for name in ["vst3", "clap", "clap-midi"] {
        let mut p = if name == "vst3" { vst3_events() } else { clap_events(name == "clap-midi") };
        p.prepare(48_000.0, BLOCK, 2);
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        // Warm up (not measured): the first blocks.
        for _ in 0..2 {
            saturate_and_process(&mut *p, &mut io);
        }
        let (refused, stats) = measure(|| saturate_and_process(&mut *p, &mut io));
        println!("{name}: {stats:?}, refused (non-release, release) {refused:?}");
        assert_eq!(stats, AllocStats::default(), "{name}: on the audio thread");
        assert!(refused.0 > 0, "{name}: saturated");
        assert_eq!(refused.1, 0, "{name}: every release fits the reserve");
        // Stop with pedals down and notes held: the reset keeps every release (review F10-1).
        saturate_and_process(&mut *p, &mut io);
        let ((), stats) = measure(|| {
            for channel in 0..16u8 {
                p.event(0, &PluginEvent { channel, note_id: -1, message: Message::Control { cc: 64, value: 0 } });
            }
            p.all_notes_off();
            p.reset();
            p.process(&mut io, BLOCK);
        });
        assert_eq!(stats, AllocStats::default(), "{name}: a reset keeping releases, on the audio thread");
    }
}

/// Observe the fixture before the mixer applies gain/pan; the SDK records are exact integers.
struct OffObserver {
    inner: Box<dyn Plugin>,
    offs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Plugin for OffObserver {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, channels: usize) {
        self.inner.prepare(sr, block, channels);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.inner.event(offset, ev)
    }
    fn all_notes_off(&mut self) {
        self.inner.all_notes_off();
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
        for r in io[0].as_chunks::<4>().0.iter().take_while(|r| r[0] != 0.0) {
            if r[0] == 5.0 && r[2] == 0x83_3C_00 as f32 {
                self.offs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

#[test]
fn midi_dialect_source_revocation_keeps_the_other_source_sounding_without_allocating() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use transit_mix::*;
    use transit_model::{ChannelFormat, Insert, Session, TrackKind};
    MixEngine::force_serial(true);
    let offs = Arc::new(AtomicUsize::new(0));
    let mut plugin = clap_events(true);
    plugin.prepare(48_000.0, BLOCK, 2);
    let mut plugin: Option<Box<dyn Plugin>> = Some(Box::new(OffObserver { inner: plugin, offs: Arc::clone(&offs) }));
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let mut mix = MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default());
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(track).unwrap().instrument = Some(Insert::new("clap:org.soundcraft.test.events-midi"));
    let mut make = |_: &InstanceSpec, _: SlotGen| plugin.take().map(|plugin| NewInstance { plugin, refused_state: false });
    let mut env = comp.compile(Arc::new(s), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    let binding = comp.active_bindings().into_iter().find(|b| b.1 == INSTRUMENT_SLOT).unwrap();
    let mut io = vec![vec![0.0; BLOCK]; 2];
    mix.render_active(0, BLOCK, &mut io);
    let ((), stats) = measure(|| {
        for port in [1, 2] {
            assert_eq!(
                mix.queue_event(&InstrumentEvent {
                    engine: EngineEpoch::FIRST,
                    target: binding,
                    due: Due::Next,
                    transport: None,
                    source: Source { port, channel: 3, generation: 1 },
                    message: Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE },
                    arrival: Arrival { host_ns: 0, sequence: 0 },
                }),
                EventOutcome::Queued
            );
        }
        mix.render_active(BLOCK as i64, BLOCK, &mut io);
        assert_eq!(offs.load(Ordering::Relaxed), 0);
        mix.revoke_source(1, 1);
        mix.render_active(2 * BLOCK as i64, BLOCK, &mut io);
        assert_eq!(offs.load(Ordering::Relaxed), 0, "unplugging one source must not emit an off");
        mix.revoke_source(2, 1);
        mix.render_active(3 * BLOCK as i64, BLOCK, &mut io);
        assert_eq!(offs.load(Ordering::Relaxed), 1, "the final unplug must emit exactly one off");
        assert!(!mix.has_pending_events());
    });
    assert_eq!(stats, AllocStats::default());
}

/// Records the fixture's parameter points for parameter 5 (record code 4: offset, id, value x 1e6)
/// before the mixer applies gain/pan, into a log preallocated before the measured blocks.
struct ParamRecorder {
    inner: Box<dyn Plugin>,
    log: std::sync::Arc<std::sync::Mutex<Vec<(u32, i64)>>>,
}

impl Plugin for ParamRecorder {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, channels: usize) {
        self.inner.prepare(sr, block, channels);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn set_param_at(&mut self, offset: u32, id: &str, value: f32) -> bool {
        self.inner.set_param_at(offset, id, value)
    }
    fn sample_accurate_params(&self) -> bool {
        self.inner.sample_accurate_params()
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.inner.event(offset, ev)
    }
    fn all_notes_off(&mut self) {
        self.inner.all_notes_off();
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
        let mut log = self.log.lock().unwrap();
        for r in io[0].as_chunks::<4>().0.iter().take_while(|r| r[0] != 0.0) {
            if r[0] == 4.0 && r[2] == 5.0 && log.len() < log.capacity() {
                log.push((r[1] as u32, r[3] as i64));
            }
        }
    }
}

/// #117: an automation lane with three breakpoints inside one 512-frame block reaches a hosted
/// instrument as three parameter changes at their sample offsets (VST3: one queue with a point
/// per change; CLAP: one `clap_event_param_value` per change with its `time`), after the curve's
/// value at the block start, and the block allocates nothing.
fn automation_points_arrive_at_their_offsets(name: &str, plugin_id: &str, plugin: Box<dyn Plugin>) {
    use std::sync::{Arc, Mutex};
    use transit_mix::*;
    use transit_model::{AutoParam, AutomationLane, AutomationMode, ChannelFormat, Insert, Session, TrackKind};
    const B: usize = 512;
    MixEngine::force_serial(true);
    let log = Arc::new(Mutex::new(Vec::with_capacity(64)));
    let mut plugin: Option<Box<dyn Plugin>> = Some(Box::new(ParamRecorder { inner: plugin, log: Arc::clone(&log) }));
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: B as u32, out_channels: 0, in_channels: 0 };
    let mut mix = MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default());
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let t = s.track_mut(track).unwrap();
    let mut inst = Insert::new(plugin_id);
    inst.params.insert("5".into(), 0.0);
    t.instrument = Some(inst);
    t.mixer.automation_mode = AutomationMode::Read;
    let mut lane = AutomationLane::new(AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "5".into() });
    // The second block is [512, 1024): breakpoints at offsets 88, 188 and 388.
    for (at, v) in [(0, 0.0), (600, 0.25), (700, 0.5), (900, 0.75)] {
        lane.set_point(at, v);
    }
    t.automation.push(lane);
    let mut make = |_: &InstanceSpec, _: SlotGen| plugin.take().map(|plugin| NewInstance { plugin, refused_state: false });
    let mut env = comp.compile(Arc::new(s), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted, "{name}");
    comp.reclaim(env);
    let mut io = vec![vec![0.0; B]; 2];
    // Warm up (not measured): the first block.
    mix.render_active(0, B, &mut io);
    log.lock().unwrap().clear();
    let ((), stats) = measure(|| mix.render_active(B as i64, B, &mut io));
    assert_eq!(stats, AllocStats::default(), "{name}: the timed points allocate nothing on the audio thread");
    let got = log.lock().unwrap().clone();
    // The curve at the block start: 0.25 * 512 / 600.
    let (start, rest) = got.split_first().unwrap_or_else(|| panic!("{name}: no points"));
    assert_eq!(start.0, 0, "{name}: the block-start value first: {got:?}");
    assert!((start.1 - 213_333).abs() <= 2, "{name}: the block-start value: {got:?}");
    assert_eq!(rest, [(88, 250_000), (188, 500_000), (388, 750_000)], "{name}: each breakpoint at its offset");
}

#[test]
fn vst3_automation_points_arrive_at_their_offsets() {
    let p = vst3_events();
    let id = p.info().id;
    automation_points_arrive_at_their_offsets("vst3", id, p);
}

#[test]
fn clap_automation_points_arrive_at_their_offsets() {
    automation_points_arrive_at_their_offsets("clap", "clap:org.soundcraft.test.events", clap_events(false));
}
