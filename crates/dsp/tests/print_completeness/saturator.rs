//! The Saturator's witness set (ADR 0029, amendment for #742, section 13; slice 6).
//!
//! It takes no notes (the inventory in `harmonic_record.rs`). The spans the inventory declares, fed
//! to the one duration calculator:
//! - `SETTLE`: the four smoothers (20 ms each) settle within 16 time constants;
//! - `DECAY`: silence long enough for the DC blockers' output memories (a 10 Hz pole: 57.6 time
//!   constants of 16 ms reach the flush threshold in under a second at any rate) and the tone
//!   low-passes (flushed far sooner) to reach zero.

use super::*;
use transit_dsp::plugins::harmonic::Saturator;

const SETTLE: Span = Span::Seconds(16.0 * 0.020);
const DECAY: Span = Span::Seconds(3.0);

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

type W = Witness<Saturator>;

/// Every path in use: drive, asymmetry (the DC blocker runs), the tone low-pass closed, a mostly
/// wet mix, an output gain.
fn setup() -> Vec<Step> {
    vec![Step::Param("drive", 18.0), Step::Param("bias", 40.0), Step::Param("tone", 4_000.0), Step::Param("mix", 80.0), Step::Param("output", -3.0)]
}

/// Ramps of all four smoothers at once.
fn ramp() -> Vec<Step> {
    vec![Step::Param("drive", 30.0), Step::Param("bias", 70.0), Step::Param("mix", 40.0), Step::Param("output", 4.0)]
}

fn ramp_again() -> Vec<Step> {
    vec![Step::Param("drive", 3.0), Step::Param("bias", 10.0), Step::Param("mix", 100.0), Step::Param("output", -9.0), Step::Param("tone", 2_000.0)]
}

fn witnesses() -> Vec<W> {
    vec![
        // 1: the first blocks with parameter changes, through the ramps.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [setup(), vec![Step::Run(97)], ramp(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
            reached: |p| !p.observe().dc_memory && !p.observe().lp_memory && !p.observe().settling,
            completed: |p| p.observe().dc_memory && p.observe().lp_memory && !p.observe().settling,
        },
        // 2: a reset in the middle of a ramp (it snaps and clears the memories); the continuation
        // sets and ramps again.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| [setup(), vec![Step::Run(span(&[SETTLE], rate))], ramp(), vec![Step::Run(97), Step::Reset]].concat(),
            continuation: |rate| [ramp_again(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
            reached: |p| !p.observe().dc_memory && !p.observe().lp_memory && !p.observe().settling,
            completed: |p| p.observe().dc_memory && p.observe().lp_memory && !p.observe().settling,
        },
        // 5: mid-ramp (all four smoothers); the continuation changes every target again mid-ramp,
        // then runs until each ramp completes, plus one block.
        W {
            boundary: Boundary::MidRamp,
            history: |rate| [setup(), vec![Step::Run(span(&[SETTLE], rate))], ramp(), vec![Step::RunBlocks(150, 11)]].concat(),
            continuation: |rate| [vec![Step::RunBlocks(17, 7)], ramp_again(), vec![Step::Run(frames(&[SETTLE], rate))]].concat(),
            reached: |p| p.observe().settling,
            completed: |p| !p.observe().settling,
        },
        // 6: both optional stages switched out at T with their memories non-zero (dormant): the
        // asymmetry at 0 (the DC blocker skipped once the smoother reaches 0) and the tone fully
        // open (the low-pass skipped). The continuation brings each back, so each reads its
        // dormant memory, then switches the tone out and in again.
        W {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                [
                    setup(),
                    vec![Step::Run(span(&[SETTLE], rate)), Step::Param("bias", 0.0), Step::Param("tone", 20_000.0), Step::Run(span(&[SETTLE], rate))],
                ]
                .concat()
            },
            continuation: |rate| {
                vec![
                    Step::Param("bias", 55.0),
                    Step::Run(401),
                    Step::Param("tone", 6_000.0),
                    Step::Run(401),
                    Step::Param("tone", 20_000.0),
                    Step::Run(203),
                    Step::Param("tone", 1_500.0),
                    Step::Run(span(&[SETTLE], rate)),
                ]
            },
            reached: |p| p.observe().dc_memory && p.observe().lp_memory && !p.observe().tone_on && !p.observe().settling,
            completed: |p| p.observe().dc_memory && p.observe().tone_on && !p.observe().settling,
        },
        // 7: the flush boundary: a burst, then silence until the DC blockers' and low-passes'
        // memories are flushed to zero; the continuation runs on in silence, then a small input.
        W {
            boundary: Boundary::Flush,
            history: |rate| [setup(), vec![Step::Run(span(&[SETTLE], rate)), Step::Silent(span(&[DECAY], rate))]].concat(),
            continuation: |rate| vec![Step::Silent(997), Step::Small(span(&[SETTLE], rate))],
            reached: |p| !p.observe().dc_memory && !p.observe().lp_memory,
            completed: |p| p.observe().dc_memory && p.observe().lp_memory,
        },
        // The tone closed at T and no parameter changed after it: the coefficient the restore
        // re-derives (`tone`, derived) is the only way the continuation's low-pass runs, as no
        // `update` follows.
        W {
            boundary: Boundary::Entry("tone closed"),
            history: |rate| vec![Step::Param("tone", 2_500.0), Step::Param("drive", 12.0), Step::Run(span(&[SETTLE], rate))],
            continuation: |rate| vec![Step::Run(span(&[SETTLE], rate))],
            reached: |p| p.observe().tone_on && p.observe().lp_memory,
            completed: |p| p.observe().tone_on,
        },
        // The parameter values read again: settings made and settled at T; the continuation
        // prepares again at the case's rate (`prepare` calls `update`, which reads every value).
        W {
            boundary: Boundary::Entry("prepare reads the parameters"),
            history: |rate| [setup(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
            continuation: |rate| vec![Step::Prepare(rate), Step::Run(span(&[SETTLE], rate))],
            reached: |p| p.observe().dc_memory && !p.observe().settling,
            completed: |p| p.observe().dc_memory && !p.observe().settling,
        },
        // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
        // case's rate (T); the continuation meets boundary 1's obligation again.
        W {
            boundary: Boundary::SecondPrepare,
            history: |rate| {
                let o = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                [vec![Step::Prepare(o)], setup(), vec![Step::Run(97)], ramp(), vec![Step::Run(span(&[SETTLE], o)), Step::Prepare(rate)]].concat()
            },
            continuation: |rate| [ramp_again(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
            reached: |p| !p.observe().dc_memory && !p.observe().lp_memory && !p.observe().settling,
            completed: |p| p.observe().dc_memory && p.observe().lp_memory && !p.observe().settling,
        },
    ]
}

pub(super) fn entry() -> Entry<Saturator> {
    Entry {
        id: "saturator",
        make: || Box::new(Saturator::new()),
        describe: |p| format!("{:?}", p.observe()),
        witnesses: witnesses(),
        inapplicable: vec![
            (Boundary::VoicesStolen, "no voices: `Saturator` takes no notes (`note_on`, `event` are the trait defaults)"),
            (Boundary::ReleaseTail, "no notes, so no release tails"),
            (Boundary::InternalBlock, "no internal block: `Saturator::process` works sample by sample over the host block"),
            (Boundary::Pending, "nothing pending: `set_param` sets the targets and the tone at once (`update`; the ramps are boundary 5's)"),
            (Boundary::Sustain, "no keys or pedals: `Saturator` takes no notes"),
        ],
        poison: None,
    }
}
