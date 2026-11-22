//! Gain's and Trim's witness sets (ADR 0029, amendment for #742, section 13; slice 2).
//!
//! Each holds its parameter values and one gain smoother, nothing else (the inventory in
//! `utility_record.rs`). The span the inventory declares, fed to the one duration calculator: the
//! smoother's settling, at most 16 time constants (it snaps within 1e-6·(1 + |target|) of its
//! target, and e^-16 times the largest step a witness makes is below that), 20 ms for Gain and
//! 10 ms for Trim.

use super::*;
use transit_dsp::plugins::utility::{Gain, Trim};

const GAIN_SETTLE: Span = Span::Seconds(16.0 * 0.020);
const TRIM_SETTLE: Span = Span::Seconds(16.0 * 0.010);

/// The checklist boundaries neither entry has, with their code-backed explanations.
fn inapplicable() -> Vec<(Boundary, &'static str)> {
    vec![
        (Boundary::VoicesStolen, "no voices: neither takes notes (`note_on`, `event` are the trait defaults)"),
        (Boundary::ReleaseTail, "no notes, so no release tails"),
        (Boundary::ModeSwitch, "no modes: `apply_gain` is the one path (its settled shortcut reads the smoother's state, encoded)"),
        (Boundary::Flush, "no feedback and no flush threshold: one smoother, whose snap boundary 5's witness reaches"),
        (Boundary::InternalBlock, "no internal block: `apply_gain` works sample by sample over the host block"),
        (Boundary::Pending, "nothing pending: `set_param` sets the smoother's target at once (its ramp is boundary 5's)"),
        (Boundary::Sustain, "no keys or pedals: neither takes notes"),
    ]
}

macro_rules! witnesses {
    ($ty:ty, $settle:expr, $extra:expr) => {{
        type W = Witness<$ty>;
        vec![
            // 1: the first blocks with parameter changes, through the ramp.
            W {
                boundary: Boundary::AfterPrepare,
                history: |_| vec![],
                continuation: |rate| vec![Step::Param("gain", -9.0), Step::Run(97), Step::Param("gain", 4.0), Step::Run(frames(&[$settle], rate))],
                reached: |p| !p.settling(),
                completed: |p| !p.settling(),
            },
            // 2: a reset in the middle of a ramp (it snaps); the continuation ramps again.
            W {
                boundary: Boundary::AfterReset,
                history: |_| vec![Step::Param("gain", -20.0), Step::Run(300), Step::Reset],
                continuation: |rate| vec![Step::Param("gain", 3.0), Step::Run(frames(&[$settle], rate))],
                reached: |p| !p.settling(),
                completed: |p| !p.settling(),
            },
            // 5: mid-ramp; the continuation changes the target again mid-ramp ($extra: Trim also
            // flips its polarity), then runs until the ramp completes, plus one block.
            W {
                boundary: Boundary::MidRamp,
                history: |_| vec![Step::Param("gain", -30.0), Step::RunBlocks(200, 13)],
                continuation: |rate| {
                    let mut c = vec![Step::RunBlocks(17, 7), Step::Param("gain", 6.0)];
                    c.extend($extra);
                    c.push(Step::Run(frames(&[$settle], rate)));
                    c
                },
                reached: |p| p.settling(),
                completed: |p| !p.settling(),
            },
            // The gain at its floor (the "-inf" step: a gain of 0), then back.
            W {
                boundary: Boundary::Entry("gain off"),
                history: |rate| vec![Step::Param("gain", -96.0), Step::Run(frames(&[$settle], rate))],
                continuation: |rate| vec![Step::Run(64), Step::Param("gain", 0.0), Step::Run(frames(&[$settle], rate))],
                reached: |p| !p.settling(),
                completed: |p| !p.settling(),
            },
            // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
            // case's rate (T); the continuation meets boundary 1's obligation again.
            W {
                boundary: Boundary::SecondPrepare,
                history: |rate| {
                    let o = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                    vec![
                        Step::Prepare(o),
                        Step::Param("gain", -12.0),
                        Step::Run(frames(&[$settle], o)),
                        Step::Param("gain", 2.0),
                        Step::Run(400),
                        Step::Prepare(rate),
                    ]
                },
                continuation: |rate| vec![Step::Param("gain", -5.0), Step::Run(frames(&[$settle], rate))],
                reached: |p| !p.settling(),
                completed: |p| !p.settling(),
            },
        ]
    }};
}

pub(super) fn gain() -> Entry<Gain> {
    Entry {
        id: "gain",
        make: || Box::new(Gain::new()),
        describe: |p| format!("settling {}", p.settling()),
        witnesses: witnesses!(Gain, GAIN_SETTLE, Vec::<Step>::new()),
        inapplicable: inapplicable(),
        poison: None,
    }
}

pub(super) fn trim() -> Entry<Trim> {
    Entry {
        id: "trim",
        make: || Box::new(Trim::new()),
        describe: |p| format!("settling {}, gain {}", p.settling(), p.gain()),
        witnesses: {
            let mut w = witnesses!(Trim, TRIM_SETTLE, [Step::Param("invert", 1.0)]);
            // The polarity switch at T: inverted and settled; the continuation flips it back
            // mid-ramp of a gain change.
            w.push(Witness {
                boundary: Boundary::Entry("polarity"),
                history: |rate| vec![Step::Param("invert", 1.0), Step::Param("gain", -3.0), Step::Run(frames(&[TRIM_SETTLE], rate))],
                continuation: |rate| {
                    vec![Step::Param("gain", 5.0), Step::RunBlocks(40, 9), Step::Param("invert", 0.0), Step::Run(frames(&[TRIM_SETTLE], rate))]
                },
                reached: |p| p.gain() < 0.0 && !p.settling(),
                completed: |p| p.gain() > 0.0 && !p.settling(),
            });
            w
        },
        inapplicable: inapplicable(),
        poison: None,
    }
}
