//! The Time Stretcher's witness set (ADR 0029, amendment for #742, section 13; slice 4).
//!
//! An insert effect with no notes: the voice, release, sustain and pending-event boundaries do not
//! apply (the explanations at [`entry`]). Its entry-specific state is the recording: the firmware
//! records its input into its buffer, and with Freeze on the recording stops and plays for as long
//! as it stays frozen, which no declared tail covers (section 12); its witness freezes a recording,
//! scrubs the whole frozen span, unfreezes (the crossfade from the tail kept while frozen) and
//! replays the whole span. The spans the inventory declares, fed to the one duration calculator
//! (the firmware runs at 32 kHz):
//! - `REC[q]`: the recording at quality `q` (1.02, 1.86, 4.09 and 7.42 s);
//! - `REVERB`: the reverb's memory (16384 samples) and `DIFFUSER` its diffuser's (2048);
//! - `FADE`: the restart fade (10 ms each way) and `HOLD` a few firmware blocks.

use super::*;
use transit_dsp::plugins::time_stretcher::{Observed, TimeStretcher};

const CODEC: f64 = 32_000.0;
const REC: [Span; 4] =
    [Span::Seconds(32_696.0 / CODEC), Span::Seconds(59_384.0 / CODEC), Span::Seconds(65_400.0 / 16_000.0), Span::Seconds(118_776.0 / 16_000.0)];
const REVERB: Span = Span::Seconds(16_384.0 / CODEC);
const DIFFUSER: Span = Span::Seconds(2_048.0 / CODEC);
const FADE: Span = Span::Seconds(0.02);
const HOLD: Span = Span::Seconds(0.01);

type W = Witness<TimeStretcher>;

fn obs(p: &TimeStretcher) -> Observed {
    p.observe()
}

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

/// The witnesses' common setup: the windows reading back into the recording, a transposition, the
/// diffuser, a tone off center, some feedback and reverb, a mostly wet mix.
fn setup() -> Vec<Step> {
    vec![
        Step::Param("pitch", 5.0),
        Step::Param("fine", 13.0),
        Step::Param("window", 55.0),
        Step::Param("position", 40.0),
        Step::Param("diffusion", 30.0),
        Step::Param("tone", 35.0),
        Step::Param("feedback", 20.0),
        Step::Param("reverb", 40.0),
        Step::Param("dry_wet", 80.0),
    ]
}

/// Boundary 1's obligation: the first blocks with parameter changes (a quality change before the
/// first block applies at once), then the recording's span with the read point moving.
fn first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::Param("quality", 1.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("pitch", -7.0),
        Step::Param("input_level", -6.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("position", 75.0),
        Step::Run(span(&[REC[1]], rate)),
    ]
}

/// One visit of quality `q`: the restart, then its recording's span recorded and read.
fn visit(q: usize, rate: f32) -> Vec<Step> {
    vec![Step::Param("quality", q as f32), Step::Run(span(&[FADE, FADE, REC[q]], rate))]
}

fn witnesses() -> Vec<W> {
    let mut v = vec![
        // 1: after prepare, before any process.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [setup(), first_blocks(rate)].concat(),
            reached: |p| obs(p).idle,
            completed: |p| !obs(p).idle && obs(p).quality == 1,
        },
        // 2: after a reset in the middle of a recording and a restart fade.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Run(span(&[HOLD, HOLD], rate)), Step::Param("quality", 2.0), Step::Run(97), Step::Reset]);
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).idle && obs(p).fade == 0,
            completed: |p| !obs(p).idle,
        },
        // 5: mid-ramp: the restart fade going down (a quality change); the continuation turns the
        // quality back before the bottom (the fade turns round, no restart), then changes it again
        // and runs through the restart and the fade-in, plus one block.
        W {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Run(span(&[HOLD, HOLD], rate)), Step::Param("quality", 1.0), Step::RunBlocks(span(&[HOLD], rate) / 4, 13)]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::RunBlocks(29, 7),
                    Step::Param("quality", 0.0),
                    Step::Run(span(&[FADE], rate)),
                    Step::Param("quality", 3.0),
                    Step::Run(frames(&[FADE, FADE], rate)),
                ]
            },
            reached: |p| obs(p).fade == 1,
            completed: |p| obs(p).fade == 0 && obs(p).quality == 3,
        },
        // 6: after a quality switch with the other qualities' recordings made (the firmware's
        // memory reused at another layout); at T the stereo quality runs. The continuation visits
        // every other quality, each for its recording's span, and comes back.
        W {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                let mut h = setup();
                h.push(Step::Run(span(&[HOLD], rate)));
                for q in [3, 2, 1, 0] {
                    h.extend(visit(q, rate));
                }
                h
            },
            continuation: |rate| {
                let mut c = Vec::new();
                for q in [1, 2, 3, 0] {
                    c.extend(visit(q, rate));
                }
                c
            },
            reached: |p| obs(p).quality == 0 && obs(p).fade == 0,
            completed: |p| obs(p).quality == 0 && obs(p).fade == 0,
        },
        // 7: the flush boundary: input, then silence through the feedback, diffuser and reverb's
        // decay; the continuation runs on in silence, then a small input (the input level at its
        // lowest).
        W {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("feedback", 60.0),
                    Step::Param("reverb", 90.0),
                    Step::Run(span(&[REC[0]], rate)),
                    Step::Silent(span(&[Span::Seconds(4.0)], rate)),
                ]);
                h
            },
            continuation: |rate| vec![Step::Silent(997), Step::Param("input_level", -18.0), Step::Run(span(&[REC[0]], rate))],
            reached: |p| !obs(p).idle,
            completed: |p| !obs(p).idle,
        },
        // 8: inside the 32-frame firmware block (codec frames gathered); the continuation crosses
        // block edges with odd host block lengths and a parameter change between them.
        W {
            boundary: Boundary::InternalBlock,
            history: |rate| {
                let mut h = setup();
                h.push(Step::RunBlocks(span(&[HOLD], rate) / 7 * 7 + 40, 7));
                h
            },
            continuation: |_| vec![Step::RunBlocks(21, 7), Step::Param("pitch", 2.0), Step::RunBlocks(35, 5), Step::RunBlocks(33, 11)],
            reached: |p| (1..32).contains(&obs(p).fill),
            completed: |p| !obs(p).idle,
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
            reached: |p| obs(p).idle,
            completed: |p| !obs(p).idle,
        },
        // The frozen recording: recorded, then frozen (the input still arrives: the first 256
        // samples go to the crossfade tail) and the read point scrubbed across the whole frozen
        // span; the continuation scrubs on, unfreezes (recording resumes with the crossfade from
        // the tail) and replays the whole span.
        W {
            boundary: Boundary::Entry("frozen recording"),
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Run(span(&[REC[0], HOLD], rate)),
                    Step::Param("freeze", 1.0),
                    Step::Param("position", 0.0),
                    Step::Run(span(&[HOLD], rate)),
                ]);
                h
            },
            continuation: |rate| {
                let mut c = Vec::new();
                for k in 1..=10 {
                    c.extend([Step::Param("position", k as f32 * 10.0), Step::Run(span(&[REC[0]], rate) / 10)]);
                }
                c.extend([Step::Param("freeze", 0.0), Step::Run(span(&[REC[0], HOLD], rate))]);
                c
            },
            reached: |p| obs(p).frozen_tail > 0,
            completed: |p| obs(p).frozen_tail == 0,
        },
        // The same at the longest recording (the low-fidelity mono quality, behind the firmware's
        // 2:1 converters).
        W {
            boundary: Boundary::Entry("frozen low-fidelity recording"),
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("quality", 3.0),
                    Step::Run(span(&[REC[3], HOLD], rate)),
                    Step::Param("freeze", 1.0),
                    Step::Run(span(&[HOLD], rate)),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Param("position", 90.0),
                    Step::Run(span(&[REC[3]], rate) / 2),
                    Step::Param("position", 10.0),
                    Step::Run(span(&[REC[3]], rate) / 2),
                    Step::Param("freeze", 0.0),
                    Step::Run(span(&[REC[3]], rate)),
                ]
            },
            reached: |p| obs(p).frozen_tail > 0 && obs(p).quality == 3,
            completed: |p| obs(p).frozen_tail == 0,
        },
        // The trigger: rising edges restart the read at the oldest audio and glide back at the
        // rate of the triggers; at T the gate is high.
        W {
            boundary: Boundary::Entry("trigger"),
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Run(span(&[REC[0]], rate)), Step::Param("trigger", 1.0), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| {
                // The gate stays high first: no new edge, unless the gate was not high before.
                let mut c = vec![Step::Run(span(&[FADE], rate))];
                for _ in 0..4 {
                    c.extend([
                        Step::Param("trigger", 0.0),
                        Step::Run(span(&[HOLD], rate)),
                        Step::Param("trigger", 1.0),
                        Step::Run(span(&[FADE, FADE], rate)),
                    ]);
                }
                c.push(Step::Run(span(&[REC[0]], rate)));
                c
            },
            reached: |p| obs(p).last_gate,
            completed: |p| obs(p).last_gate,
        },
        // The delay lines: the reverb's and the diffuser's memories filled; the continuation reads
        // every position of each (its span), the input silent so only the memories sound.
        W {
            boundary: Boundary::Entry("delay lines"),
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("reverb", 100.0), Step::Param("diffusion", 100.0), Step::Run(span(&[REVERB, DIFFUSER, HOLD], rate))]);
                h
            },
            continuation: |rate| vec![Step::Silent(span(&[REVERB, DIFFUSER, HOLD], rate))],
            reached: |p| !obs(p).idle,
            completed: |p| !obs(p).idle,
        },
    ];
    v.extend(quality_witnesses());
    v
}

/// Quality `Q` running at T, its recording made; the continuation moves the window, the pitch and
/// the read point and runs the recording's span.
fn quality_history<const Q: usize>(rate: f32) -> Vec<Step> {
    let mut h = setup();
    h.extend([Step::Param("quality", Q as f32), Step::Run(span(&[FADE, FADE, REC[Q]], rate))]);
    h
}

fn quality_continuation<const Q: usize>(rate: f32) -> Vec<Step> {
    vec![Step::Param("window", 90.0), Step::Param("pitch", -12.0), Step::Param("position", 70.0), Step::Run(span(&[REC[Q]], rate))]
}

fn quality_reached<const Q: usize>(p: &TimeStretcher) -> bool {
    obs(p).quality == Q && obs(p).fade == 0
}

fn quality_witnesses() -> Vec<W> {
    macro_rules! each {
        ($($q:literal),*) => {
            vec![$(W {
                boundary: Boundary::Entry(concat!("quality ", stringify!($q), " at T")),
                history: quality_history::<$q>,
                continuation: quality_continuation::<$q>,
                reached: quality_reached::<$q>,
                completed: quality_reached::<$q>,
            }),*]
        };
    }
    each!(0, 1, 2, 3)
}

/// The Time Stretcher's entry: its witnesses, and each boundary that does not apply with its
/// code-backed explanation.
pub(super) fn entry() -> Entry<TimeStretcher> {
    Entry {
        id: "time_stretcher",
        make: || Box::new(TimeStretcher::new()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: vec![
            (Boundary::VoicesStolen, "no voices: `TimeStretcher` takes no notes (`note_on`, `event` are the trait defaults)"),
            (Boundary::ReleaseTail, "no notes, so no release tails: the recording's and the reverb's decay are the flush and delay-line witnesses'"),
            (Boundary::Pending, "nothing pending: `set_param` applies at once (the quality restart it may start is boundary 5's)"),
            (Boundary::Sustain, "no keys or pedals: `TimeStretcher` takes no notes"),
        ],
        // The buffer memory past each live extent, the path's `starved` count and converter scratch.
        poison: Some(|p, k| p.poison_unobservable(k + 1)),
    }
}
