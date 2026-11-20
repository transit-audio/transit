//! The Modal Voice's witness set (ADR 0029, amendment for #742, section 13; slice 3).
//!
//! The instrument plays the module's one voice mono, with a real gate (`modal_voice.rs`, the module
//! docs): a key down raises it, the last key up (or the pedal lifting) lowers it, and a new note
//! while a key is held retriggers it. "Every voice sounding, one stolen" is that voice taken by a
//! newer key while older keys are still held; "idle" is no key down, the gate and the shapers'
//! envelopes released and silent, and nothing queued ([`idle`]): the part's resonators ring on by
//! their damping and its reverb by its time, and the record is compared at every block of that
//! ring-down. The spans the inventory declares, fed to the one duration calculator (the part runs
//! at 32 kHz, so a span in part samples is given in seconds and scales with the case's rate):
//! - `RAMP`: the level smoother's ramp (a 20 ms time constant, settled within 1e-6);
//! - `LINES`: the longest of the voice's lines but the reverb: a string's two lines (2048 plus
//!   1024 part samples; the tube's 2048, the bowed modes' 1024 and the diffuser's 1024 are shorter);
//!   `REVERB`: the reverb's memory (32768 part samples);
//! - `SHAPER_RELEASE`: the shaper's amp release at the witnesses' 40 ms, to its end (the module's
//!   own envelope is shorter at the witnesses' Contour);
//! - `HOLD`: a short hold (a few part blocks); `VISIT`: one model's visit.

use super::*;
use transit_dsp::plugins::modal_voice::{ModalVoiceInstrument, Observed};

const PART_RATE: f64 = 32_000.0;
const RAMP: Span = Span::Seconds(0.3);
const LINES: Span = Span::Seconds((2048.0 + 1024.0) / PART_RATE);
const REVERB: Span = Span::Seconds(32_768.0 / PART_RATE);
const SHAPER_RELEASE: Span = Span::Seconds(0.2);
const HOLD: Span = Span::Seconds(0.02);
const VISIT: Span = Span::Seconds(0.08);

/// The models: the modal bank, one string, the chord strings, the FM pair.
const MODAL: f32 = 0.0;
const STRING: f32 = 1.0;
const STRINGS: f32 = 2.0;
const FM_PAIR: f32 = 3.0;

type W = Witness<ModalVoiceInstrument>;

fn obs(p: &ModalVoiceInstrument) -> Observed {
    p.observe()
}

/// What the instrument tracks as idle (see the module docs).
fn idle(p: &ModalVoiceInstrument) -> bool {
    let o = obs(p);
    o.keys == 0 && !o.gate && !o.shaper_gate && !o.retrigger && o.pending == 0 && o.amp == 0.0
}

fn run_out(spans: &[Span], rate: f32) -> Step {
    Step::RunBlocks(frames(spans, rate), MAX_BLOCK)
}

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

/// The witnesses' common setup: high damping, a short Contour (a percussive envelope) and a short
/// amp release, and every exciter sounding (bow, blow through the tube, strike), so a witness reads
/// all of them.
fn setup() -> Vec<Step> {
    vec![
        Step::Param("damping", 70.0),
        Step::Param("contour", 10.0),
        Step::Param("amp_release", 40.0),
        Step::Param("bow_level", 40.0),
        Step::Param("blow_level", 90.0),
        Step::Param("strike_level", 80.0),
    ]
}

/// Boundary 1's obligation (first blocks with notes and parameter changes), then every note
/// released and the instrument run to idle.
fn first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::Param("model", STRING),
        Step::On(60, 100, 3),
        Step::On(67, 90, 29),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("geometry", 80.0),
        Step::Param("level", -6.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Off(60, 5),
        Step::Off(67, 11),
        run_out(&[RAMP, SHAPER_RELEASE], rate),
    ]
}

/// One visit of `model`: a note held, then released, for `hold`.
fn visit(model: f32, key: u8, hold: Span, rate: f32) -> Vec<Step> {
    vec![Step::Param("model", model), Step::On(key, 110, 7), Step::Run(span(&[hold], rate)), Step::Off(key, 3), Step::Run(span(&[HOLD], rate))]
}

/// The shaper on (short envelopes, a resonant filter with key tracking and envelope, both LFOs
/// free-running: one with a fade-in, one synced to a bar division) and matrix rows from LFO 1 to
/// the cutoff, from LFO 2 to Brightness and from the note's random value to Damping.
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
/// the key) to the part's controls, Pitch, Pan and Level.
fn controller_rows() -> Vec<Step> {
    let rows: [(&'static str, &'static str, &'static str, f32, f32); 6] = [
        ("mod2_source", "mod2_dest", "mod2_amount", 10.0, 2.0),
        ("mod3_source", "mod3_dest", "mod3_amount", 8.0, 3.0),
        ("mod4_source", "mod4_dest", "mod4_amount", 9.0, 5.0),
        ("mod5_source", "mod5_dest", "mod5_amount", 11.0, 1.0),
        ("mod6_source", "mod6_dest", "mod6_amount", 13.0, 14.0),
        ("mod7_source", "mod7_dest", "mod7_amount", 6.0, 6.0),
    ];
    rows.into_iter().flat_map(|(s, d, a, source, dest)| [Step::Param(s, source), Step::Param(d, dest), Step::Param(a, 30.0)]).collect()
}

fn witnesses() -> Vec<W> {
    let mut v = vec![
        // 1: after prepare, before any process.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [setup(), first_blocks(rate)].concat(),
            reached: |p| obs(p).fresh && obs(p).keys == 0,
            completed: idle,
        },
        // 2: after a reset in the middle of a sounding note and a ramp.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("model", STRINGS), Step::On(48, 120, 0), Step::On(55, 80, 40), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::Param("level", -20.0), Step::Run(97), Step::Reset]);
                h
            },
            continuation: |rate| first_blocks(rate),
            reached: |p| obs(p).fresh && obs(p).keys == 0 && obs(p).pending == 0,
            completed: idle,
        },
        // 3: the voice sounding the newest of three held keys (each new key took it from the one
        // before); the continuation takes it again with a fourth key, releases that one (the voice
        // returns to the newest key still held, legato), then releases all and runs to idle.
        W {
            boundary: Boundary::VoicesStolen,
            history: |rate| {
                let mut h = setup();
                h.push(Step::Param("model", MODAL));
                for (i, key) in [48u8, 52, 55].into_iter().enumerate() {
                    h.push(Step::On(key, 80 + 10 * i as u8, 0));
                    h.push(Step::Run(span(&[HOLD], rate)));
                }
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::On(59, 100, 2), Step::Run(span(&[HOLD], rate)), Step::Off(59, 0), Step::Run(span(&[HOLD], rate))];
                for (k, key) in [48u8, 52, 55].into_iter().enumerate() {
                    c.push(Step::Off(key, k));
                }
                c.push(run_out(&[SHAPER_RELEASE], rate));
                c
            },
            reached: |p| obs(p).keys == 3 && obs(p).gate,
            completed: idle,
        },
        // 4: inside the release (no key down, the gate low, the shapers' amp envelope falling); the
        // restored release runs to its end without a new note.
        W {
            boundary: Boundary::ReleaseTail,
            history: release_history,
            continuation: |rate| vec![run_out(&[SHAPER_RELEASE], rate)],
            reached: in_release,
            completed: idle,
        },
        // 4, the separate branch: retriggered during the release, released again, run to idle.
        W {
            boundary: Boundary::ReleaseTail,
            history: release_history,
            continuation: |rate| vec![Step::On(60, 90, 1), Step::Run(span(&[HOLD], rate)), Step::Off(60, 0), run_out(&[SHAPER_RELEASE], rate)],
            reached: in_release,
            completed: idle,
        },
        // 5: mid-ramp (the level smoother, with the shaper on and matrix rows moving the part);
        // the target changes again mid-ramp, then the ramp completes, plus one block.
        W {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = setup();
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
        // 6: after a model switch with every other model's state left non-default (the modal bank
        // and its bowed modes, the strings, the FM pair, each played for its lines); at T the modal
        // bank plays, so the strings and the FM pair hold what they kept. The continuation revisits
        // every model, each for its lines' span.
        W {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                let mut h = setup();
                h.extend(shaper_on());
                for m in [STRINGS, STRING, FM_PAIR, MODAL] {
                    h.extend(visit(m, 40 + m as u8, LINES, rate));
                }
                h.extend([Step::Param("model", MODAL), Step::On(52, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::Off(52, 0)];
                for m in [FM_PAIR, STRING, STRINGS, MODAL] {
                    c.extend(visit(m, 64 - m as u8, LINES, rate));
                }
                c.push(run_out(&[SHAPER_RELEASE], rate));
                c
            },
            reached: |p| obs(p).model == 0 && obs(p).keys == 1,
            completed: |p| obs(p).model == 0 && idle(p),
        },
        // 7: the flush boundary: a loud note at the lowest damping, then silence long enough for
        // the resonator's tail to fall into the range its flush-to-zero arithmetic flushes; the
        // continuation runs on, then plays a small input (a soft note) and runs it out.
        W {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("damping", 0.0), Step::Param("space", 0.0), Step::Param("model", MODAL), Step::On(36, 127, 0)]);
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Off(36, 0), run_out(&[Span::Seconds(3.0)], rate)]);
                h
            },
            continuation: |rate| {
                vec![Step::Run(997), Step::On(72, 1, 5), Step::Run(span(&[HOLD], rate)), Step::Off(72, 0), run_out(&[SHAPER_RELEASE], rate)]
            },
            reached: idle,
            completed: idle,
        },
        // 8: inside the part's 16-frame block (frames rendered ahead of the converter's output) and
        // inside the shaper's 12-frame block; the continuation crosses both edges with odd block
        // lengths, events at odd offsets included.
        W {
            boundary: Boundary::InternalBlock,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("model", STRING), Step::On(62, 100, 0), Step::RunBlocks(span(&[HOLD], rate) / 16 * 16 + 5, 16)]);
                h
            },
            continuation: |_| vec![Step::RunBlocks(21, 7), Step::On(66, 90, 3), Step::RunBlocks(35, 5), Step::Off(62, 1), Step::RunBlocks(33, 11)],
            reached: |p| (1..16).contains(&obs(p).ahead) && obs(p).shaper_at != 0,
            completed: |p| obs(p).ahead > 0,
        },
        // 9: events pending past the block (a note-on, a controller, a pitch bend, a note-off);
        // the continuation delivers every one, runs past the latest due frame, plus one block.
        W {
            boundary: Boundary::Pending,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("model", STRINGS), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::On(57, 100, 900), Step::Midi([0xB0, 1, 90], 700), Step::Midi([0xE0, 0, 100], 1200), Step::Off(50, 1500)]);
                h.push(Step::RunBlocks(256, 256));
                h
            },
            continuation: |rate| vec![Step::RunBlocks(1500 + MAX_BLOCK, 64), Step::Off(57, 0), run_out(&[SHAPER_RELEASE], rate)],
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
                h.extend(setup());
                h.extend(first_blocks(other));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).fresh && obs(p).keys == 0,
            completed: idle,
        },
        // 11: a key released while the sustain pedal stays down, beside a held key (the gate stays
        // up); the continuation releases the held key too while the pedal is down, then releases
        // the pedal (the gate falls) and runs to idle.
        W {
            boundary: Boundary::Sustain,
            history: |rate| {
                let mut h = setup();
                h.extend(shaper_on());
                h.extend([Step::Param("model", STRING), Step::Midi([0xB0, 64, 127], 0), Step::On(60, 100, 2), Step::On(64, 90, 9)]);
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
                    run_out(&[SHAPER_RELEASE], rate),
                ]
            },
            reached: |p| obs(p).sustain && obs(p).pedalled && obs(p).keys == 2 && obs(p).gate,
            completed: |p| !obs(p).sustain && idle(p),
        },
        // The delay lines: the reverb's memory filled (Space above its threshold), then the
        // strings, the modal bank's bowed modes, the tube and the diffuser; the continuation reads
        // every position of each (its span).
        W {
            boundary: Boundary::Entry("delay lines"),
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("space", 150.0), Step::Param("model", STRINGS), Step::On(40, 120, 0), Step::Run(span(&[REVERB, HOLD], rate))]);
                h.extend([Step::Param("model", MODAL), Step::On(33, 120, 0), Step::Run(span(&[LINES, HOLD], rate))]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::On(34, 100, 0),
                    Step::Run(span(&[LINES, HOLD], rate)),
                    Step::Param("model", STRINGS),
                    Step::On(41, 100, 0),
                    Step::Run(span(&[REVERB, HOLD], rate)),
                ]
            },
            reached: |p| obs(p).model == 0 && obs(p).gate,
            completed: |p| obs(p).model == 2,
        },
        // The generators: the part's generator (the exciters' noise and particles) and the
        // shapers' drawn from by notes; the continuation draws from them again.
        W {
            boundary: Boundary::Entry("generators"),
            history: |rate| {
                let mut h = setup();
                h.extend(shaper_on());
                h.extend([Step::Param("mallet", 100.0), Step::Param("flow", 90.0), Step::On(60, 100, 0), Step::Run(span(&[VISIT], rate))]);
                h
            },
            continuation: |rate| vec![Step::On(62, 100, 0), Step::Run(span(&[VISIT], rate)), Step::On(63, 100, 0), Step::Run(span(&[VISIT], rate))],
            reached: |p| obs(p).gate,
            completed: |p| obs(p).gate,
        },
        // The transport anchor and the controllers: the transport plays (the synced LFO counts
        // from its anchor), the rack macros, the wheel, channel and poly pressure and the bend move
        // matrix rows; the continuation plays on, jumps the tempo (a new anchor), stops, resets the
        // controllers and runs the shaped release to idle.
        W {
            boundary: Boundary::Entry("transport and controllers"),
            history: |rate| {
                let mut h = setup();
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
            reached: |p| obs(p).anchored && obs(p).keys == 1,
            completed: |p| !obs(p).anchored && idle(p),
        },
        // The meter after the transport stops: it played in 3/4 and was told it stopped; the
        // continuation carries on with no transport, changes a parameter (the settings are worked
        // out again from the bar it holds), and runs the shaped release to idle.
        W {
            boundary: Boundary::Entry("stopped transport"),
            history: |rate| {
                let mut h = setup();
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
            reached: |p| !obs(p).anchored && obs(p).keys == 1,
            completed: idle,
        },
        // A key struck again before its first note's off (two open note-ons): the continuation's
        // first off closes the older one and keeps the key down; the second releases it.
        W {
            boundary: Boundary::Entry("re-strike"),
            history: |rate| {
                let mut h = setup();
                h.extend(shaper_on());
                h.extend([Step::On(60, 100, 0), Step::Run(span(&[HOLD], rate)), Step::On(60, 80, 5), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| vec![Step::Off(60, 0), Step::Run(span(&[HOLD], rate)), Step::Off(60, 3), run_out(&[SHAPER_RELEASE], rate)],
            reached: |p| obs(p).keys == 1,
            completed: idle,
        },
        // All Sound Off and All Notes Off queued while a note sounds; the continuation delivers
        // them (the part, the shapers and the converter back to power-on; the keys cleared), then
        // plays again.
        W {
            boundary: Boundary::Entry("all sound off"),
            history: |rate| {
                let mut h = setup();
                h.extend(shaper_on());
                h.extend([Step::Param("model", STRING), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::Midi([0xB0, 120, 0], 300), Step::Midi([0xB0, 123, 0], 400), Step::Run(100)]);
                h
            },
            continuation: |rate| {
                vec![Step::Run(600), Step::On(55, 100, 0), Step::Run(span(&[HOLD], rate)), Step::Off(55, 0), run_out(&[SHAPER_RELEASE], rate)]
            },
            reached: |p| obs(p).pending == 2,
            completed: idle,
        },
        // The reverb frozen (Space from 175 %): its memory recirculates; the continuation keeps it
        // frozen for its span, then thaws it.
        W {
            boundary: Boundary::Entry("frozen reverb"),
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("space", 120.0), Step::Param("model", STRINGS), Step::On(45, 120, 0), Step::Run(span(&[HOLD, HOLD], rate))]);
                h.extend([Step::Off(45, 0), Step::Param("space", 190.0), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| vec![Step::Run(span(&[REVERB], rate)), Step::Param("space", 120.0), Step::Run(span(&[HOLD], rate))],
            reached: |p| obs(p).keys == 0,
            completed: |p| obs(p).keys == 0,
        },
    ];
    // The chord strings' hysteresis: Geometry picked chord 4 before T; the continuation lowers it
    // to where the chord the voice keeps (4) and a fresh voice's (0) round to different chords,
    // then plays on and releases.
    v.push(W {
        boundary: Boundary::Entry("chord hysteresis"),
        history: |rate| {
            let mut h = setup();
            h.extend([Step::Param("model", STRINGS), Step::Param("geometry", 45.0), Step::On(52, 110, 0), Step::Run(span(&[HOLD, HOLD], rate))]);
            h
        },
        continuation: |rate| vec![Step::Param("geometry", 34.5), Step::Run(span(&[LINES], rate)), Step::Off(52, 0), run_out(&[SHAPER_RELEASE], rate)],
        reached: |p| obs(p).model == 2 && obs(p).keys == 1,
        completed: idle,
    });
    v.extend(model_witnesses());
    v
}

/// The model `M` at T in the middle of a held note, with the exciter set `X` (0: strike alone, 1:
/// bow and blow alone, 2: all three).
fn model_history<const M: u8, const X: u8>(rate: f32) -> Vec<Step> {
    let mut h = setup();
    let (bow, blow, strike) = match X {
        0 => (0.0, 0.0, 90.0),
        1 => (70.0, 80.0, 0.0),
        _ => (40.0, 90.0, 80.0),
    };
    h.extend([Step::Param("bow_level", bow), Step::Param("blow_level", blow), Step::Param("strike_level", strike)]);
    h.extend([Step::Param("model", f32::from(M)), Step::On(50, 110, 5), Step::Run(span(&[HOLD, HOLD], rate))]);
    h
}

/// The geometry and brightness move, the held key is struck again (a retrigger), everything is
/// released and the part rings on for its lines' span.
fn model_continuation<const M: u8, const X: u8>(rate: f32) -> Vec<Step> {
    vec![
        Step::Run(span(&[HOLD], rate)),
        Step::Param("geometry", 90.0),
        Step::Param("brightness", 20.0),
        Step::On(57, 100, 3),
        Step::Run(span(&[LINES], rate)),
        Step::Off(57, 2),
        Step::Off(50, 5),
        Step::Run(span(&[LINES], rate)),
    ]
}

fn model_reached<const M: u8, const X: u8>(p: &ModalVoiceInstrument) -> bool {
    obs(p).model == usize::from(M) && obs(p).gate
}

fn model_completed<const M: u8, const X: u8>(p: &ModalVoiceInstrument) -> bool {
    obs(p).model == usize::from(M) && obs(p).keys == 0
}

/// One witness per model and exciter set: see [`witnesses`].
fn model_witnesses() -> Vec<W> {
    macro_rules! each {
        ($(($m:literal, $x:literal)),*) => {
            vec![$(W {
                boundary: Boundary::Entry(concat!("model ", stringify!($m), " with exciters ", stringify!($x), " at T")),
                history: model_history::<$m, $x>,
                continuation: model_continuation::<$m, $x>,
                reached: model_reached::<$m, $x>,
                completed: model_completed::<$m, $x>,
            }),*]
        };
    }
    each!((0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (2, 0), (2, 1), (2, 2), (3, 2))
}

/// A note released while the shapers' amp envelope still rings out at T.
fn release_history(rate: f32) -> Vec<Step> {
    let mut h = setup();
    h.extend(shaper_on());
    h.extend([Step::Param("model", STRING), Step::On(60, 100, 0), Step::Run(span(&[HOLD], rate))]);
    h.extend([Step::Off(60, 0), Step::Run(span(&[Span::Seconds(0.01)], rate))]);
    h
}

fn in_release(p: &ModalVoiceInstrument) -> bool {
    let o = obs(p);
    o.keys == 0 && !o.gate && !o.shaper_gate && o.amp > 0.0
}

/// The Modal Voice's entry: every checklist boundary has a witness.
pub(super) fn entry() -> Entry<ModalVoiceInstrument> {
    Entry {
        id: "modal_voice",
        make: || Box::new(ModalVoiceInstrument::new()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: vec![],
        // The part's, the voice's and the FM pair's scratch buffers, the converter's scratch.
        poison: Some(|p, k| p.poison_unobservable(k + 1)),
    }
}
