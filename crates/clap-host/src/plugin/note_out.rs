//! Note output, the lifecycle generation and the input dialect per activation (ADR 0064,
//! decisions 3, 4.1 and 6), the safe side.

use super::{ClapPlugin, Core, NoteMode, Shared, note_mode};
use clap_sys::ext::note_ports::CLAP_NOTE_DIALECT_MIDI;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use transit_dsp::hosted_out::{Lifecycle, NoteOutputInfo, calls_due};

/// No readable output note port.
const NONE: u32 = u32::MAX;
/// The input dialect, packed: the note mode in the low bits, `MIDI_IN` when the port takes MIDI.
const MODE_CLAP: u8 = 1;
const MODE_MIDI: u8 = 2;
const MIDI_IN: u8 = 4;

/// What each activation publishes to the audio side, with the lifecycle generation bumped last
/// (while the instance's core is held), so a block that sees the new generation sees what the
/// activation read: the input dialect (input note port 0), the first readable output note port
/// and how many output note ports are declared.
pub(crate) struct LifeOut {
    generation: AtomicU64,
    alive: AtomicBool,
    dialect: AtomicU8,
    read: AtomicU32,
    declared: AtomicU32,
    /// Note output collection asked for (control thread), and in force for this activation.
    asked: AtomicBool,
    collecting: AtomicBool,
}

impl Default for LifeOut {
    fn default() -> Self {
        LifeOut {
            generation: AtomicU64::new(0),
            alive: AtomicBool::new(false),
            dialect: AtomicU8::new(0),
            read: AtomicU32::new(NONE),
            declared: AtomicU32::new(0),
            asked: AtomicBool::new(false),
            collecting: AtomicBool::new(false),
        }
    }
}

impl LifeOut {
    /// A new generation (caller holds the instance's core); `alive`: the instance is active.
    pub(crate) fn next_generation(&self, alive: bool) {
        self.alive.store(alive, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn lifecycle(&self) -> Lifecycle {
        let generation = self.generation();
        Lifecycle { generation, active: self.alive.load(Ordering::Relaxed) }
    }

    /// Asks for collection from the next activation on.
    pub(crate) fn ask_collection(&self, on: bool) {
        self.asked.store(on, Ordering::Relaxed);
    }

    /// Collection is in force for this activation (read under the core).
    pub(crate) fn collecting(&self) -> bool {
        self.collecting.load(Ordering::Relaxed)
    }

    pub(crate) fn read_port(&self) -> Option<u32> {
        let r = self.read.load(Ordering::Relaxed);
        (r != NONE).then_some(r)
    }

    pub(crate) fn note_output(&self) -> Option<NoteOutputInfo> {
        let declared = self.declared.load(Ordering::Relaxed);
        self.read_port().map(|read| NoteOutputInfo { read, declared })
    }
}

impl Shared {
    /// After an activation attempt (caller holds the core, main thread or `prepare`): the note
    /// ports read again in both directions and published, then a new generation.
    pub(super) fn publish_activation(&self, core: &Core, activated: bool) {
        let port = core.inst.note_port();
        let mode = match note_mode(port) {
            NoteMode::Clap => MODE_CLAP,
            NoteMode::Midi => MODE_MIDI,
            NoteMode::None => 0,
        };
        let midi = if port.is_some_and(|p| p.supported & CLAP_NOTE_DIALECT_MIDI != 0) { MIDI_IN } else { 0 };
        let (declared, read) = core.inst.note_outputs();
        let lo = &self.life_out;
        lo.dialect.store(mode | midi, Ordering::Relaxed);
        lo.declared.store(declared, Ordering::Relaxed);
        lo.read.store(read.unwrap_or(NONE), Ordering::Relaxed);
        lo.collecting.store(lo.asked.load(Ordering::Relaxed), Ordering::Relaxed);
        lo.next_generation(activated && core.inst.is_active());
    }
}

impl ClapPlugin {
    /// The input dialect the current activation published (ADR 0064, 4.1).
    pub(super) fn take_dialect(&mut self) {
        let d = self.shared.life_out.dialect.load(Ordering::Acquire);
        self.notes = match d & 3 {
            MODE_CLAP => NoteMode::Clap,
            MODE_MIDI => NoteMode::Midi,
            _ => NoteMode::None,
        };
        self.midi_in = d & MIDI_IN != 0;
    }

    /// A block of `frames` not processed: the native calls it was due are counted skipped.
    pub(super) fn skip_calls(&mut self, frames: usize) {
        self.note_out.skipped(calls_due(frames, self.block));
    }
}

/// Test seam: the parameter values every live instance's host state has queued for the edit path.
#[cfg(feature = "test-hooks")]
pub(crate) fn queued_edits() -> Vec<(u32, f64)> {
    let all: Vec<std::sync::Arc<Shared>> = super::lock(super::live()).iter().filter_map(std::sync::Weak::upgrade).collect();
    all.iter().flat_map(|s| super::lock(&s.host.edits).clone()).collect()
}
