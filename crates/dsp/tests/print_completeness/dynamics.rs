//! The Compressor/Limiter's and the Expander/Gate's witness sets (ADR 0029, amendment for #742,
//! section 13; slice 2).
//!
//! Neither takes notes. The spans the inventories declare (`dynamics_record.rs`), fed to the one
//! duration calculator:
//! - `SMOOTH`: the Compressor's makeup and mix smoothers settle within 16 time constants of 20 ms
//!   (e^-16 times the largest step a witness makes is below the snap tolerance);
//! - `RING`: a lookahead ring, at most twice 10 ms plus a few samples (power-of-two rounding);
//! - `DETECT`: the detectors' release to rest, 14 time constants of the longest release the
//!   witnesses set (150 ms);
//! - `HOLD`: 50 ms, longer than the gate's hold at the witnesses' 30 ms;
//! - `DECAY`: silence long enough for every memory to reach its flush or underflow endpoint.

use super::*;
use transit_dsp::plugins::dynamics::{Compressor, ExpanderGate};

const SMOOTH: Span = Span::Seconds(16.0 * 0.020);
const RING: Span = Span::Seconds(0.025);
const DETECT: Span = Span::Seconds(14.0 * 0.150);
const HOLD: Span = Span::Seconds(0.050);
const DECAY: Span = Span::Seconds(2.0);

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

// ---- Compressor ----

type CW = Witness<Compressor>;

/// The common setup: compressing hard enough that the detector moves, makeup and a mostly wet mix.
fn comp_setup() -> Vec<Step> {
    vec![
        Step::Param("threshold", -24.0),
        Step::Param("ratio", 6.0),
        Step::Param("attack", 2.0),
        Step::Param("release", 80.0),
        Step::Param("knee", 6.0),
        Step::Param("makeup", 6.0),
        Step::Param("mix", 80.0),
    ]
}

/// Boundary 1's obligation: the first blocks with parameter changes (the sidechain high-pass on, a
/// lookahead), then the detector through attack and release.
fn comp_first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::Param("sc_hpf_on", 1.0),
        Step::Param("lookahead", 3.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("threshold", -12.0),
        Step::Run(span(&[RING, SMOOTH], rate)),
    ]
}

fn comp_witnesses() -> Vec<CW> {
    vec![
        // 1: after prepare, before any process.
        CW {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [comp_setup(), comp_first_blocks(rate)].concat(),
            reached: |p| p.observe().gr == 0.0 && p.observe().line_peak == 0.0,
            completed: |p| p.observe().line_peak > 0.0 && p.observe().sidechain_memory,
        },
        // 2: a reset in the middle of compressing and of a makeup ramp.
        CW {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = comp_setup();
                h.extend([Step::Param("lookahead", 5.0), Step::Run(span(&[HOLD], rate)), Step::Param("makeup", 12.0), Step::Run(97), Step::Reset]);
                h
            },
            continuation: comp_first_blocks,
            reached: |p| p.observe().gr == 0.0 && !p.observe().settling,
            completed: |p| p.observe().line_peak > 0.0,
        },
        // 5: mid-ramp (makeup and mix); the continuation changes both targets again mid-ramp, then
        // runs until both complete, plus one block.
        CW {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = comp_setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Param("makeup", 20.0), Step::Param("mix", 30.0), Step::RunBlocks(150, 11)]);
                h
            },
            continuation: |rate| {
                vec![Step::RunBlocks(17, 7), Step::Param("makeup", 2.0), Step::Param("mix", 100.0), Step::Run(frames(&[SMOOTH], rate))]
            },
            reached: |p| p.observe().settling,
            completed: |p| !p.observe().settling,
        },
        // 6: the sidechain high-pass switched off with its memories non-zero (dormant), the limiter
        // on; the continuation turns the high-pass back on (it reads the dormant memories), turns
        // the limiter off, changes the lookahead and the high-pass frequency.
        CW {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                let mut h = comp_setup();
                h.extend([
                    Step::Param("sc_hpf_on", 1.0),
                    Step::Param("sc_hpf_freq", 300.0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("sc_hpf_on", 0.0),
                    Step::Param("limiter", 1.0),
                    Step::Run(span(&[HOLD], rate)),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Param("sc_hpf_on", 1.0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("limiter", 0.0),
                    Step::Param("lookahead", 8.0),
                    Step::Run(span(&[RING], rate)),
                    Step::Param("sc_hpf_freq", 40.0),
                    Step::Param("lookahead", 1.0),
                    Step::Run(span(&[RING, DETECT], rate)),
                ]
            },
            reached: |p| p.observe().sidechain_memory,
            completed: |p| p.observe().sidechain_memory,
        },
        // 7: the flush boundary: a burst, then silence through the detector's release and the
        // high-pass memories' flush; the continuation runs on in silence, then a small input.
        CW {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = comp_setup();
                h.extend([
                    Step::Param("sc_hpf_on", 1.0),
                    Step::Param("lookahead", 4.0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Silent(span(&[DECAY], rate)),
                ]);
                h
            },
            continuation: |rate| vec![Step::Silent(997), Step::Small(span(&[DETECT], rate))],
            reached: |p| p.observe().line_peak == 0.0 && !p.observe().sidechain_memory,
            completed: |p| p.observe().line_peak > 0.0,
        },
        // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
        // case's rate (T); the continuation meets boundary 1's obligation again.
        CW {
            boundary: Boundary::SecondPrepare,
            history: |rate| {
                let other = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                let mut h = vec![Step::Prepare(other)];
                h.extend(comp_setup());
                h.extend(comp_first_blocks(other));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: comp_first_blocks,
            reached: |p| p.observe().line_peak == 0.0,
            completed: |p| p.observe().line_peak > 0.0,
        },
        // The lookahead lines: filled at the longest lookahead; the continuation reads every
        // position of each ring (its span), the input silent so the line alone sounds.
        CW {
            boundary: Boundary::Entry("lookahead line"),
            history: |rate| {
                let mut h = comp_setup();
                h.extend([Step::Param("lookahead", 10.0), Step::Run(span(&[RING, HOLD], rate) + 13)]);
                h
            },
            continuation: |rate| vec![Step::Silent(span(&[RING], rate)), Step::Run(span(&[RING], rate))],
            reached: |p| p.observe().line_peak > 0.0 && p.observe().lookahead > 0,
            completed: |p| p.observe().line_peak > 0.0,
        },
        // The detector envelope inside its release at T (the input stopped); the continuation runs
        // the release out, then compresses again.
        CW {
            boundary: Boundary::Entry("detector release"),
            history: |rate| {
                let mut h = comp_setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Silent(301)]);
                h
            },
            continuation: |rate| vec![Step::Silent(span(&[DETECT], rate)), Step::Run(span(&[HOLD], rate))],
            reached: |p| p.observe().gr > 0.0,
            completed: |p| p.observe().gr > 0.0,
        },
    ]
}

pub(super) fn compressor() -> Entry<Compressor> {
    Entry {
        id: "compressor",
        make: || Box::new(Compressor::new()),
        describe: |p| format!("{:?}", p.observe()),
        witnesses: comp_witnesses(),
        inapplicable: vec![
            (Boundary::VoicesStolen, "no voices: `Compressor` takes no notes (`note_on`, `event` are the trait defaults)"),
            (Boundary::ReleaseTail, "no notes, so no release tails: the detector's release is the detector witness's"),
            (Boundary::InternalBlock, "no internal block: `Compressor::run` works sample by sample over the host block"),
            (Boundary::Pending, "nothing pending: `set_param` applies at once (`update`; the smoothers' ramps are boundary 5's)"),
            (Boundary::Sustain, "no keys or pedals: `Compressor` takes no notes"),
        ],
        // The meter readings.
        poison: Some(|p, k| p.poison_unobservable(k)),
    }
}

// ---- Expander/Gate ----

type GW = Witness<ExpanderGate>;

/// The common setup: a threshold the input crosses often (its peaks reach -6 dB and its quiet
/// stretches fall far below), a short hold, a range short of silence (every time off its default).
fn gate_setup() -> Vec<Step> {
    vec![
        Step::Param("threshold", -12.0),
        Step::Param("ratio", 4.0),
        Step::Param("range", 40.0),
        Step::Param("attack", 1.0),
        Step::Param("hold", 30.0),
        Step::Param("release", 150.0),
    ]
}

fn gate_first_blocks(rate: f32) -> Vec<Step> {
    vec![Step::Run(span(&[HOLD], rate)), Step::Param("threshold", -30.0), Step::Silent(span(&[HOLD, HOLD], rate)), Step::Run(span(&[HOLD], rate))]
}

fn gate_witnesses() -> Vec<GW> {
    vec![
        // 1: after prepare, before any process.
        GW {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [gate_setup(), gate_first_blocks(rate)].concat(),
            reached: |p| p.observe().env == 0.0 && p.observe().gr == 0.0,
            completed: |p| p.observe().env > 0.0,
        },
        // 2: a reset in the middle of a hold.
        GW {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = gate_setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Silent(301), Step::Reset]);
                h
            },
            continuation: gate_first_blocks,
            reached: |p| p.observe().env == 0.0 && p.observe().hold_left == 0,
            completed: |p| p.observe().env > 0.0,
        },
        // 5: the gain reduction mid-ramp (closing after the hold); the continuation changes the
        // release and the range mid-ramp, then runs the ramp out, then opens.
        GW {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = gate_setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Silent(span(&[HOLD], rate) + 600)]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Silent(17),
                    Step::Param("release", 30.0),
                    Step::Param("range", 60.0),
                    Step::Silent(span(&[DETECT], rate)),
                    Step::Run(span(&[HOLD], rate)),
                ]
            },
            reached: |p| p.observe().gr > 0.0 && p.observe().gr < 40.0,
            completed: |p| p.observe().env > 0.0,
        },
        // 7: the tiny-feedback boundary: a burst, then silence until the detector envelope has
        // decayed into the subnormal range, where it stays (`env * env_rel` rounds back to the same
        // subnormal: retained subnormal state, encoded like any other value); the continuation runs
        // on in silence, then a small input.
        GW {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = gate_setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Silent(span(&[DECAY], rate))]);
                h
            },
            continuation: |rate| vec![Step::Silent(997), Step::Small(span(&[HOLD, HOLD], rate))],
            reached: |p| p.observe().env < f32::MIN_POSITIVE,
            completed: |p| p.observe().env > f32::MIN_POSITIVE,
        },
        // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
        // case's rate (T); the continuation meets boundary 1's obligation again.
        GW {
            boundary: Boundary::SecondPrepare,
            history: |rate| {
                let other = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                let mut h = vec![Step::Prepare(other)];
                h.extend(gate_setup());
                h.extend(gate_first_blocks(other));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: gate_first_blocks,
            reached: |p| p.observe().env == 0.0,
            completed: |p| p.observe().env > 0.0,
        },
        // The hold countdown inside its span at T (the input dropped below the threshold); the
        // continuation runs the hold out with odd block lengths, then the release, then a burst
        // that refills it.
        GW {
            boundary: Boundary::Entry("hold"),
            history: |rate| {
                let mut h = gate_setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Silent(400)]);
                h
            },
            continuation: |rate| vec![Step::Silent(span(&[HOLD], rate)), Step::Silent(span(&[DETECT], rate)), Step::Run(span(&[HOLD], rate))],
            reached: |p| p.observe().hold_left > 0 && p.observe().hold_left < p.observe().hold,
            completed: |p| p.observe().env > 0.0,
        },
        // The detector envelope above the threshold at T (the input loud); the continuation goes
        // silent, so the envelope's decay decides when the hold starts, then the gate closes
        // partway and the input comes back.
        GW {
            boundary: Boundary::Entry("detector envelope"),
            history: |rate| {
                let mut h = gate_setup();
                h.push(Step::Run(span(&[HOLD], rate)));
                h
            },
            continuation: |rate| vec![Step::Silent(span(&[HOLD], rate) + 300), Step::Run(span(&[HOLD], rate))],
            reached: |p| p.observe().env > 0.25,
            completed: |p| p.observe().env > 0.0,
        },
    ]
}

pub(super) fn expander_gate() -> Entry<ExpanderGate> {
    Entry {
        id: "expander_gate",
        make: || Box::new(ExpanderGate::new()),
        describe: |p| format!("{:?}", p.observe()),
        witnesses: gate_witnesses(),
        inapplicable: vec![
            (Boundary::VoicesStolen, "no voices: `ExpanderGate` takes no notes (`note_on`, `event` are the trait defaults)"),
            (Boundary::ReleaseTail, "no notes, so no release tails: the gate's own hold and release are the hold and ramp witnesses'"),
            (Boundary::ModeSwitch, "no modes: `ExpanderGate::tick` is one path (ratio and range are parameters)"),
            (Boundary::InternalBlock, "no internal block: `ExpanderGate::run` works sample by sample over the host block"),
            (Boundary::Pending, "nothing pending: `set_param` applies at once (`update`)"),
            (Boundary::Sustain, "no keys or pedals: `ExpanderGate` takes no notes"),
        ],
        // The meter reading.
        poison: Some(|p, k| p.poison_unobservable(k)),
    }
}
