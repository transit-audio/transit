//! The built-in MIDI effects' witness sets (ADR 0029, amendment for #742, section 13; slice 5):
//! Transpose, Velocity and the Random Sequencer, on the `MidiEffect` seam.
//!
//! The same transplant case as the plug-ins' (A reaches T, a fresh B restores A's record, consumes
//! every word and re-records the same words, then both run the same continuation), with a MIDI
//! effect's output compared: every block's emitted events with their offsets, bit for bit, and the
//! refused counts. Every block the runner gives each instance the transport and the sample rate
//! before `process`, as the host's contract says (the mix's MIDI effect host does, for every stage,
//! at the start of each block). A MIDI effect has no `prepare`: the configurations are the three
//! sample rates the host gives (no channel count). The checklist is the plug-ins': each boundary has
//! a witness or a code-backed explanation.

use super::{Boundary, CHECKLIST, Span};
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::midi_fx::{Builtin, MidiOut, TimedEvent};
use transit_state_record::{Reader, Writer};

const RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
/// The host block the runner uses (frames), and the output capacity.
const BLOCK: u32 = 256;
const OUT: usize = 512;

/// One step of a history or a continuation.
#[derive(Clone, Debug)]
pub(super) enum MStep {
    /// `blocks` blocks of `frames` frames, the transport advancing.
    Run(usize, u32),
    /// Events (offset, channel, note id, message) into the next block.
    Events(Vec<(u32, u8, i32, Message)>),
    Param(&'static str, f32),
    Reset,
    /// The transport plays at this tempo from now on, or stops.
    Transport(Option<f64>),
    /// The transport jumps to this sample position (a locate).
    Locate(i64),
    /// The host's sample rate from now on.
    Rate(f64),
}

/// The transport and rate a script runs at.
#[derive(Clone, Copy)]
struct Clock {
    samples: i64,
    tempo: Option<f64>,
    rate: f64,
}

impl Clock {
    fn transport(&self) -> transit_dsp::Transport {
        let tempo = self.tempo.unwrap_or(120.0);
        let seconds = self.samples as f64 / self.rate;
        let quarters = seconds * tempo / 60.0;
        transit_dsp::Transport {
            playing: self.tempo.is_some(),
            recording: false,
            tempo,
            numerator: 4,
            denominator: 4,
            samples: self.samples,
            seconds,
            quarters,
            bar_start: (quarters / 4.0).floor() * 4.0,
            bar: (quarters / 4.0).floor() as i64,
        }
    }
}

/// A witness for a MIDI effect.
pub(super) struct MWitness {
    pub(super) boundary: Boundary,
    pub(super) history: fn(f64) -> Vec<MStep>,
    pub(super) continuation: fn(f64) -> Vec<MStep>,
    pub(super) reached: fn(&Builtin) -> bool,
    pub(super) completed: fn(&Builtin) -> bool,
}

/// A MIDI effect entry.
pub(super) struct MEntry {
    pub(super) id: &'static str,
    pub(super) witnesses: Vec<MWitness>,
    pub(super) inapplicable: Vec<(Boundary, &'static str)>,
    pub(super) describe: fn(&Builtin) -> String,
    pub(super) poison: Option<fn(&mut Builtin, u32) -> u64>,
}

#[derive(Default)]
struct Words(Vec<u64>);

impl std::hash::Hasher for Words {
    fn write(&mut self, _: &[u8]) {
        panic!("the record writer emits words only");
    }
    fn write_u64(&mut self, w: u64) {
        self.0.push(w);
    }
    fn finish(&self) -> u64 {
        0
    }
}

fn record(b: &Builtin) -> Vec<u64> {
    let mut sink = Words::default();
    b.effect_ref().fingerprint(&mut Writer::new(&mut sink));
    sink.0
}

/// What one block emitted: the events with their offsets, and the refused counts.
type Emitted = (Vec<TimedEvent>, u32, u32);

/// Runs `steps` on every instance alike; after every block `each` gets the instances and what
/// each emitted. Returns the blocks run.
fn run(
    fx: &mut [Builtin],
    steps: &[MStep],
    clock: &mut Clock,
    mut each: impl FnMut(&mut [Builtin], &[Emitted]) -> Result<(), String>,
) -> Result<usize, String> {
    let mut pending: Vec<TimedEvent> = Vec::new();
    let mut blocks = 0;
    for step in steps {
        match step {
            MStep::Run(n, frames) => {
                for _ in 0..*n {
                    let input: Vec<TimedEvent> = pending.iter().copied().filter(|e| e.offset < *frames).collect();
                    pending.retain(|e| e.offset >= *frames);
                    for e in &mut pending {
                        e.offset -= frames;
                    }
                    let mut outs = Vec::new();
                    for f in fx.iter_mut() {
                        let e = f.effect();
                        e.set_transport(&clock.transport());
                        e.set_sample_rate(clock.rate);
                        let mut buf = [TimedEvent::EMPTY; OUT];
                        let mut out = MidiOut::new(&mut buf);
                        e.process(&input, &mut out, *frames);
                        outs.push((out.events().to_vec(), out.refused(), out.refused_ons()));
                    }
                    if clock.tempo.is_some() {
                        clock.samples += i64::from(*frames);
                    }
                    blocks += 1;
                    each(fx, &outs)?;
                }
            }
            MStep::Events(list) => {
                for &(offset, channel, note_id, message) in list {
                    pending.push(TimedEvent { offset, event: PluginEvent { channel, note_id, message } });
                }
                pending.sort_by_key(|e| e.offset);
            }
            MStep::Param(id, v) => {
                for f in fx.iter_mut() {
                    assert!(f.effect().set_param(id, *v), "unknown parameter {id}");
                }
            }
            MStep::Reset => fx.iter_mut().for_each(|f| f.effect().reset()),
            MStep::Transport(t) => clock.tempo = *t,
            MStep::Locate(s) => clock.samples = *s,
            MStep::Rate(r) => clock.rate = *r,
        }
    }
    Ok(blocks)
}

fn case(e: &MEntry, w: &MWitness, rate: f64) -> Result<(usize, usize, usize), String> {
    let at = |what: String| format!("{} {:?} at {rate} Hz: {what}", e.id, w.boundary);
    let mut clock = Clock { samples: 0, tempo: None, rate };
    let mut one = vec![Builtin::new(e.id).ok_or_else(|| at("no such effect".into()))?];
    let hist = run(&mut one, &(w.history)(rate), &mut clock, |_, _| Ok(())).map_err(at)?;
    let a = one.pop().ok_or_else(|| at("no instance".into()))?;
    if !(w.reached)(&a) {
        return Err(at(format!("the history did not reach the boundary ({})", (e.describe)(&a))));
    }
    let rec = record(&a);
    let mut b = Builtin::new(e.id).ok_or_else(|| at("no such effect".into()))?;
    let mut r = Reader::new(&rec);
    if !b.effect().restore_print(&mut r) || !r.finish() {
        return Err(at("the restore refused the record or left words unread".into()));
    }
    if record(&b) != rec {
        return Err(at("the restored instance re-records other words".into()));
    }
    if let Some(poison) = e.poison
        && poison(&mut b, 0) == 0
    {
        return Err(at("the poison witness wrote nothing".into()));
    }
    let mut k = 0;
    let mut events = 0;
    let mut both = vec![a, b];
    let cont = run(&mut both, &(w.continuation)(rate), &mut clock, |fx, outs| {
        assert!(fx[0].effect_ref().print_complete() && fx[1].effect_ref().print_complete(), "the declaration changed");
        if outs[0] != outs[1] {
            return Err(format!("block {k}: the emitted events differ ({:?} against {:?})", outs[0], outs[1]));
        }
        events += outs[0].0.len();
        if record(&fx[0]) != record(&fx[1]) {
            return Err(format!("block {k}: the records differ"));
        }
        k += 1;
        if let Some(poison) = e.poison {
            poison(&mut fx[1], k as u32);
        }
        Ok(())
    })
    .map_err(at)?;
    if !(w.completed)(&both[0]) {
        return Err(at(format!("the continuation did not complete its obligation ({})", (e.describe)(&both[0]))));
    }
    Ok((hist, cont, events))
}

/// Every witness of `e` at every rate; the first failure, else the report lines.
pub(super) fn harness(e: &MEntry) -> Result<Vec<String>, String> {
    for b in CHECKLIST {
        let witnessed = e.witnesses.iter().any(|w| w.boundary == b);
        let explained = e.inapplicable.iter().any(|(x, why)| *x == b && !why.is_empty());
        if witnessed == explained {
            return Err(format!("{}: boundary {b:?} needs exactly one of a witness or a code-backed explanation", e.id));
        }
    }
    let mut lines = Vec::new();
    for w in &e.witnesses {
        let (mut h, mut c, mut n) = (0, 0, 0);
        for rate in RATES {
            let (hh, cc, nn) = case(e, w, rate)?;
            (h, c, n) = (h + hh, c + cc, n + nn);
        }
        lines.push(format!("{} {:?}: history {h} blocks, continuation {c} blocks, {n} events emitted", e.id, w.boundary));
        println!("{}", lines.last().map_or("", String::as_str));
    }
    Ok(lines)
}

// ---- Scripts ----

fn on(offset: u32, voice: i32, key: u8, velocity: u8) -> (u32, u8, i32, Message) {
    (offset, 0, voice, Message::NoteOn { key, velocity, id: NoteId::NONE })
}

fn off(offset: u32, voice: i32, key: u8) -> (u32, u8, i32, Message) {
    (offset, 0, voice, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })
}

fn pressure(offset: u32, voice: i32, key: u8, value: u8) -> (u32, u8, i32, Message) {
    (offset, 0, voice, Message::PolyPressure { key, value, id: NoteId::NONE })
}

/// Blocks covering `span` at `rate`, in `BLOCK`-frame blocks, plus one.
fn blocks(span: Span, rate: f64) -> usize {
    let frames = match span {
        Span::Seconds(t) => (t * rate).ceil() as usize,
        Span::Samples(n) => n,
    };
    frames.div_ceil(BLOCK as usize) + 1
}

fn idle_events(n: usize) -> MStep {
    MStep::Run(n, BLOCK)
}

// ---- Transpose ----

fn transpose_remembered(b: &Builtin) -> usize {
    match b {
        Builtin::Transpose(t) => t.remembered(),
        _ => 0,
    }
}

fn transpose() -> MEntry {
    MEntry {
        id: "midi_transpose",
        describe: |b| format!("{} notes remembered", transpose_remembered(b)),
        witnesses: vec![
            // 1: fresh, before any process: the first blocks with notes and a parameter change.
            MWitness {
                boundary: Boundary::AfterPrepare,
                history: |_| vec![],
                continuation: |_| {
                    vec![
                        MStep::Param("semitones", 7.0),
                        MStep::Events(vec![on(3, 1, 60, 100), on(9, -1, 64, 90)]),
                        idle_events(2),
                        MStep::Param("semitones", -5.0),
                        MStep::Events(vec![off(5, 1, 60), off(7, -1, 64)]),
                        idle_events(2),
                    ]
                },
                reached: |b| transpose_remembered(b) == 0,
                completed: |b| transpose_remembered(b) == 0,
            },
            // 2: after a reset with notes remembered: their releases pass unchanged (unknown).
            MWitness {
                boundary: Boundary::AfterReset,
                history: |_| {
                    vec![MStep::Param("semitones", 12.0), MStep::Events(vec![on(0, 2, 50, 80), on(4, -1, 52, 80)]), idle_events(1), MStep::Reset]
                },
                continuation: |_| {
                    vec![
                        MStep::Events(vec![off(1, 2, 50), off(2, -1, 52), on(3, 3, 55, 70)]),
                        idle_events(1),
                        MStep::Events(vec![off(0, 3, 55)]),
                        idle_events(1),
                    ]
                },
                reached: |b| transpose_remembered(b) == 0,
                completed: |b| transpose_remembered(b) == 0,
            },
            // 6: the shift a sounding note got, kept across a parameter change (each note keeps its
            // own shift; a note shifted out of range drops its off too); the continuation releases
            // every note and sends poly pressure to them first.
            MWitness {
                boundary: Boundary::ModeSwitch,
                history: |_| {
                    vec![
                        MStep::Param("semitones", 24.0),
                        MStep::Events(vec![on(0, 1, 60, 100), on(1, -1, 110, 100)]),
                        idle_events(1),
                        MStep::Param("semitones", -36.0),
                        MStep::Events(vec![on(2, 2, 62, 100), on(3, -1, 20, 90)]),
                        idle_events(1),
                        MStep::Param("semitones", 3.0),
                    ]
                },
                continuation: |_| {
                    vec![
                        MStep::Events(vec![pressure(0, 1, 60, 40), pressure(1, 2, 62, 50), pressure(2, -1, 20, 60)]),
                        idle_events(1),
                        MStep::Events(vec![off(0, 1, 60), off(1, -1, 110), off(2, 2, 62), off(3, -1, 20)]),
                        idle_events(1),
                    ]
                },
                reached: |b| transpose_remembered(b) == 4,
                completed: |b| transpose_remembered(b) == 0,
            },
            // The note table: a re-strike of a remembered note replaces its entry; notes on other
            // channels and voice ids stay apart; then everything is released.
            MWitness {
                boundary: Boundary::Entry("note table"),
                history: |_| {
                    let mut ev = Vec::new();
                    for k in 0..40u8 {
                        ev.push((
                            u32::from(k),
                            k % 3,
                            if k % 2 == 0 { i32::from(k) } else { -1 },
                            Message::NoteOn { key: 30 + k, velocity: 90, id: NoteId::NONE },
                        ));
                    }
                    vec![
                        MStep::Param("semitones", 5.0),
                        MStep::Events(ev),
                        idle_events(1),
                        MStep::Events(vec![(0, 1, 4, Message::NoteOff { key: 34, velocity: 0, id: NoteId::NONE })]),
                        idle_events(1),
                    ]
                },
                continuation: |_| {
                    let mut ev = vec![(0, 1, -1, Message::NoteOn { key: 31, velocity: 70, id: NoteId::NONE })];
                    for k in 0..40u8 {
                        ev.push((
                            1 + u32::from(k),
                            k % 3,
                            if k % 2 == 0 { i32::from(k) } else { -1 },
                            Message::NoteOff { key: 30 + k, velocity: 0, id: NoteId::NONE },
                        ));
                    }
                    vec![MStep::Param("semitones", -2.0), MStep::Events(ev), idle_events(1)]
                },
                reached: |b| transpose_remembered(b) == 39,
                completed: |b| transpose_remembered(b) == 0,
            },
        ],
        inapplicable: vec![
            (Boundary::SecondPrepare, "no prepare and no rate: `Transpose` ignores `set_sample_rate` (the trait default)"),
            (Boundary::VoicesStolen, "no voices to steal: every note-on takes its own table entry (the note table witness fills 40 of them)"),
            (Boundary::ReleaseTail, "no release tails: an event goes out in the block it came in"),
            (Boundary::MidRamp, "no ramps: `set_param` applies at once"),
            (Boundary::Flush, "no feedback or decay: the effect rewrites events"),
            (Boundary::InternalBlock, "no internal block: `process` works event by event"),
            (Boundary::Pending, "nothing pending: every event goes out at its offset in the block it came in"),
            (Boundary::Sustain, "no sustain handling: controllers pass unchanged"),
        ],
        poison: Some(|b, k| match b {
            Builtin::Transpose(t) => t.poison_unobservable(k + 1),
            _ => 0,
        }),
    }
}

// ---- Velocity ----

fn velocity() -> MEntry {
    MEntry {
        id: "midi_velocity",
        describe: |_| String::new(),
        witnesses: vec![
            // 1: fresh: the first blocks with notes and changes to each parameter (a minimum above
            // the maximum included).
            MWitness {
                boundary: Boundary::AfterPrepare,
                history: |_| vec![],
                continuation: |_| {
                    vec![
                        MStep::Param("scale", 150.0),
                        MStep::Param("offset", -20.0),
                        MStep::Events(vec![on(0, 1, 60, 100), on(5, 2, 61, 10)]),
                        idle_events(1),
                        MStep::Param("min", 90.0),
                        MStep::Param("max", 40.0),
                        MStep::Events(vec![on(1, 3, 62, 127), off(2, 1, 60), off(3, 2, 61)]),
                        idle_events(1),
                    ]
                },
                reached: |_| true,
                completed: |_| true,
            },
            // 2: after a reset (which keeps the parameters): notes map by them.
            MWitness {
                boundary: Boundary::AfterReset,
                history: |_| {
                    vec![
                        MStep::Param("scale", 50.0),
                        MStep::Param("offset", 30.0),
                        MStep::Param("min", 20.0),
                        MStep::Param("max", 100.0),
                        MStep::Reset,
                    ]
                },
                continuation: |_| vec![MStep::Events(vec![on(0, 1, 60, 1), on(1, 2, 62, 64), on(2, 3, 64, 127)]), idle_events(1)],
                reached: |_| true,
                completed: |_| true,
            },
        ],
        inapplicable: vec![
            (Boundary::SecondPrepare, "no prepare and no rate: `Velocity` ignores `set_sample_rate` (the trait default)"),
            (Boundary::VoicesStolen, "no voices: `Velocity` keeps nothing per note"),
            (Boundary::ReleaseTail, "no release tails: an event goes out in the block it came in"),
            (Boundary::MidRamp, "no ramps: `set_param` applies at once"),
            (Boundary::ModeSwitch, "no modes: one mapping, `Velocity::map`"),
            (Boundary::Flush, "no feedback or decay: the effect rewrites velocities"),
            (Boundary::InternalBlock, "no internal block: `process` works event by event"),
            (Boundary::Pending, "nothing pending: every event goes out at its offset in the block it came in"),
            (Boundary::Sustain, "no sustain handling: controllers pass unchanged"),
        ],
        poison: None,
    }
}

// ---- Random Sequencer ----

fn sequencer_obs(b: &Builtin) -> (usize, usize) {
    match b {
        Builtin::RandomSequencer(s) => s.observe(),
        _ => (0, 0),
    }
}

/// A bar at 120 BPM.
const BAR: Span = Span::Seconds(2.0);

fn sequencer_setup() -> Vec<MStep> {
    vec![
        MStep::Param("play", 4.0),
        MStep::Param("rate", 13.0),
        MStep::Param("jitter", 30.0),
        MStep::Param("length_random", 40.0),
        MStep::Param("dynamics", 60.0),
    ]
}

fn random_sequencer() -> MEntry {
    MEntry {
        id: "random_sequencer",
        describe: |b| format!("{:?} (pending, sounding)", sequencer_obs(b)),
        witnesses: vec![
            // 1: fresh: the transport starts, notes come in (swallowed, then through with Thru),
            // parameters change; the transport stops (the sounding notes end).
            MWitness {
                boundary: Boundary::AfterPrepare,
                history: |_| vec![],
                continuation: |rate| {
                    let mut c = sequencer_setup();
                    c.extend([MStep::Transport(Some(120.0)), MStep::Events(vec![on(3, 1, 60, 100)]), MStep::Run(blocks(BAR, rate), BLOCK)]);
                    c.extend([
                        MStep::Param("thru", 1.0),
                        MStep::Param("model", 2.0),
                        MStep::Events(vec![off(0, 1, 60), on(4, 2, 62, 90)]),
                        MStep::Run(blocks(BAR, rate), BLOCK),
                    ]);
                    c.extend([MStep::Transport(None), MStep::Events(vec![off(0, 2, 62)]), MStep::Run(2, BLOCK)]);
                    c
                },
                reached: |b| sequencer_obs(b) == (0, 0),
                completed: |b| sequencer_obs(b).1 == 0,
            },
            // 2: after a reset while notes sound and events are scheduled.
            MWitness {
                boundary: Boundary::AfterReset,
                history: |rate| {
                    let mut h = sequencer_setup();
                    h.extend([MStep::Transport(Some(120.0)), MStep::Run(blocks(BAR, rate), BLOCK), MStep::Reset]);
                    h
                },
                continuation: |rate| vec![MStep::Run(blocks(BAR, rate), BLOCK), MStep::Transport(None), MStep::Run(2, BLOCK)],
                reached: |_| true,
                completed: |b| sequencer_obs(b).1 == 0,
            },
            // 6: the models and the loop: each model played (the deja vu loop recorded and
            // locked), then at T a switch; the continuation visits every model again and the
            // pitch modes and ranges.
            MWitness {
                boundary: Boundary::ModeSwitch,
                history: |rate| {
                    let mut h = sequencer_setup();
                    h.extend([MStep::Param("deja_vu", 50.0), MStep::Param("loop_length", 3.0), MStep::Transport(Some(120.0))]);
                    for m in 0..6 {
                        h.extend([MStep::Param("model", m as f32), MStep::Run(blocks(BAR, rate), BLOCK)]);
                    }
                    h.push(MStep::Param("model", 1.0));
                    h
                },
                continuation: |rate| {
                    let mut c = Vec::new();
                    for m in [5.0, 4.0, 3.0, 2.0, 0.0] {
                        c.extend([MStep::Param("model", m), MStep::Run(blocks(BAR, rate), BLOCK)]);
                    }
                    for (mode, range) in [(1.0, 1.0), (2.0, 2.0), (0.0, 0.0)] {
                        c.extend([
                            MStep::Param("pitch_mode", mode),
                            MStep::Param("range", range),
                            MStep::Param("deja_vu", 90.0),
                            MStep::Run(blocks(BAR, rate), BLOCK),
                        ]);
                    }
                    c
                },
                reached: |_| true,
                completed: |_| true,
            },
            // 8: inside a tick (the jittery clock between grid ticks, gates high at T across a
            // block edge); the continuation crosses tick edges with odd block lengths.
            MWitness {
                boundary: Boundary::InternalBlock,
                history: |rate| {
                    let mut h = sequencer_setup();
                    h.extend([MStep::Transport(Some(120.0)), MStep::Run(blocks(BAR, rate), 97)]);
                    h
                },
                continuation: |rate| vec![MStep::Run(blocks(BAR, rate) * 3, 61), MStep::Run(blocks(BAR, rate), 7)],
                reached: |b| sequencer_obs(b).1 > 0,
                completed: |_| true,
            },
            // 9: events scheduled past the block (note-offs of sounding gates, the next tick's
            // notes at long gate lengths); the continuation plays them, plus a bar.
            MWitness {
                boundary: Boundary::Pending,
                history: |rate| {
                    let mut h = sequencer_setup();
                    h.extend([
                        MStep::Param("gate_length", 95.0),
                        MStep::Param("rate", 8.0),
                        MStep::Transport(Some(120.0)),
                        MStep::Run(blocks(BAR, rate), BLOCK),
                    ]);
                    h
                },
                continuation: |rate| vec![MStep::Run(blocks(BAR, rate) * 2, BLOCK)],
                reached: |b| sequencer_obs(b).0 > 0,
                completed: |_| true,
            },
            // 10: the host's rate changes (another session rate) mid-play.
            MWitness {
                boundary: Boundary::SecondPrepare,
                history: |rate| {
                    let other = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                    let mut h = vec![MStep::Rate(other)];
                    h.extend(sequencer_setup());
                    h.extend([MStep::Transport(Some(120.0)), MStep::Run(blocks(BAR, other), BLOCK), MStep::Rate(rate)]);
                    h
                },
                continuation: |rate| vec![MStep::Run(blocks(BAR, rate) * 2, BLOCK)],
                reached: |_| true,
                completed: |_| true,
            },
            // The generator and the seed: Latch off, so the sequence runs on across a stop and a
            // locate; the continuation restarts the transport elsewhere and plays on, then latches
            // a new seed.
            MWitness {
                boundary: Boundary::Entry("generator and seed"),
                history: |rate| {
                    let mut h = sequencer_setup();
                    h.extend([
                        MStep::Param("seed_latch", 0.0),
                        MStep::Param("seed", 77.0),
                        MStep::Transport(Some(120.0)),
                        MStep::Run(blocks(BAR, rate), BLOCK),
                    ]);
                    h.extend([MStep::Transport(None), MStep::Run(2, BLOCK)]);
                    h
                },
                continuation: |rate| {
                    vec![
                        MStep::Locate(1_000_000),
                        MStep::Transport(Some(100.0)),
                        MStep::Run(blocks(BAR, rate), BLOCK),
                        MStep::Param("seed_latch", 1.0),
                        MStep::Param("seed", 78.0),
                        MStep::Run(blocks(BAR, rate), BLOCK),
                    ]
                },
                reached: |b| sequencer_obs(b).1 == 0,
                completed: |_| true,
            },
            // The loop frozen: Freeze locks the recorded decisions; the continuation plays the
            // loop's whole length twice, then unfreezes.
            MWitness {
                boundary: Boundary::Entry("frozen loop"),
                history: |rate| {
                    let mut h = sequencer_setup();
                    h.extend([
                        MStep::Param("loop_length", 5.0),
                        MStep::Transport(Some(120.0)),
                        MStep::Run(blocks(BAR, rate), BLOCK),
                        MStep::Param("freeze", 1.0),
                    ]);
                    h
                },
                continuation: |rate| {
                    vec![MStep::Run(blocks(BAR, rate) * 2, BLOCK), MStep::Param("freeze", 0.0), MStep::Run(blocks(BAR, rate), BLOCK)]
                },
                reached: |_| true,
                completed: |_| true,
            },
        ],
        inapplicable: vec![
            (
                Boundary::VoicesStolen,
                "no voice allocation: each of the three gates owns its voice; a new note on a sounding voice ends the old one first (the model witnesses play all three)",
            ),
            (Boundary::ReleaseTail, "no release tails: a gate's note ends when its gate falls (scheduled, the pending witness)"),
            (
                Boundary::MidRamp,
                "no ramps the record could stop mid-way that the tick and pending witnesses do not cross: parameters apply at the next tick",
            ),
            (Boundary::Flush, "no feedback or decay: the sequencer emits events"),
            (Boundary::Sustain, "no sustain handling: incoming controllers pass unchanged"),
        ],
        poison: Some(|b, k| match b {
            Builtin::RandomSequencer(s) => s.poison_unobservable(k + 1),
            _ => 0,
        }),
    }
}

/// The three entries.
pub(super) fn entries() -> Vec<MEntry> {
    vec![transpose(), velocity(), random_sequencer()]
}
