//! EQ 7-Band's and EQ 1-Band's witness sets (ADR 0029, amendment for #742, section 13; slice 6).
//!
//! One generic type, two entries (the inventory in `eq_record.rs`). Neither takes notes. The spans
//! the inventory declares, fed to the one duration calculator:
//! - `SETTLE`: every smoother (each parameter's and the output gain's, 20 ms each) settles within
//!   16 time constants (e^-16 times the largest step a witness makes is below the snap
//!   tolerance);
//! - `DECAY`: silence long enough for every section's memories to reach the flush threshold at the
//!   witnesses' settings (the slowest pole is the 24 dB/oct high-pass's second section at 60 Hz,
//!   Q 1.31: a time constant of about 7 ms, so 1e-25 is reached in well under half a second).

use super::*;
use transit_dsp::plugins::eq::{Eq1, Eq7};

const SETTLE: Span = Span::Seconds(16.0 * 0.020);
const DECAY: Span = Span::Seconds(2.0);

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

/// The checklist boundaries neither entry has, with their code-backed explanations.
fn inapplicable() -> Vec<(Boundary, &'static str)> {
    vec![
        (Boundary::VoicesStolen, "no voices: neither takes notes (`note_on`, `event` are the trait defaults)"),
        (Boundary::ReleaseTail, "no notes, so no release tails"),
        (
            Boundary::InternalBlock,
            "no internal block kept between calls: `Eq::process`'s 32-sample control chunk starts at its local `pos = 0` on every call",
        ),
        (Boundary::Pending, "nothing pending: `set_param` sets the smoothers' targets at once (`update`; their ramps are boundary 5's)"),
        (Boundary::Sustain, "no keys or pedals: neither takes notes"),
    ]
}

/// EQ 7-Band's continuous settings: every band in use (the pass filters are the mode switch's).
fn eq7_bands() -> Vec<Step> {
    vec![
        Step::Param("input_gain", -2.0),
        Step::Param("low_shelf_gain", 4.0),
        Step::Param("low_mid_gain", -3.0),
        Step::Param("mid_gain", 5.0),
        Step::Param("mid_q", 2.0),
        Step::Param("high_mid_gain", -4.0),
        Step::Param("high_shelf_gain", 3.0),
        Step::Param("output_gain", 1.0),
    ]
}

/// EQ 7-Band's setup: the bands and both pass filters on, at slopes other than the defaults.
fn eq7_setup() -> Vec<Step> {
    let mut s = eq7_bands();
    s.extend([
        Step::Param("hpf_on", 1.0),
        Step::Param("hpf_freq", 60.0),
        Step::Param("hpf_slope", 3.0),
        Step::Param("lpf_on", 1.0),
        Step::Param("lpf_freq", 12_000.0),
        Step::Param("lpf_slope", 2.0),
    ]);
    s
}

/// A ramp of several parameters at once (frequency, gain and Q moves, the output gain).
fn eq7_ramp() -> Vec<Step> {
    vec![Step::Param("mid_freq", 2_500.0), Step::Param("mid_gain", -8.0), Step::Param("low_shelf_q", 1.5), Step::Param("output_gain", -6.0)]
}

fn eq7_ramp_again() -> Vec<Step> {
    vec![Step::Param("mid_freq", 600.0), Step::Param("high_mid_gain", 7.0), Step::Param("output_gain", 3.0)]
}

fn eq1_setup() -> Vec<Step> {
    vec![Step::Param("type", 0.0), Step::Param("freq", 800.0), Step::Param("gain", 6.0), Step::Param("q", 2.0), Step::Param("output_gain", -1.0)]
}

fn eq1_ramp() -> Vec<Step> {
    vec![Step::Param("freq", 3_000.0), Step::Param("gain", -9.0), Step::Param("output_gain", 4.0)]
}

fn eq1_ramp_again() -> Vec<Step> {
    vec![Step::Param("freq", 300.0), Step::Param("q", 0.5), Step::Param("output_gain", -3.0)]
}

macro_rules! witnesses {
    ($ty:ty, $setup:path, $ramp:path, $again:path) => {{
        type W = Witness<$ty>;
        vec![
            // 1: the first blocks with parameter changes, through the ramps.
            W {
                boundary: Boundary::AfterPrepare,
                history: |_| vec![],
                continuation: |rate| [$setup(), vec![Step::Run(97)], $ramp(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
                reached: |p| !p.observe().memory && !p.observe().settling,
                completed: |p| p.observe().memory && !p.observe().settling,
            },
            // 2: a reset in the middle of a ramp (it snaps and clears the memories); the
            // continuation sets and ramps again.
            W {
                boundary: Boundary::AfterReset,
                history: |rate| [$setup(), vec![Step::Run(span(&[SETTLE], rate))], $ramp(), vec![Step::Run(97), Step::Reset]].concat(),
                continuation: |rate| [$again(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
                reached: |p| !p.observe().memory && !p.observe().settling,
                completed: |p| p.observe().memory && !p.observe().settling,
            },
            // 5: mid-ramp; the continuation changes the targets again mid-ramp, then runs until
            // every ramp completes, plus one block.
            W {
                boundary: Boundary::MidRamp,
                history: |rate| [$setup(), vec![Step::Run(span(&[SETTLE], rate))], $ramp(), vec![Step::RunBlocks(150, 11)]].concat(),
                continuation: |rate| [vec![Step::RunBlocks(17, 7)], $again(), vec![Step::Run(frames(&[SETTLE], rate))]].concat(),
                reached: |p| p.observe().settling,
                completed: |p| !p.observe().settling,
            },
            // 7: the flush boundary: a burst, then silence until every memory is flushed to zero;
            // the continuation runs on in silence, then a small input.
            W {
                boundary: Boundary::Flush,
                history: |rate| [$setup(), vec![Step::Run(span(&[SETTLE], rate)), Step::Silent(span(&[DECAY], rate))]].concat(),
                continuation: |rate| vec![Step::Silent(997), Step::Small(span(&[SETTLE], rate))],
                reached: |p| !p.observe().memory && !p.observe().settling,
                completed: |p| p.observe().memory,
            },
            // The parameter values read again: settings made and settled at T; the continuation
            // prepares again at the case's rate (`prepare` sets every target from the parameters,
            // snaps and designs).
            W {
                boundary: Boundary::Entry("prepare reads the parameters"),
                history: |rate| [$setup(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
                continuation: |rate| vec![Step::Prepare(rate), Step::Run(span(&[SETTLE], rate))],
                reached: |p| p.observe().memory && !p.observe().settling,
                completed: |p| p.observe().memory && !p.observe().settling,
            },
            // 10: prepared at another rate, boundary 1's obligation met there, then prepared at
            // the case's rate (T); the continuation meets boundary 1's obligation again.
            W {
                boundary: Boundary::SecondPrepare,
                history: |rate| {
                    let o = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                    [vec![Step::Prepare(o)], $setup(), vec![Step::Run(97)], $ramp(), vec![Step::Run(span(&[SETTLE], o)), Step::Prepare(rate)]]
                        .concat()
                },
                continuation: |rate| [$again(), vec![Step::Run(span(&[SETTLE], rate))]].concat(),
                reached: |p| !p.observe().memory && !p.observe().settling,
                completed: |p| p.observe().memory && !p.observe().settling,
            },
        ]
    }};
}

pub(super) fn eq7() -> Entry<Eq7> {
    let mut witnesses = witnesses!(Eq7, eq7_setup, eq7_ramp, eq7_ramp_again);
    // 6: the low-pass switched off at T (its sections hold the identity, its memories drained),
    // the high-pass on at its default slope, every ramp settled. At T only `hpf_on` differs from
    // a fresh instance among the snapped toggles and choices, so the continuation's first switch
    // (the high-pass off) is designed only because `vals` holds the old value (the derived field,
    // `eq_record.rs`). Then each mode is revisited: the low-pass back on, the high-pass on at
    // 24 dB/oct.
    witnesses.push(Witness {
        boundary: Boundary::ModeSwitch,
        history: |rate| {
            let mut h = eq7_bands();
            h.extend([
                Step::Param("hpf_on", 1.0),
                Step::Param("hpf_freq", 90.0),
                Step::Param("lpf_on", 1.0),
                Step::Param("lpf_slope", 3.0),
                Step::Run(span(&[SETTLE], rate)),
                Step::Param("lpf_on", 0.0),
                Step::Param("lpf_slope", 1.0),
                Step::Run(span(&[SETTLE], rate)),
            ]);
            h
        },
        continuation: |rate| {
            vec![
                Step::Param("hpf_on", 0.0),
                Step::Run(401),
                Step::Param("lpf_on", 1.0),
                Step::Run(401),
                Step::Param("hpf_on", 1.0),
                Step::Param("hpf_slope", 3.0),
                Step::Run(span(&[SETTLE], rate)),
            ]
        },
        reached: |p| p.observe().memory && !p.observe().settling && p.observe().sections == 6,
        completed: |p| p.observe().memory && !p.observe().settling && p.observe().sections == 8,
    });
    Entry {
        id: "eq_7band",
        make: || Box::new(Eq7::new()),
        describe: |p| format!("{:?}", p.observe()),
        witnesses,
        inapplicable: inapplicable(),
        poison: Some(Eq7::poison_unobservable),
    }
}

pub(super) fn eq1() -> Entry<Eq1> {
    let mut witnesses = witnesses!(Eq1, eq1_setup, eq1_ramp, eq1_ramp_again);
    // 6: the type switched (notch, then high-pass) at T, every ramp settled; only `type` differs
    // from a fresh instance among the snapped choices, so the continuation's switch to a peak at
    // 0 dB (the identity) is designed only because `vals` holds the old type (the derived field).
    // Then the peak, the band-pass and the low shelf are visited.
    witnesses.push(Witness {
        boundary: Boundary::ModeSwitch,
        history: |rate| {
            vec![
                Step::Param("freq", 500.0),
                Step::Param("q", 1.5),
                Step::Param("type", 5.0),
                Step::Run(span(&[SETTLE], rate)),
                Step::Param("type", 3.0),
                Step::Run(span(&[SETTLE], rate)),
            ]
        },
        continuation: |rate| {
            vec![
                Step::Param("type", 0.0),
                Step::Run(401),
                Step::Param("gain", 5.0),
                Step::Run(401),
                Step::Param("type", 6.0),
                Step::Run(401),
                Step::Param("type", 1.0),
                Step::Run(span(&[SETTLE], rate)),
            ]
        },
        reached: |p| p.observe().memory && !p.observe().settling && p.observe().sections == 1,
        completed: |p| p.observe().memory && !p.observe().settling && p.observe().sections == 1,
    });
    Entry {
        id: "eq_1band",
        make: || Box::new(Eq1::new()),
        describe: |p| format!("{:?}", p.observe()),
        witnesses,
        inapplicable: inapplicable(),
        poison: Some(Eq1::poison_unobservable),
    }
}
