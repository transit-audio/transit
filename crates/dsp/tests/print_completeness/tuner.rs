//! The Tuner's witness set (ADR 0029, amendment for #742, section 13; slice 6).
//!
//! The Tuner holds its two parameter values and nothing else (the inventory in `tuner_record.rs`);
//! `process` leaves the buffers untouched, so the outputs compare equal under any state, and the
//! witnesses rest on the record comparison and the parameter readback (the harness compares every
//! parameter through `Plugin::param` after the restore and after every block: the mix reads it as
//! an automation lane's base value). No span is declared; the continuations run a few blocks.

use super::*;
use transit_dsp::plugins::tuner::Tuner;

type W = Witness<Tuner>;

fn reference(p: &Tuner) -> f32 {
    p.param("reference_hz").unwrap_or(f32::NAN)
}

fn hold(p: &Tuner) -> bool {
    p.param("hold").is_some_and(|v| v >= 0.5)
}

fn witnesses() -> Vec<W> {
    vec![
        // 1: the first blocks with parameter changes.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |_| vec![Step::Param("reference_hz", 432.0), Step::Run(300), Step::Param("hold", 1.0), Step::Run(300)],
            reached: |p| reference(p) == 440.0 && !hold(p),
            completed: |p| reference(p) == 432.0 && hold(p),
        },
        // 2: after a reset (which keeps the parameters); the continuation changes them again.
        W {
            boundary: Boundary::AfterReset,
            history: |_| vec![Step::Param("reference_hz", 450.0), Step::Param("hold", 1.0), Step::Run(300), Step::Reset],
            continuation: |_| vec![Step::Param("hold", 0.0), Step::Run(300), Step::Param("reference_hz", 420.0), Step::Run(300)],
            reached: |p| reference(p) == 450.0 && hold(p),
            completed: |p| reference(p) == 420.0 && !hold(p),
        },
        // Parameters set at T and none changed after (the values reach later calls only through
        // the record).
        W {
            boundary: Boundary::Entry("parameters held"),
            history: |_| vec![Step::Param("reference_hz", 443.0), Step::Param("hold", 1.0), Step::Run(500)],
            continuation: |_| vec![Step::Run(500)],
            reached: |p| reference(p) == 443.0 && hold(p),
            completed: |p| reference(p) == 443.0 && hold(p),
        },
        // 10: prepared at another rate with parameters changed there, then prepared at the case's
        // rate (T); the continuation changes them again.
        W {
            boundary: Boundary::SecondPrepare,
            history: |rate| {
                let o = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                vec![Step::Prepare(o), Step::Param("reference_hz", 460.0), Step::Run(300), Step::Prepare(rate)]
            },
            continuation: |_| vec![Step::Param("hold", 1.0), Step::Run(300)],
            reached: |p| reference(p) == 460.0,
            completed: |p| hold(p),
        },
    ]
}

pub(super) fn entry() -> Entry<Tuner> {
    Entry {
        id: "tuner",
        make: || Box::new(Tuner::new()),
        describe: |p| format!("reference {} Hz, hold {}", reference(p), hold(p)),
        witnesses: witnesses(),
        inapplicable: vec![
            (Boundary::VoicesStolen, "no voices: `Tuner` takes no notes (`note_on`, `event` are the trait defaults)"),
            (Boundary::ReleaseTail, "no notes, so no release tails"),
            (Boundary::MidRamp, "no ramps: `Tuner` has no smoother (`set_param` stores the value, `update` is empty)"),
            (Boundary::ModeSwitch, "no modes reach the instance's processing: `Tuner::process` does nothing; hold is read by the panel only"),
            (Boundary::Flush, "no feedback and no flush threshold: `Tuner::process` does nothing"),
            (Boundary::InternalBlock, "no internal block: `Tuner::process` does nothing"),
            (Boundary::Pending, "nothing pending: `set_param` stores the value at once"),
            (Boundary::Sustain, "no keys or pedals: `Tuner` takes no notes"),
        ],
        poison: None,
    }
}
