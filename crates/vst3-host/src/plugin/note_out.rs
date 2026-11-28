//! Note output, the lifecycle generation and the stage reset (ADR 0064, decisions 3 and 6), the
//! safe side: what each native call of a block ran under, and what it wrote.

use super::{Core, Shared, Vst3Plugin};
use std::sync::atomic::Ordering;
use transit_dsp::hosted_out::{CallRecord, Lifecycle, NoteOutputInfo};

impl Shared {
    /// A new lifecycle generation (caller holds the instance's core): `alive` says whether the
    /// instance is active after it.
    pub(super) fn next_generation(&self, alive: bool) {
        self.alive.store(alive, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub(super) fn lifecycle(&self) -> Lifecycle {
        let generation = self.generation.load(Ordering::Acquire);
        Lifecycle { generation, active: self.alive.load(Ordering::Relaxed) }
    }

    /// Event output bus 0 when the last configuration found one, with the count declared.
    pub(super) fn note_output(&self) -> Option<NoteOutputInfo> {
        let declared = self.event_outputs.load(Ordering::Relaxed);
        (declared > 0).then_some(NoteOutputInfo { read: 0, declared })
    }
}

/// What the stage reset did before a call: whether it ran, and whether it succeeded.
#[derive(Clone, Copy)]
pub(super) struct Reset {
    ran: bool,
    ok: bool,
}

impl Vst3Plugin {
    /// Before an actual call's `process`, under the core: its output list limited to the block
    /// buffer's room (decision 3), then the pending stage reset's `setProcessing` cycle (decision 6).
    pub(super) fn stage_reset(&mut self, core: &mut Core) -> Reset {
        core.inst.limit_output(self.note_out.room());
        if !std::mem::take(&mut self.cycle_pending) {
            return Reset { ran: false, ok: true };
        }
        Reset { ran: true, ok: core.inst.cycle_processing() }
    }

    /// After an actual call, under the core: its output into the block's buffer and its record
    /// (the generation it ran under, the reset before it, an error). A failed reset is the call's
    /// processing error: returned as an error, so the call is counted and contained as one.
    pub(super) fn note_call(&mut self, core: &Core, r: Result<u32, i32>, reset: Reset, start: u32, frames: usize) -> Result<u32, i32> {
        if core.inst.collecting() {
            core.inst.drain_output(&mut self.note_out, start, u32::try_from(frames).unwrap_or(u32::MAX));
        } else {
            core.inst.forget_output();
        }
        let r = if reset.ok { r } else { r.and(Err(vst3::Steinberg::kResultFalse)) };
        let generation = self.shared.generation.load(Ordering::Acquire);
        self.note_out.record_call(CallRecord { generation, reset_before: reset.ran, error: r.is_err() });
        r
    }
}
