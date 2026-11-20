//! The Resonator's witness set (ADR 0029, amendment for #742, section 13; slice 3).
//!
//! The part has no gate and no idle state: a note-off does not stop a ringing voice and the part
//! rings down by its damping (`resonator.rs`, the module docs). So "release every note" and "run
//! until every voice is idle" mean here what the instrument tracks: no key held or kept by the
//! pedal, no strum waiting, and the shapers' envelopes released ([`idle`]); the part rings on and
//! its record is compared at every block of that ring-down. The spans the inventory declares, fed
//! to the one duration calculator (the part runs at 48 kHz, so a span in part samples is given in
//! seconds and scales with the case's rate):
//! - `RAMP`: the level smoother's ramp (a 20 ms time constant, settled within 1e-6);
//! - `STRING`: a string's lines (2048 plus 1024 part samples) and `REVERB`: the reverb's memory
//!   (32768 part samples);
//! - `SHAPER_RELEASE`: the shaper's amp release at the witnesses' 40 ms, to its end;
//! - `HOLD`: a short hold (a few part blocks, the strum ring drained and the note filter settled);
//! - `VISIT`: one model's visit.

use super::*;
use transit_dsp::plugins::resonator::{Observed, ResonatorInstrument};

const PART_RATE: f64 = 48_000.0;
const RAMP: Span = Span::Seconds(0.3);
const STRING: Span = Span::Seconds((2048.0 + 1024.0) / PART_RATE);
const REVERB: Span = Span::Seconds(32_768.0 / PART_RATE);
const SHAPER_RELEASE: Span = Span::Seconds(0.2);
const HOLD: Span = Span::Seconds(0.02);
const VISIT: Span = Span::Seconds(0.08);

/// The models in the firmware's order.
const MODAL: f32 = 0.0;
const SYMPATHETIC: f32 = 1.0;
const INHARMONIC: f32 = 2.0;
const FM: f32 = 3.0;
const CHORDS: f32 = 4.0;
const REVERB_MODEL: f32 = 5.0;

type W = Witness<ResonatorInstrument>;

fn obs(p: &ResonatorInstrument) -> Observed {
    p.observe()
}

/// What the instrument tracks as idle (see the module docs): no key held or pedalled, no strum
/// or event waiting, the shapers' envelopes released and silent.
fn idle(p: &ResonatorInstrument) -> bool {
    let o = obs(p);
    !o.held && !o.pedalled && o.strums == 0 && o.pending == 0 && !o.gate && o.amp == 0.0
}

fn run_out(spans: &[Span], rate: f32) -> Step {
    Step::RunBlocks(frames(spans, rate), MAX_BLOCK)
}

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

/// The witnesses' common setup: high damping, so the part rings down within the witnesses, a short
/// amp release (the shapers' envelopes run whether or not the shaper shapes the output), and
/// `voices` voices (1, 2 or 4: the polyphony choices 0, 1, 2).
fn setup(voices: usize) -> Vec<Step> {
    let choice = match voices {
        1 => 0.0,
        2 => 1.0,
        _ => 2.0,
    };
    vec![Step::Param("damping", 10.0), Step::Param("amp_release", 40.0), Step::Param("polyphony", choice)]
}

/// Boundary 1's obligation (first blocks with strums and parameter changes), then every note
/// released and the instrument run to idle.
fn first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::Param("model", SYMPATHETIC),
        Step::On(60, 100, 3),
        Step::On(67, 90, 29),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("structure", 80.0),
        Step::Param("level", -6.0),
        Step::Run(span(&[HOLD], rate)),
        Step::Off(60, 5),
        Step::Off(67, 11),
        run_out(&[RAMP, SHAPER_RELEASE], rate),
    ]
}

/// One visit of `model`: a strum held, then released, for `hold` (its lines' span at least).
fn visit(model: f32, key: u8, hold: Span, rate: f32) -> Vec<Step> {
    vec![Step::Param("model", model), Step::On(key, 110, 7), Step::Run(span(&[hold], rate)), Step::Off(key, 3), Step::Run(span(&[HOLD], rate))]
}

/// The span a model's visit holds its strum for: its lines' span where it has lines.
fn hold_of(model: f32) -> Span {
    if model == REVERB_MODEL {
        REVERB
    } else if model == FM || model == MODAL {
        VISIT
    } else {
        STRING
    }
}

/// The shaper on (short envelopes, a resonant filter with key tracking and envelope, both LFOs
/// free-running: one with a fade-in, one synced to a bar division) and matrix rows from LFO 1 to
/// the cutoff, from LFO 2 to Brightness and from the strum's random value to Damping.
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
/// the key) to the part's macros, Pitch, Pan and Level.
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
            continuation: |rate| [setup(4), first_blocks(rate)].concat(),
            reached: |p| obs(p).fresh && obs(p).dirty,
            completed: idle,
        },
        // 2: after a reset in the middle of ringing strums and a ramp.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = setup(4);
                h.extend([Step::Param("model", INHARMONIC), Step::On(48, 120, 0), Step::On(55, 80, 40), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::Param("level", -20.0), Step::Run(97), Step::Reset]);
                h
            },
            continuation: |rate| first_blocks(rate),
            reached: |p| obs(p).fresh && obs(p).strums == 0 && obs(p).pending == 0,
            completed: idle,
        },
        // 3: every voice sounding (four voices, five strums: the fifth takes the first voice's
        // place in the rotation); the continuation strums twice more, then releases all and runs
        // to idle (module docs).
        W {
            boundary: Boundary::VoicesStolen,
            history: |rate| {
                let mut h = setup(4);
                h.push(Step::Param("model", MODAL));
                for (i, key) in [48u8, 52, 55, 59, 62].into_iter().enumerate() {
                    h.push(Step::On(key, 70 + 10 * i as u8, 0));
                    h.push(Step::Run(span(&[HOLD], rate)));
                }
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::On(64, 100, 2), Step::Run(37), Step::On(67, 100, 0), Step::Run(span(&[HOLD], rate))];
                for (k, key) in [48u8, 52, 55, 59, 62, 64, 67].into_iter().enumerate() {
                    c.push(Step::Off(key, k));
                }
                c.push(run_out(&[SHAPER_RELEASE], rate));
                c
            },
            reached: |p| obs(p).polyphony == 4 && obs(p).active_voice == 1 && obs(p).held,
            completed: idle,
        },
        // 4: inside the shapers' release (no key held, the amp envelope falling); the restored
        // release runs to its end without a new strum.
        W {
            boundary: Boundary::ReleaseTail,
            history: release_history,
            continuation: |rate| vec![run_out(&[SHAPER_RELEASE], rate)],
            reached: in_release,
            completed: idle,
        },
        // 4, the separate branch: struck again during the release, released again, run to idle.
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
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::On(57, 100, 0), Step::Run(span(&[HOLD], rate)), Step::Param("level", -30.0), Step::RunBlocks(span(&[HOLD], rate) / 2, 13)]);
                h
            },
            continuation: |rate| vec![Step::RunBlocks(29, 7), Step::Param("level", 3.0), Step::Run(frames(&[RAMP], rate))],
            reached: |p| obs(p).ramping,
            completed: |p| !obs(p).ramping,
        },
        // 6: after a model switch with every other model's state left non-default: each model
        // visited at each polyphony (the strings, pluckers and modal resonators of every voice
        // count filled, the reverb's memory filled); at T the modal model is configured at one
        // voice, so the strings, pluckers and three modal resonators are dormant. The continuation
        // revisits every model at every polyphony, each for its lines' span.
        W {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                let mut h = shaper_on();
                h.push(Step::Param("damping", 10.0));
                for (n, voices) in [4usize, 2, 1].into_iter().enumerate() {
                    h.extend(setup(voices));
                    for m in 0..6u8 {
                        h.extend(visit(f32::from(m), 40 + m + 7 * n as u8, hold_of(f32::from(m)), rate));
                    }
                }
                h.extend([Step::Param("model", MODAL), Step::On(52, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::Off(52, 0)];
                for (n, voices) in [1usize, 4, 2].into_iter().enumerate() {
                    c.extend(setup(voices));
                    for m in (0..6u8).rev() {
                        c.extend(visit(f32::from(m), 70 - m - 7 * n as u8, hold_of(f32::from(m)), rate));
                    }
                }
                c.push(run_out(&[SHAPER_RELEASE], rate));
                c
            },
            reached: |p| obs(p).model == transit_resonator::ResonatorModel::Modal && obs(p).polyphony == 1 && !obs(p).dirty,
            completed: |p| obs(p).model == transit_resonator::ResonatorModel::Modal && obs(p).polyphony == 2 && idle(p),
        },
        // 7: the flush boundary: a loud strum at the lowest damping and darkest brightness, then
        // silence long enough for the part's tail to fall into the range its flush-to-zero
        // arithmetic flushes; the continuation runs on, then plays a small input (a soft strum)
        // and runs it out.
        W {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = setup(4);
                h.extend([Step::Param("damping", 0.0), Step::Param("brightness", 0.0), Step::Param("model", MODAL), Step::On(36, 127, 0)]);
                h.extend([Step::Run(span(&[HOLD], rate)), Step::Off(36, 0), run_out(&[Span::Seconds(3.0)], rate)]);
                h
            },
            continuation: |rate| vec![Step::Run(997), Step::On(72, 1, 5), Step::Run(span(&[HOLD], rate)), Step::Off(72, 0), run_out(&[SHAPER_RELEASE], rate)],
            reached: idle,
            completed: idle,
        },
        // 8: inside the part's 24-frame block (frames rendered ahead of the converter's output);
        // the continuation crosses part-block edges with odd block lengths, events at odd offsets
        // included.
        W {
            boundary: Boundary::InternalBlock,
            history: |rate| {
                let mut h = setup(4);
                h.extend([Step::Param("model", FM), Step::On(62, 100, 0), Step::RunBlocks(span(&[HOLD], rate) / 24 * 24 + 5, 24)]);
                h
            },
            continuation: |_| vec![Step::RunBlocks(21, 7), Step::On(66, 90, 3), Step::RunBlocks(35, 5), Step::Off(62, 1), Step::RunBlocks(33, 11)],
            reached: |p| (1..24).contains(&obs(p).ahead),
            completed: |p| obs(p).ahead > 0,
        },
        // 9: events pending past the block (a note-on, a controller, a pitch bend, a note-off), and
        // a chord's strums waiting for their part blocks; the continuation delivers every one, runs
        // past the latest due frame, plus one block.
        W {
            boundary: Boundary::Pending,
            history: |rate| {
                let mut h = setup(4);
                h.extend([Step::Param("model", CHORDS), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate))]);
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
                h.extend(setup(4));
                h.extend(first_blocks(other));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).fresh && obs(p).dirty,
            completed: idle,
        },
        // 11: a key released while the sustain pedal stays down, beside a held key (the pedal and
        // the held key keep the shapers gated); the continuation releases the held key too while
        // the pedal is down, then releases the pedal and runs to idle.
        W {
            boundary: Boundary::Sustain,
            history: |rate| {
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::Param("model", SYMPATHETIC), Step::Midi([0xB0, 64, 127], 0), Step::On(60, 100, 2), Step::On(64, 90, 9)]);
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
            reached: |p| obs(p).sustain && obs(p).pedalled && obs(p).held && obs(p).gate,
            completed: |p| !obs(p).sustain && idle(p),
        },
        // The delay lines: the strings (sympathetic strings, every string of four voices) and the
        // reverb's memory filled; the continuation reads every position of each (its span).
        W {
            boundary: Boundary::Entry("delay lines"),
            history: |rate| {
                let mut h = setup(4);
                h.extend([Step::Param("model", REVERB_MODEL), Step::On(40, 120, 0), Step::Run(span(&[REVERB, HOLD], rate))]);
                h.extend([Step::Param("model", SYMPATHETIC), Step::On(28, 120, 0), Step::Run(span(&[STRING, HOLD], rate))]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::On(29, 100, 0),
                    Step::Run(span(&[STRING, HOLD], rate)),
                    Step::Param("model", REVERB_MODEL),
                    Step::On(41, 100, 0),
                    Step::Run(span(&[REVERB, HOLD], rate)),
                ]
            },
            reached: |p| obs(p).model == transit_resonator::ResonatorModel::SympatheticStrings && !obs(p).dirty,
            completed: |p| obs(p).model == transit_resonator::ResonatorModel::StringAndReverb,
        },
        // The generators: the part's generator (the pluckers' noise bursts) and the shapers'
        // drawn from by strums; the continuation draws from them again.
        W {
            boundary: Boundary::Entry("generators"),
            history: |rate| {
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::Param("model", INHARMONIC), Step::On(60, 100, 0), Step::On(61, 100, 0), Step::Run(span(&[VISIT], rate))]);
                h
            },
            continuation: |rate| vec![Step::On(62, 100, 0), Step::Run(span(&[VISIT], rate)), Step::On(63, 100, 0), Step::Run(span(&[VISIT], rate))],
            reached: |p| obs(p).held && obs(p).strums == 0,
            completed: |p| obs(p).held,
        },
        // The transport anchor and the controllers: the transport plays (the synced LFO counts
        // from its anchor), the rack macros, the wheel, channel and poly pressure and the bend move
        // matrix rows; the continuation plays on, jumps the tempo (a new anchor), stops, resets the
        // controllers and runs the shaped release to idle.
        W {
            boundary: Boundary::Entry("transport and controllers"),
            history: |rate| {
                let mut h = setup(4);
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
            reached: |p| obs(p).anchored && obs(p).held,
            completed: |p| !obs(p).anchored && idle(p),
        },
        // The meter after the transport stops: it played in 3/4 and was told it stopped; the
        // continuation carries on with no transport, changes a parameter (the settings are worked
        // out again from the bar it holds), and runs the shaped release to idle.
        W {
            boundary: Boundary::Entry("stopped transport"),
            history: |rate| {
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::Transport(Some(140.0)), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate)), Step::Transport(None)]);
                h
            },
            continuation: |rate| {
                vec![Step::Run(span(&[HOLD], rate)), Step::Param("lfo2_amount", 70.0), Step::Run(span(&[HOLD], rate)), Step::Off(50, 0), run_out(&[SHAPER_RELEASE], rate)]
            },
            reached: |p| !obs(p).anchored && obs(p).held,
            completed: idle,
        },
        // A key struck again before its first note's off (two open note-ons): the continuation's
        // first off closes the older one and keeps the key held; the second releases it.
        W {
            boundary: Boundary::Entry("re-strike"),
            history: |rate| {
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::On(60, 100, 0), Step::Run(span(&[HOLD], rate)), Step::On(60, 80, 5), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| vec![Step::Off(60, 0), Step::Run(span(&[HOLD], rate)), Step::Off(60, 3), run_out(&[SHAPER_RELEASE], rate)],
            reached: |p| obs(p).held,
            completed: idle,
        },
        // All Sound Off and All Notes Off: queued with strums waiting; the continuation delivers
        // them (the part, the shapers and the converter back to power-on; the keys cleared), then
        // strums again.
        W {
            boundary: Boundary::Entry("all sound off"),
            history: |rate| {
                let mut h = setup(4);
                h.extend(shaper_on());
                h.extend([Step::Param("model", CHORDS), Step::On(50, 100, 0), Step::Run(span(&[HOLD], rate))]);
                h.extend([Step::Midi([0xB0, 120, 0], 300), Step::Midi([0xB0, 123, 0], 400), Step::Run(100)]);
                h
            },
            continuation: |rate| vec![Step::Run(600), Step::On(55, 100, 0), Step::Run(span(&[HOLD], rate)), Step::Off(55, 0), run_out(&[SHAPER_RELEASE], rate)],
            reached: |p| obs(p).pending == 2,
            completed: idle,
        },
    ];
    v.extend(model_witnesses());
    v
}

/// The model `M` at T at `voices` voices, in the middle of a held strum (the part's state of that
/// model as it plays).
fn model_history<const M: u8, const VOICES: usize>(rate: f32) -> Vec<Step> {
    let mut h = setup(VOICES);
    h.extend([Step::Param("model", f32::from(M)), Step::On(50, 110, 5), Step::On(57, 90, 40), Step::Run(span(&[HOLD, HOLD], rate))]);
    h
}

/// The structure and brightness move, the held key is struck again, everything is released and
/// the part rings on for its lines' span.
fn model_continuation<const M: u8, const VOICES: usize>(rate: f32) -> Vec<Step> {
    vec![
        Step::Run(span(&[HOLD], rate)),
        Step::Param("structure", 90.0),
        Step::Param("brightness", 20.0),
        Step::On(50, 100, 3),
        Step::Run(span(&[hold_of(f32::from(M))], rate)),
        Step::Off(50, 2),
        Step::Off(50, 5),
        Step::Off(57, 7),
        Step::Run(span(&[HOLD], rate)),
    ]
}

fn model_reached<const M: u8, const VOICES: usize>(p: &ResonatorInstrument) -> bool {
    let o = obs(p);
    o.model == transit_resonator::ResonatorModel::ALL[usize::from(M)] && o.polyphony == VOICES && !o.dirty && o.held
}

fn model_completed<const M: u8, const VOICES: usize>(p: &ResonatorInstrument) -> bool {
    let o = obs(p);
    o.model == transit_resonator::ResonatorModel::ALL[usize::from(M)] && !o.held
}

/// One witness per model at one and four voices (and the string models at two): see
/// [`witnesses`].
fn model_witnesses() -> Vec<W> {
    macro_rules! each {
        ($(($m:literal, $v:literal)),*) => {
            vec![$(W {
                boundary: Boundary::Entry(concat!("model ", stringify!($m), " at ", stringify!($v), " voices at T")),
                history: model_history::<$m, $v>,
                continuation: model_continuation::<$m, $v>,
                reached: model_reached::<$m, $v>,
                completed: model_completed::<$m, $v>,
            }),*]
        };
    }
    each!((0, 1), (0, 4), (1, 1), (1, 2), (1, 4), (2, 1), (2, 4), (3, 1), (3, 4), (4, 1), (4, 2), (4, 4), (5, 1), (5, 4))
}

/// Two strums released while the shapers' amp envelope still rings out at T.
fn release_history(rate: f32) -> Vec<Step> {
    let mut h = setup(4);
    h.extend(shaper_on());
    h.extend([Step::Param("model", INHARMONIC), Step::On(60, 100, 0), Step::On(63, 100, 0), Step::Run(span(&[HOLD], rate))]);
    h.extend([Step::Off(60, 0), Step::Off(63, 0), Step::Run(span(&[Span::Seconds(0.01)], rate))]);
    h
}

fn in_release(p: &ResonatorInstrument) -> bool {
    let o = obs(p);
    !o.held && !o.gate && o.amp > 0.0
}

/// The Resonator's entry: every checklist boundary has a witness.
pub(super) fn entry() -> Entry<ResonatorInstrument> {
    Entry {
        id: "resonator",
        make: || Box::new(ResonatorInstrument::new()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: vec![],
        // The part's block buffers, the dormant state outside its read set, the converter's scratch.
        poison: Some(|p, k| p.poison_unobservable(k + 1)),
    }
}
