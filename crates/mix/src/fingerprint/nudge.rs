//! Test seam (feature `testing`; ADR 0029, amendment for #742, section 14's targeted omission
//! controls): changes exactly one inventoried field of a strip, so the mix-state gate can drive
//! two strips into states that differ only in that field and require their prints to differ.
//! Removing the field from the record must fail that control.

use super::super::*;

/// The field a control changes (section 14's list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nudge {
    /// The instrument's MIDI effect host: its edit `epoch` (it decides whether a release is
    /// swallowed).
    MidiEpoch,
    /// The host's `retry`: one more refused release held for the next block.
    MidiRetry,
    /// The first slot's first parameter value last pushed (`Applied`): another finite value.
    AppliedValue,
    /// The same entry's sentinel: "never pushed" (`NaN`) instead of the value, or the reverse.
    AppliedSentinel,
    /// The first reading insert's `NoteDelay` stand-in.
    NoteDelayStandIn,
    /// The host's note view: `full` (it refuses later starts).
    NoteViewFull,
    /// The instrument's pending rack exit: a port its rack has sent notes from.
    RackExit,
    /// A touch ramp mid-flight in the strip's first touch slot.
    TouchRamp,
    /// The strip's retained `latency`.
    Latency,
    /// A key keyed at the strip with early-key frames pending (`pending > 0`).
    KeyPending,
    /// The first clip-effect chain's EQ module toggle (`eq_on`).
    ClipFxToggle,
    /// The first open note's unmatched-on count (`open`: what a coalesced same-key off counts down).
    NoteOpen,
    /// The first open note's retained `last_on`, moved past the next block start.
    LastOn,
    /// The first queued note-on's charge: a re-strike's 2 (it carries the old voice's off).
    RestrikeCost,
    /// The strip's first hardware insert's `live` flag (it decides whether its send is summed).
    HwLive,
}

impl MixEngine {
    /// Changes `what` of `track`'s strip (see [`Nudge`]); `false` when the strip has no such field
    /// to change (the control then proves nothing). Between blocks, with the strip home.
    pub fn nudge(&mut self, track: TrackId, what: Nudge) -> bool {
        let start = self.events.next_frame;
        let Some(p) = self.plan.as_deref_mut() else { return false };
        let Some(i) = p.tables.strip_of(track) else { return false };
        let tables = &mut p.tables;
        let Some(st) = tables.strips.get_mut(i) else { return false };
        let first_slot = st.plugins.iter_mut().flatten().chain(st.instrument.iter_mut()).find(|s| s.applied.len > 0);
        match what {
            Nudge::AppliedValue | Nudge::AppliedSentinel => {
                let Some(s) = first_slot else { return false };
                let n = s.applied.len;
                let Some(v) = s.applied.values(n).first_mut() else { return false };
                *v = match (what, v.is_nan()) {
                    (Nudge::AppliedValue, false) => *v + 0.25,
                    (Nudge::AppliedValue, true) => return false,
                    (_, true) => 0.5,
                    (_, false) => f32::NAN,
                };
                true
            }
            Nudge::NoteDelayStandIn => st.plugins.iter_mut().flatten().find_map(|s| s.note_delay.as_mut()).is_some_and(|d| d.nudge_stand_in()),
            Nudge::TouchRamp => {
                st.touch.nudge_ramp();
                true
            }
            Nudge::Latency => {
                st.latency += 1;
                true
            }
            Nudge::KeyPending => tables.keys.nudge_pending(i),
            Nudge::HwLive => st.hw.first_mut().map(hardware::HwSlot::nudge_live).is_some(),
            Nudge::ClipFxToggle => st.clip_fx.0.first_mut().is_some_and(|(_, fx)| {
                fx.eq_on = !fx.eq_on;
                true
            }),
            Nudge::MidiEpoch | Nudge::MidiRetry | Nudge::NoteViewFull | Nudge::RackExit | Nudge::NoteOpen | Nudge::LastOn | Nudge::RestrikeCost => {
                st.instrument.as_mut().and_then(|s| s.events.as_deref_mut()).is_some_and(|e| e.nudge(what, start))
            }
        }
    }
}
