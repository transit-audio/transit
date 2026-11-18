//! The Macro Oscillator's witness set (ADR 0029, amendment for #742, section 13; slice 1 push 2).
//!
//! Every checklist boundary applies to the Macro Oscillator and has a witness here; the entry-
//! specific ones are its delay lines (the string engine's strings, the particle engine's diffuser,
//! the string machine's ensemble, the voice's trigger delay), its generators, its transport anchor
//! and controllers, the meter it holds after the transport stops, and a re-strike's open note-ons. The spans the inventory declares, fed to the one duration calculator:
//! - `RELEASE`: a voice's release at the witnesses' Decay (15 %) to idle (its low-pass gate tail,
//!   then [`transit_dsp::plugins::macro_osc::voice::QUIET_SECONDS`] of silence);
//! - `RAMP`: the level smoother's ramp (a 20 ms time constant, settled within 1e-6);
//! - `VISIT`: one engine's visit (a note held, then released), long enough for its lines;
//! - `DIFFUSER`: the particle engine's diffuser ring (8192 samples) and `STRING`: a string's
//!   lines (1024 plus 256 samples);
//! - `SHAPER_RELEASE`: the shaper's amp release at the witnesses' 40 ms, to idle.

use super::*;
use transit_dsp::plugins::macro_osc::{MacroOscillator, row_param};

const RELEASE: Span = Span::Seconds(1.0);
const RAMP: Span = Span::Seconds(0.3);
const VISIT: Span = Span::Seconds(0.05);
const DIFFUSER: Span = Span::Samples(8192);
const STRING: Span = Span::Samples(1024 + 256);
const SHAPER_RELEASE: Span = Span::Seconds(0.2);
/// A short block-free hold of a note (the trigger delay's five blocks and the attack).
const HOLD: Span = Span::Seconds(0.02);

/// Engines in the firmware's order: the particle engine (its diffuser), the string engine (its
/// strings), the string machine (its ensemble), the filtered noise engine (its generators).
const PARTICLE: f32 = 18.0;
const STRINGS: f32 = 19.0;
const STRING_MACHINE: f32 = 6.0;
const NOISE: f32 = 17.0;
const CHIPTUNE: f32 = 7.0;

type W = Witness<MacroOscillator>;

fn obs(p: &MacroOscillator) -> transit_dsp::plugins::macro_osc::Observed {
    p.observe()
}

fn idle(p: &MacroOscillator) -> bool {
    obs(p).sounding == 0 && obs(p).pending == 0
}

/// A run to the end of `spans` (a release to idle) in whole maximum blocks: no odd lengths are
/// needed there, and fewer blocks cost fewer record comparisons.
fn run_out(spans: &[Span], rate: f32) -> Step {
    Step::RunBlocks(frames(spans, rate), MAX_BLOCK)
}

/// Frames for `spans` at `rate`, without the duration calculator's extra block.
fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

/// The witnesses' common setup: a short Decay, so release tails end quickly, and `voices` voices.
fn setup(voices: usize) -> Vec<Step> {
    vec![Step::Param("decay", 0.0), Step::Param("voices", (voices - 1) as f32)]
}

/// Boundary 1's obligation (first blocks with notes and parameter changes), then every note
/// released and the voices run to idle.
fn first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::Param("model", 10.0),
        Step::On(60, 100, 3),
        Step::On(67, 90, 29),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("harmonics", 80.0),
        Step::Param("level", -12.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Off(60, 5),
        Step::Off(67, 11),
        run_out(&[RELEASE, RAMP], rate),
    ]
}

/// One visit of `engine`: a note held, then released, for `hold` (its lines' span at least).
fn visit(engine: f32, key: u8, hold: Span, rate: f32) -> Vec<Step> {
    vec![Step::Param("model", engine), Step::On(key, 110, 7), Step::Run(span(&[hold], rate)), Step::Off(key, 3), Step::Run(span(&[VISIT], rate))]
}

/// The span an engine's visit holds its note for: its lines' span where it has lines.
fn hold_of(engine: f32) -> Span {
    if engine == PARTICLE {
        DIFFUSER
    } else if engine == STRINGS {
        STRING
    } else {
        VISIT
    }
}

fn witnesses() -> Vec<W> {
    let mut v = vec![
        // 1: after prepare, before any process.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [setup(8), first_blocks(rate)].concat(),
            reached: |p| obs(p).engines.iter().all(Option::is_none),
            completed: idle,
        },
        // 2: after a reset in the middle of sounding notes and a ramp.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = setup(8);
                h.extend([Step::Param("model", 13.0), Step::On(48, 120, 0), Step::On(55, 80, 40), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::Param("level", -20.0), Step::Run(97), Step::Reset]);
                h
            },
            continuation: |rate| first_blocks(rate),
            reached: |p| obs(p).sounding == 0 && obs(p).pending == 0,
            completed: idle,
        },
        // 3: every voice sounding (four voices, five keys: one stolen); the continuation steals
        // again, then releases all and runs until every voice is idle.
        W {
            boundary: Boundary::VoicesStolen,
            history: |rate| {
                let mut h = setup(4);
                h.push(Step::Param("model", 0.0));
                for (i, key) in [48u8, 52, 55, 59, 62].into_iter().enumerate() {
                    h.push(Step::On(key, 70 + 10 * i as u8, 0));
                    h.push(Step::Run(span(&[HOLD], rate)));
                }
                h
            },
            continuation: |rate| {
                // Steal twice (the oldest held voices), then release two voices one after the
                // other and strike two keys: each takes the quietest voice in its release (the
                // loudness decides, not the age), then release everything and run to idle.
                let mut c = vec![Step::On(64, 100, 2), Step::Run(37), Step::On(67, 100, 0), Step::Run(span(&[HOLD], rate))];
                c.extend([Step::Off(62, 0), Step::Run(span(&[HOLD], rate)), Step::Off(59, 0), Step::Run(97)]);
                c.extend([Step::On(70, 100, 0), Step::Run(span(&[HOLD], rate)), Step::On(72, 100, 0), Step::Run(span(&[HOLD], rate))]);
                for (k, key) in [64u8, 67, 70, 72].into_iter().enumerate() {
                    c.push(Step::Off(key, k));
                }
                c.push(run_out(&[RELEASE], rate));
                c
            },
            reached: |p| obs(p).held & 0xF == 0xF,
            completed: idle,
        },
        // 4: inside release tails; the restored release runs to its end without a retrigger.
        W {
            boundary: Boundary::ReleaseTail,
            history: release_history,
            continuation: |rate| vec![run_out(&[RELEASE], rate)],
            reached: in_release,
            completed: idle,
        },
        // 4, the separate branch: retriggered during the release, released again, run to idle.
        W {
            boundary: Boundary::ReleaseTail,
            history: release_history,
            continuation: |rate| vec![Step::On(60, 90, 1), Step::Run(span(&[HOLD], rate)), Step::Off(60, 0), run_out(&[RELEASE], rate)],
            reached: in_release,
            completed: idle,
        },
        // 5: mid-ramp (the level smoother, with the shaper on, a matrix row and the synced LFO
        // moving the voices); the target changes again mid-ramp, then the ramp completes, plus one
        // block.
        W {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = setup(8);
                h.extend(shaper_on());
                h.extend([
                    Step::On(57, 100, 0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("level", -30.0),
                    Step::RunBlocks(span(&[HOLD], rate) / 2, 13),
                ]);
                h
            },
            continuation: |rate| vec![Step::RunBlocks(29, 7), Step::Param("level", 3.0), Step::Run(frames(&[RAMP], rate))],
            reached: |p| obs(p).ramping,
            completed: |p| !obs(p).ramping,
        },
        // 6: after an engine switch with every other engine's state left non-default: each of
        // the 24 engines visited once (the last one sounds at T); the continuation revisits every
        // dormant engine with a note, each for its lines' span.
        W {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                let mut h = setup(1);
                h.extend(shaper_on());
                for e in 0..24u8 {
                    h.extend(visit(f32::from(e), 40 + e, hold_of(f32::from(e)), rate));
                }
                h.extend([Step::Param("model", 9.0), Step::On(52, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::Off(52, 0)];
                for e in (0..24u8).filter(|&e| e != 9) {
                    c.extend(visit(f32::from(e), 64 - e, hold_of(f32::from(e)), rate));
                }
                c.push(run_out(&[RELEASE], rate));
                c
            },
            reached: |p| obs(p).engines[0] == Some(9),
            completed: |p| obs(p).engines[0] == Some(23) && idle(p),
        },
        // 7: the flush boundary: a loud burst, then silence through the voices' decay to idle
        // (the firmware's flushes to zero); the continuation runs on, then plays a small input (a
        // quiet note) and runs it to idle.
        W {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = setup(8);
                h.extend([Step::Param("model", 8.0), Step::On(36, 127, 0), Step::On(48, 127, 0), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::Off(36, 0), Step::Off(48, 0), run_out(&[RELEASE], rate)]);
                h
            },
            continuation: |rate| {
                vec![Step::Run(997), Step::On(72, 1, 5), Step::Run(span(&[HOLD], rate)), Step::Off(72, 0), run_out(&[RELEASE], rate)]
            },
            reached: idle,
            completed: idle,
        },
        // 8: inside the 12-frame firmware block; the continuation crosses internal-block edges with
        // odd block lengths, events at odd offsets included.
        W {
            boundary: Boundary::InternalBlock,
            history: |rate| {
                let mut h = setup(8);
                h.extend([Step::Param("model", 1.0), Step::On(62, 100, 0), Step::RunBlocks(span(&[HOLD], rate) / 12 * 12 + 5, 12)]);
                h
            },
            continuation: |_| vec![Step::RunBlocks(21, 7), Step::On(66, 90, 3), Step::RunBlocks(35, 5), Step::Off(62, 1), Step::RunBlocks(33, 11)],
            reached: |p| (1..12).contains(&obs(p).pos),
            completed: |p| (1..12).contains(&obs(p).pos),
        },
        // 9: events pending past the block (a note-on, a controller, a pitch bend, a note-off);
        // the continuation delivers every one, runs past the latest due frame, plus one block.
        W {
            boundary: Boundary::Pending,
            history: |rate| {
                let mut h = setup(8);
                h.extend([Step::Param("model", 12.0), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::On(57, 100, 900), Step::Midi([0xB0, 1, 90], 700), Step::Midi([0xE0, 0, 100], 1200), Step::Off(50, 1500)]);
                h.push(Step::RunBlocks(256, 256));
                h
            },
            continuation: |rate| vec![Step::RunBlocks(1500 + MAX_BLOCK, 64), Step::Off(57, 0), run_out(&[RELEASE], rate)],
            reached: |p| obs(p).pending == 4,
            completed: idle,
        },
        // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
        // case's rate (T); the continuation meets boundary 1's obligation again.
        W {
            boundary: Boundary::SecondPrepare,
            history: |rate| {
                let other = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                let mut h = vec![Step::Prepare(other)];
                h.extend(setup(8));
                h.extend(first_blocks(other));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).sounding == 0 && obs(p).engines.iter().all(Option::is_none),
            completed: idle,
        },
        // 11: a key released while the sustain pedal stays down, beside a held key; the
        // continuation keeps the sustained note sounding, releases the held key too while the
        // pedal is still down (the pedal keeps it), then releases the pedal and runs to idle.
        W {
            boundary: Boundary::Sustain,
            history: |rate| {
                let mut h = setup(8);
                h.extend([Step::Param("model", 14.0), Step::Midi([0xB0, 64, 127], 0), Step::On(60, 100, 2), Step::On(64, 90, 9)]);
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Off(60, 4), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Run(span(&[HOLD], rate)),
                    Step::Off(64, 0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Midi([0xB0, 64, 0], 3),
                    Step::Run(97),
                    run_out(&[RELEASE], rate),
                ]
            },
            reached: |p| obs(p).sustain && obs(p).pedalled != 0 && obs(p).held != 0,
            completed: |p| !obs(p).sustain && idle(p),
        },
        // The delay lines: the particle engine's diffuser, the string engine's strings and the
        // string machine's ensemble filled; the continuation reads every position of each (its
        // span), the voice's trigger delay with every note.
        W {
            boundary: Boundary::Entry("delay lines"),
            history: |rate| {
                let mut h = setup(3);
                for (e, key) in [(PARTICLE, 50u8), (STRINGS, 28), (STRING_MACHINE, 55)] {
                    h.extend([Step::Param("model", e), Step::On(key, 120, 0), Step::Run(span(&[hold_of(e), HOLD], rate))]);
                }
                h
            },
            continuation: |rate| {
                let mut c = Vec::new();
                for (e, key) in [(PARTICLE, 52u8), (STRINGS, 29), (STRING_MACHINE, 57)] {
                    c.extend([Step::Param("model", e), Step::On(key, 100, 0), Step::Run(span(&[hold_of(e), HOLD], rate))]);
                }
                c
            },
            reached: |p| obs(p).sounding & 0b111 == 0b111,
            completed: |p| obs(p).engines[2] == Some(STRING_MACHINE as usize),
        },
        // The generators: every voice's generator and the shaper's drawn from by the noise engine
        // and new notes; the continuation draws from them again.
        W {
            boundary: Boundary::Entry("generators"),
            history: |rate| {
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::Param("model", NOISE), Step::On(60, 100, 0), Step::On(61, 100, 0), Step::Run(span(&[VISIT], rate))]);
                h
            },
            continuation: |rate| {
                vec![Step::On(62, 100, 0), Step::Run(span(&[VISIT], rate)), Step::Param("model", CHIPTUNE), Step::Run(span(&[VISIT], rate))]
            },
            reached: |p| obs(p).sounding & 0b11 == 0b11,
            completed: |p| obs(p).sounding & 0b111 == 0b111,
        },
        // The transport anchor and the controllers: the transport plays (the synced LFO counts
        // from its anchor), the rack macros, the wheel, channel and poly pressure and the bend move
        // matrix rows; the continuation plays on, jumps the tempo (a new anchor), stops, and runs
        // the shaped release to idle.
        W {
            boundary: Boundary::Entry("transport and controllers"),
            history: |rate| {
                let mut h = setup(8);
                h.extend(shaper_on());
                h.extend(controller_rows());
                h.extend([Step::Transport(Some(128.0)), Step::Macros([0.25, 0.5, 0.75, 1.0, 0.0, 0.1, 0.2, 0.3]), Step::On(55, 100, 0)]);
                h.extend([Step::Midi([0xB0, 1, 64], 0), Step::Midi([0xD0, 70, 0], 5), Step::Midi([0xA0, 55, 90], 9), Step::Midi([0xE0, 0, 80], 13)]);
                h.push(Step::Run(span(&[HOLD], rate)));
                h
            },
            continuation: |rate| {
                vec![
                    Step::Run(span(&[HOLD], rate)),
                    Step::Transport(Some(96.0)),
                    Step::Macros([0.0; 8]),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Transport(None),
                    Step::Midi([0xB0, 121, 0], 0),
                    Step::Off(55, 2),
                    run_out(&[SHAPER_RELEASE], rate),
                ]
            },
            reached: |p| obs(p).anchored && obs(p).held != 0,
            completed: |p| !obs(p).anchored && idle(p),
        },
        // The meter after the transport stops: it played in 3/4 and was told it stopped; the
        // continuation carries on with no transport (the synced LFO runs at the tempo over the
        // bar it last heard), changes a parameter (the settings are worked out again from the
        // bar it holds), and runs the shaped release to idle.
        W {
            boundary: Boundary::Entry("stopped transport"),
            history: |rate| {
                let mut h = setup(8);
                h.extend(shaper_on());
                h.extend([Step::Transport(Some(140.0)), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate)), Step::Transport(None)]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("lfo2_amount", 70.0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Off(50, 0),
                    run_out(&[SHAPER_RELEASE], rate),
                ]
            },
            reached: |p| !obs(p).anchored && obs(p).held != 0,
            completed: idle,
        },
        // A key struck again before its first note's off (two open note-ons): the continuation's
        // first off closes the older one and leaves the voice sounding; the second releases it.
        W {
            boundary: Boundary::Entry("re-strike"),
            history: |rate| {
                let mut h = setup(8);
                h.extend([Step::Param("model", 8.0), Step::On(60, 100, 0), Step::Run(span(&[HOLD], rate)), Step::On(60, 80, 5)]);
                h.push(Step::Run(span(&[HOLD], rate)));
                h
            },
            continuation: |rate| vec![Step::Off(60, 0), Step::Run(span(&[HOLD], rate)), Step::Off(60, 3), run_out(&[RELEASE], rate)],
            reached: |p| obs(p).held != 0,
            completed: idle,
        },
        // The allocator's loudness and the quiet count: the hi-hat model's hits decay faster than
        // the loudness the allocator keeps falls, so each voice's loudness is its remembered peak,
        // not its present one. At T two voices are held and two are in their release, silent and
        // counting down to idle: the older of those was struck loud, the younger soft, so the
        // quietest is not the oldest. The continuation strikes one key, which takes the quietest
        // voice in its release, lets the other count down to idle, then releases everything.
        W {
            boundary: Boundary::Entry("loudness and quiet count"),
            history: |rate| {
                // Morph sets a drum model's decay: at 0 a hit is over within a few milliseconds.
                let mut h = setup(4);
                h.extend([Step::Param("model", 23.0), Step::Param("morph", 0.0)]);
                for (key, vel) in [(48u8, 127u8), (52, 60), (55, 10), (59, 60)] {
                    h.push(Step::On(key, vel, 0));
                    h.push(Step::Run(span(&[HOLD], rate)));
                }
                // Both released within the quiet span (50 ms) before T, 45 and 35 ms, each silent by then.
                let (gap, wait) = (Span::Seconds(0.01), Span::Seconds(0.035));
                h.extend([Step::Off(48, 0), Step::Run(span(&[gap], rate)), Step::Off(55, 0), Step::Run(span(&[wait], rate))]);
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::On(62, 100, 0), Step::Run(span(&[HOLD], rate)), Step::Run(span(&[VISIT, HOLD], rate))];
                for (k, key) in [52u8, 59, 62].into_iter().enumerate() {
                    c.push(Step::Off(key, k));
                }
                c.push(run_out(&[RELEASE], rate));
                c
            },
            reached: |p| obs(p).held.count_ones() == 2 && obs(p).sounding.count_ones() == 4 && obs(p).counting.count_ones() == 2,
            completed: idle,
        },
        // The allocator's loudness when the quietest released voice is not the oldest: the virtual
        // analog model's level follows the velocity, two voices struck loud (the oldest) and soft
        // are released together, so at T the soft one has the lower remembered peak; the
        // continuation strikes a key, which takes that voice, then releases everything.
        W {
            boundary: Boundary::Entry("loudness"),
            history: |rate| {
                let mut h = setup(4);
                h.push(Step::Param("model", 8.0));
                for (key, vel) in [(48u8, 127u8), (52, 90), (55, 15), (59, 90)] {
                    h.push(Step::On(key, vel, 0));
                    h.push(Step::Run(span(&[HOLD], rate)));
                }
                h.extend([Step::Off(48, 0), Step::Off(55, 0), Step::Run(span(&[Span::Seconds(0.005)], rate))]);
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::On(62, 100, 0), Step::Run(span(&[HOLD], rate))];
                for (k, key) in [52u8, 59, 62].into_iter().enumerate() {
                    c.push(Step::Off(key, k));
                }
                c.push(run_out(&[RELEASE], rate));
                c
            },
            reached: |p| obs(p).held.count_ones() == 2 && obs(p).sounding.count_ones() == 4,
            completed: idle,
        },
    ];
    // Every engine active at T, in the middle of a note: an engine that resets on a switch (so its
    // dormant state is not read again) is still recorded while it plays.
    v.extend(engine_witnesses());
    v
}

/// The engine at T of [`engine_witnesses`]' witness for `E`: a note held on it for `VISIT`.
fn engine_history<const E: u8>(rate: f32) -> Vec<Step> {
    let mut h = setup(8);
    h.extend([Step::Param("model", f32::from(E)), Step::On(50, 110, 5), Step::Run(span(&[VISIT], rate))]);
    h
}

/// The note goes on; Harmonics moves and the held key is struck again (a six-operator engine
/// then loads a patch it had not loaded, from the patches its restore rebuilt, and the speech
/// engine changes word bank); the note is released and its tail plays for `VISIT`.
fn engine_continuation<const E: u8>(rate: f32) -> Vec<Step> {
    vec![
        Step::Run(span(&[VISIT], rate)),
        Step::Param("harmonics", 90.0),
        Step::On(50, 100, 3),
        Step::Run(span(&[VISIT], rate)),
        // One release for each strike.
        Step::Off(50, 2),
        Step::Off(50, 5),
        Step::Run(span(&[VISIT], rate)),
    ]
}

fn engine_reached<const E: u8>(p: &MacroOscillator) -> bool {
    obs(p).engines[0] == Some(usize::from(E)) && obs(p).held & 1 == 1
}

fn engine_completed<const E: u8>(p: &MacroOscillator) -> bool {
    obs(p).engines[0] == Some(usize::from(E)) && obs(p).held == 0
}

/// One witness per engine (the 24 of the bank): see [`witnesses`].
fn engine_witnesses() -> Vec<W> {
    macro_rules! each {
        ($($e:literal),*) => {
            vec![$(W {
                boundary: Boundary::Entry(concat!("engine ", stringify!($e), " at T")),
                history: engine_history::<$e>,
                continuation: engine_continuation::<$e>,
                reached: engine_reached::<$e>,
                completed: engine_completed::<$e>,
            }),*]
        };
    }
    each!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23)
}

/// Two notes released into their tails, which are still playing at T.
fn release_history(rate: f32) -> Vec<Step> {
    let mut h = setup(8);
    h.extend([Step::Param("model", 11.0), Step::On(60, 100, 0), Step::On(63, 100, 0), Step::Run(span(&[HOLD], rate))]);
    h.extend([Step::Off(60, 0), Step::Off(63, 0), Step::Run(span(&[HOLD], rate))]);
    h
}

fn in_release(p: &MacroOscillator) -> bool {
    let o = obs(p);
    o.sounding != 0 && o.held == 0
}

/// The shaper on (short envelopes, a resonant filter with key tracking and envelope, both LFOs
/// free-running, so a new note takes the instrument's running phase: one with a fade-in, one
/// synced to a bar division, anchored to the song position while the transport plays) and matrix
/// rows from LFO 1 to the cutoff, from LFO 2 to Timbre and from the note's random value to Morph.
fn shaper_on() -> Vec<Step> {
    vec![
        Step::Param("shaper", 1.0),
        Step::Param("amp_attack", 3.0),
        Step::Param("amp_release", 40.0),
        Step::Param("filter_mode", 1.0),
        Step::Param("cutoff", 2_000.0),
        Step::Param("resonance", 40.0),
        Step::Param("key_track", 50.0),
        Step::Param("filter_env_amount", 30.0),
        Step::Param("lfo1_shape", 5.0),
        Step::Param("lfo1_rate", 7.0),
        Step::Param("lfo1_fade", 30.0),
        Step::Param("lfo1_amount", 80.0),
        Step::Param("lfo1_retrigger", 0.0),
        Step::Param("lfo2_sync", 1.0),
        Step::Param("lfo2_retrigger", 0.0),
        Step::Param("lfo2_division", 1.0),
        Step::Param("lfo2_amount", 50.0),
        Step::Param("mod1_source", 1.0),
        Step::Param("mod1_dest", 7.0),
        Step::Param("mod1_amount", 40.0),
        Step::Param("mod8_source", 2.0),
        Step::Param("mod8_dest", 3.0),
        Step::Param("mod8_amount", 40.0),
        Step::Param("mod9_source", 12.0),
        Step::Param("mod9_dest", 4.0),
        Step::Param("mod9_amount", 30.0),
    ]
}

/// Matrix rows from the controllers (the wheel, aftertouch, poly aftertouch, the bend, a macro,
/// the key) to the voice's targets (Harmonics, Timbre, Morph, Pitch, Pan, Level).
fn controller_rows() -> Vec<Step> {
    let mut v = Vec::new();
    for (row, source, dest) in [(1usize, 10.0, 2.0), (2, 8.0, 3.0), (3, 9.0, 4.0), (4, 11.0, 1.0), (5, 13.0, 14.0), (6, 6.0, 6.0)] {
        let id = |field| row_param(row, field).expect("a matrix row parameter");
        v.extend([Step::Param(id(0), source), Step::Param(id(2), dest), Step::Param(id(3), 30.0)]);
    }
    v
}

/// The Macro Oscillator's entry: every checklist boundary has a witness.
pub(super) fn entry() -> Entry<MacroOscillator> {
    Entry {
        id: "macro_oscillator",
        make: || Box::new(MacroOscillator::new()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: vec![],
        // The slots' last-block outputs, the port voice's block buffers and decay envelope, the
        // dormant engines' reset-rewritten state, a dormant six-operator engine's patches and the
        // speech frames no read reaches.
        poison: Some(|p, k| p.poison_unobservable(k + 1)),
    }
}
