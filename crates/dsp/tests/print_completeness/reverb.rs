//! Room Reverb's and Plate Reverb's witness sets (ADR 0029, amendment for #742, section 13;
//! slice 2): one type, two registry entries, each entry its own set of the same witnesses.
//!
//! Insert effects with no notes and no modes. Their entry-specific state is the delay lines (two
//! pre-delay lines, eight diffusers, eight feedback lines) and the modulation LFO. The spans the
//! inventory declares (`reverb_record.rs`), fed to the one duration calculator:
//! - `PRE_RING`: a pre-delay ring, 250 ms plus 8 samples rounded up to a power of two (at most
//!   0.372 s at the witnessed rates);
//! - `LINE_RING`: the longest feedback ring, Room's 56.93 ms at the largest size (1.6) plus the
//!   modulation depth and 8 samples, rounded up to a power of two (at most 0.171 s);
//! - `DIFF_RING`: the longest diffuser, 12.73 ms times 1.071;
//! - `SIZE_SETTLE`: the size smoother's settling, 16 time constants of 200 ms (the pre-delay's
//!   100 ms and the mix's and width's 30 ms settle sooner);
//! - `LFO`: one cycle of the slower modulation LFO (Room's 0.7 Hz);
//! - `DECAY`: silence long enough for every memory to reach its flush endpoint at the witnesses'
//!   shortest decay.

use super::*;
use transit_dsp::plugins::reverb::{Observed, Reverb};

const PRE_RING: Span = Span::Seconds(0.38);
const LINE_RING: Span = Span::Seconds(0.18);
const DIFF_RING: Span = Span::Seconds(0.014);
const SIZE_SETTLE: Span = Span::Seconds(16.0 * 0.200);
const LFO: Span = Span::Seconds(1.0 / 0.7);
const DECAY: Span = Span::Seconds(4.0);
const HOLD: Span = Span::Seconds(0.01);

type W = Witness<Reverb>;

fn obs(p: &Reverb) -> Observed {
    p.observe()
}

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

fn settled(p: &Reverb) -> bool {
    obs(p).settling.iter().all(|s| !s)
}

/// The witnesses' common setup: every parameter off its default.
fn setup() -> Vec<Step> {
    vec![
        Step::Param("mix", 60.0),
        Step::Param("pre_delay", 30.0),
        Step::Param("decay", 1.5),
        Step::Param("size", 70.0),
        Step::Param("damping", 4500.0),
        Step::Param("diffusion", 80.0),
        Step::Param("width", 80.0),
        Step::Param("low_cut", 80.0),
    ]
}

/// Boundary 1's obligation: the first blocks with parameter changes, then the lines' span.
fn first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::Run(span(&[HOLD], rate)),
        Step::Param("size", 40.0),
        Step::Param("pre_delay", 60.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("width", 30.0),
        Step::Run(span(&[LINE_RING, DIFF_RING], rate)),
    ]
}

fn witnesses() -> Vec<W> {
    vec![
        // 1: after prepare, before any process.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [setup(), first_blocks(rate)].concat(),
            reached: |p| obs(p).lines_peak == 0.0 && obs(p).lfo == 0.0,
            completed: |p| obs(p).lines_peak > 0.0,
        },
        // 2: a reset in the middle of a decay and of a size ramp.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Run(span(&[LINE_RING], rate)), Step::Param("size", 10.0), Step::Run(97), Step::Reset]);
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).lines_peak == 0.0 && settled(p),
            completed: |p| obs(p).lines_peak > 0.0,
        },
        // 5: mid-ramp (all four smoothers moving); the continuation changes every target again
        // mid-ramp with odd block lengths, then runs until every ramp completes, plus one block.
        W {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Run(span(&[LINE_RING], rate)),
                    Step::Param("size", 15.0),
                    Step::Param("pre_delay", 150.0),
                    Step::Param("mix", 20.0),
                    Step::Param("width", 10.0),
                    Step::RunBlocks(150, 13),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::RunBlocks(17, 7),
                    Step::Param("size", 95.0),
                    Step::Param("pre_delay", 5.0),
                    Step::Param("mix", 90.0),
                    Step::Param("width", 100.0),
                    Step::RunBlocks(33, 11),
                    Step::Run(frames(&[SIZE_SETTLE], rate)),
                ]
            },
            reached: |p| obs(p).settling.iter().all(|s| *s),
            completed: settled,
        },
        // 7: the flush boundary: input at a short decay, then silence until every line, diffuser,
        // damping and low-cut memory is flushed to zero; the continuation runs on in silence, then
        // a small input.
        W {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("decay", 0.2), Step::Run(span(&[LINE_RING], rate)), Step::Silent(span(&[DECAY], rate))]);
                h
            },
            continuation: |rate| vec![Step::Silent(997), Step::Small(span(&[PRE_RING, LINE_RING], rate))],
            reached: |p| obs(p).lines_peak == 0.0 && obs(p).pre_peak == 0.0,
            completed: |p| obs(p).lines_peak > 0.0,
        },
        // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
        // case's rate (T); the continuation meets boundary 1's obligation again.
        W {
            boundary: Boundary::SecondPrepare,
            history: |rate| {
                let other = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                let mut h = vec![Step::Prepare(other)];
                h.extend(setup());
                h.extend(first_blocks(other));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).lines_peak == 0.0,
            completed: |p| obs(p).lines_peak > 0.0,
        },
        // The delay lines: the pre-delay at its longest, the size at its largest and the decay at
        // its longest, every ring filled; the continuation reads every position of each ring (the
        // pre-delay ring, then the feedback and diffuser rings behind it), the input silent so
        // only the memories sound, then input again.
        W {
            boundary: Boundary::Entry("delay lines"),
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("pre_delay", 250.0),
                    Step::Param("size", 100.0),
                    Step::Param("decay", 20.0),
                    Step::Param("diffusion", 100.0),
                    Step::Run(span(&[PRE_RING, LINE_RING, DIFF_RING, SIZE_SETTLE], rate)),
                ]);
                h
            },
            continuation: |rate| vec![Step::Silent(span(&[PRE_RING, LINE_RING, DIFF_RING], rate)), Step::Run(span(&[HOLD], rate))],
            reached: |p| obs(p).lines_peak > 0.0 && obs(p).pre_peak > 0.0 && settled(p),
            completed: |p| obs(p).lines_peak > 0.0,
        },
        // The modulation LFO: its phase off zero at T; the continuation runs a whole cycle.
        W {
            boundary: Boundary::Entry("modulation LFO"),
            history: |rate| {
                let mut h = setup();
                h.push(Step::Run(span(&[LFO], rate) / 3));
                h
            },
            continuation: |rate| vec![Step::Run(span(&[LFO], rate))],
            reached: |p| obs(p).lfo > 0.0,
            completed: |p| obs(p).lfo > 0.0,
        },
    ]
}

fn inapplicable() -> Vec<(Boundary, &'static str)> {
    vec![
        (Boundary::VoicesStolen, "no voices: `Reverb` takes no notes (`note_on`, `event` are the trait defaults)"),
        (Boundary::ReleaseTail, "no notes, so no release tails: the decay is the flush and delay-line witnesses'"),
        (Boundary::ModeSwitch, "no modes: `Reverb::process` is one path; Room and Plate are two registry entries, each with this witness set"),
        (
            Boundary::InternalBlock,
            "no internal block: `Reverb::process` works sample by sample; the size smoother advances once per host block and the gains follow it, both encoded, and the ramp witness crosses odd block lengths",
        ),
        (Boundary::Pending, "nothing pending: `set_param` applies at once (`update`; the smoothers' ramps are boundary 5's)"),
        (Boundary::Sustain, "no keys or pedals: `Reverb` takes no notes"),
    ]
}

pub(super) fn room() -> Entry<Reverb> {
    Entry {
        id: "room_reverb",
        make: || Box::new(Reverb::room()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: inapplicable(),
        poison: None,
    }
}

pub(super) fn plate() -> Entry<Reverb> {
    Entry {
        id: "plate_reverb",
        make: || Box::new(Reverb::plate()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: inapplicable(),
        poison: None,
    }
}
