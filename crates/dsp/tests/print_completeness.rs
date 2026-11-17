//! The transplant harness (ADR 0029, amendment for #742, section 13): executable witnesses for a
//! registry entry's declaration that its state record is complete.
//!
//! It enumerates registry entries (not Rust types: Room and Plate Reverb are one type and two
//! entries) and the built-in MIDI effects, and covers each entry whose `print_complete` is true.
//! Every case:
//! 1. A reaches the boundary T through its history (block lengths cycling through 1 to 256);
//!    B is fresh (prepared alike).
//! 2. A's record is taken losslessly; B restores it, consumes every word, and re-records exactly
//!    the same words.
//! 3. Both run the same continuation; their records are equal at every block boundary and their
//!    outputs bit-identical, and no output sample is non-finite.
//!
//! Configurations: 44.1, 48 and 96 kHz crossed with one and two channels, maximum block 256.
//! The boundaries are section 13's checklist: each applicable one is a witness with its own
//! history and continuation obligation (frames from one duration calculator fed the spans the
//! entry's inventory declares), asserts that it reached its boundary and completed its
//! obligation; each inapplicable one carries a code-backed explanation. The harness prints, per
//! entry and witness, the history and continuation frames, the `process` calls and the bytes
//! recorded.
//!
//! Permanent negative controls: a fixture entry (a smoother, a delay line, a counter, a
//! generator) and one variant per field that leaves the field out of record and restore; the
//! harness fails each variant and passes the full entry.
//!
//! The Macro Oscillator declares its record complete (slice 1, push 2 of #742): its witness set is
//! `print_completeness/macro_osc.rs`. Every other entry's declaration is false; an entry turning
//! true needs its witness set here, its inventory and its mutation record.
//!
//! A continuation runs A and B in lockstep (each block's records are compared and dropped), so a
//! large record costs time, not memory.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::hash::Hasher;
use transit_dsp::Plugin;

#[path = "print_completeness/macro_osc.rs"]
mod macro_osc;
use transit_state_record::{Config, Reader, Writer};

const RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];
const CHANNELS: [usize; 2] = [1, 2];
const MAX_BLOCK: usize = 256;

/// A sink that keeps every word: the lossless record.
#[derive(Default)]
struct Words(Vec<u64>);

impl Hasher for Words {
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

fn record<P: Plugin + ?Sized>(p: &P) -> Vec<u64> {
    let mut sink = Words::default();
    p.fingerprint(&mut Writer::new(&mut sink));
    sink.0
}

// ---- The duration calculator ----

/// A span the entry's inventory declares (a ramp, a line, a counter's period, a release).
#[derive(Clone, Copy, Debug)]
enum Span {
    Seconds(f64),
    Samples(usize),
}

/// Frames that cover every span at `rate`, plus one maximum block.
fn frames(spans: &[Span], rate: f32) -> usize {
    let n: usize = spans
        .iter()
        .map(|s| match *s {
            Span::Seconds(t) => (t * f64::from(rate)).ceil() as usize,
            Span::Samples(n) => n,
        })
        .sum();
    n + MAX_BLOCK
}

// ---- Scripts ----

/// One step of a history or a continuation.
#[derive(Clone, Debug)]
enum Step {
    /// `frames` of input, in blocks whose lengths cycle through 1 to 256 (odd lengths included).
    Run(usize),
    /// The same, in blocks of exactly `block`.
    RunBlocks(usize, usize),
    Param(&'static str, f32),
    Reset,
    Prepare(f32),
    /// A note-on (key, velocity) at an offset into the next block (an offset past its end stays
    /// pending into later blocks).
    On(u8, u8, usize),
    /// A note-off at an offset into the next block.
    Off(u8, usize),
    /// A raw MIDI message (a controller, pressure, pitch bend) at an offset into the next block.
    Midi([u8; 3], usize),
    /// The rack's macros, from now on.
    Macros([f32; 8]),
    /// The transport plays at this tempo from now on (`set_transport` before every block, at the
    /// script's frame clock), or stops (`None`).
    Transport(Option<f64>),
}

/// What a script did.
#[derive(Default, Clone, Copy)]
struct Counts {
    frames: usize,
    calls: usize,
}

/// The input of a script: deterministic and nonzero, so a line fills with history.
fn input(at: usize, c: usize) -> f32 {
    let x = ((at * 31 + c * 17) % 97) as f32 / 97.0 - 0.5;
    if at % 211 < 13 { 0.0 } else { x }
}

/// The lengths of the blocks of a run: a sequence through 1 to 256 (`seed` picks where it starts).
fn lengths(total: usize, seed: usize) -> impl Iterator<Item = usize> {
    let mut left = total;
    let mut k = seed;
    std::iter::from_fn(move || {
        if left == 0 {
            return None;
        }
        k += 1;
        let n = ((k * 37) % MAX_BLOCK + 1).min(left);
        left -= n;
        Some(n)
    })
}

/// The transport a script plays: the tempo while it plays, the rate it counts frames at.
#[derive(Clone, Copy, Default)]
struct Clock {
    frames: usize,
    tempo: Option<f64>,
    rate: f32,
}

impl Clock {
    /// The transport at the script's frame clock: 3/4 (so a bar is not the default four quarter
    /// notes), and a song position that drifts from the frame count by 1e-7 quarter notes per
    /// frame, as a host's rounded position may, so a plug-in's own count from its anchor and a
    /// fresh anchor give different positions.
    fn transport(&self) -> transit_dsp::Transport {
        let tempo = self.tempo.unwrap_or(120.0);
        let seconds = self.frames as f64 / f64::from(self.rate.max(1.0));
        let quarters = seconds * tempo / 60.0 + self.frames as f64 * 1e-7;
        transit_dsp::Transport {
            playing: self.tempo.is_some(),
            recording: false,
            tempo,
            numerator: 3,
            denominator: 4,
            samples: self.frames as i64,
            seconds,
            quarters,
            bar_start: (quarters / 3.0).floor() * 3.0,
            bar: (quarters / 3.0).floor() as i64,
        }
    }
}

/// Runs `steps` on every instance of `ps` alike (`ch` channels): one instance for a history, A
/// and B in lockstep for a continuation. After every block, `each` gets the instances and each
/// one's output, and may stop the run with a failure.
fn run<P: Plugin + ?Sized>(
    ps: &mut [Box<P>],
    ch: usize,
    steps: &[Step],
    clock: &mut Clock,
    counts: &mut Counts,
    mut each: impl FnMut(&[Box<P>], &[Vec<Vec<f32>>], usize) -> Result<(), String>,
) -> Result<(), String> {
    let mut io = vec![vec![vec![0.0f32; MAX_BLOCK]; ch]; ps.len()];
    for (s, step) in steps.iter().enumerate() {
        let blocks: Vec<usize> = match *step {
            Step::Run(n) => lengths(n, s * 7 + clock.frames).collect(),
            Step::RunBlocks(n, b) => (0..n.div_ceil(b)).map(|k| b.min(n - k * b)).collect(),
            ref other => {
                for p in ps.iter_mut() {
                    match *other {
                        Step::Param(id, v) => {
                            p.set_param(id, v);
                        }
                        Step::Reset => p.reset(),
                        Step::Prepare(rate) => p.prepare(rate, MAX_BLOCK, ch),
                        Step::On(key, vel, at) => p.note_on(at, key, vel),
                        Step::Off(key, at) => p.note_off(at, key),
                        Step::Midi(msg, at) => p.midi(at, msg),
                        Step::Macros(m) => p.set_rack_macros(&m),
                        // At once (a stop is told once: no transport goes with later blocks).
                        Step::Transport(tempo) => p.set_transport(&Clock { tempo, ..*clock }.transport()),
                        Step::Run(_) | Step::RunBlocks(..) => {}
                    }
                }
                match *other {
                    Step::Prepare(rate) => clock.rate = rate,
                    Step::Transport(t) => clock.tempo = t,
                    _ => {}
                }
                continue;
            }
        };
        for n in blocks {
            for (p, io) in ps.iter_mut().zip(io.iter_mut()) {
                for (c, b) in io.iter_mut().enumerate() {
                    for (i, x) in b.iter_mut().take(n).enumerate() {
                        *x = input(clock.frames + i, c);
                    }
                }
                if clock.tempo.is_some() {
                    p.set_transport(&clock.transport());
                }
                p.process(io, n);
                counts.calls += 1;
            }
            clock.frames += n;
            counts.frames += n;
            each(ps, &io, n)?;
        }
    }
    Ok(())
}

// ---- Witnesses ----

/// Section 13's boundary checklist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    AfterPrepare,
    AfterReset,
    VoicesStolen,
    ReleaseTail,
    MidRamp,
    ModeSwitch,
    Flush,
    InternalBlock,
    Pending,
    SecondPrepare,
    Sustain,
    /// An entry-specific boundary (a frozen recording, a delay line, a generator, a counter).
    Entry(&'static str),
}

const CHECKLIST: [Boundary; 11] = [
    Boundary::AfterPrepare,
    Boundary::AfterReset,
    Boundary::VoicesStolen,
    Boundary::ReleaseTail,
    Boundary::MidRamp,
    Boundary::ModeSwitch,
    Boundary::Flush,
    Boundary::InternalBlock,
    Boundary::Pending,
    Boundary::SecondPrepare,
    Boundary::Sustain,
];

/// One witness: its boundary, the history that reaches it and the continuation obligation (both
/// built from the case's rate through the duration calculator), and its assertions: A reached the
/// boundary at T, and A completed the obligation by the end.
struct Witness<P: ?Sized> {
    boundary: Boundary,
    history: fn(f32) -> Vec<Step>,
    continuation: fn(f32) -> Vec<Step>,
    reached: fn(&P) -> bool,
    completed: fn(&P) -> bool,
}

/// An entry the harness covers: how to make it, its witnesses, and each inapplicable checklist
/// boundary with its code-backed explanation.
struct Entry<P: ?Sized> {
    id: &'static str,
    make: fn() -> Box<P>,
    /// What a failure report shows of an instance (the witnesses' observations).
    describe: fn(&P) -> String,
    witnesses: Vec<Witness<P>>,
    inapplicable: Vec<(Boundary, &'static str)>,
}

/// One witness's actual cost, summed over its configurations.
#[derive(Default)]
struct Report {
    history: usize,
    continuation: usize,
    calls: usize,
    bytes: usize,
}

/// One case (section 13's steps 1 to 3). The continuation runs A and B in lockstep, so no record
/// is stored past the block it is compared in.
fn case<P: Plugin + ?Sized>(e: &Entry<P>, w: &Witness<P>, rate: f32, ch: usize, report: &mut Report) -> Result<(), String> {
    let at = |what: String| format!("{} {:?} at {rate} Hz, {ch} channel(s): {what}", e.id, w.boundary);
    let mut a = (e.make)();
    a.prepare(rate, MAX_BLOCK, ch);
    let mut clock = Clock { rate, ..Clock::default() };
    let mut hist = Counts::default();
    let mut one = vec![a];
    run(&mut one, ch, &(w.history)(rate), &mut clock, &mut hist, |_, _, _| Ok(())).map_err(at)?;
    let a = one.pop().ok_or_else(|| at("no instance".into()))?;
    if !(w.reached)(&a) {
        return Err(at(format!("the history did not reach the boundary ({})", (e.describe)(&a))));
    }
    let rec = record(&*a);
    report.bytes += rec.len() * 8;
    let mut b = (e.make)();
    b.prepare(rate, MAX_BLOCK, ch);
    let mut r = Reader::new(&rec);
    if !b.restore_print(&mut r) || !r.finish() {
        return Err(at("the restore refused the record or left words unread".into()));
    }
    if record(&*b) != rec {
        return Err(at("the restored instance re-records other words".into()));
    }
    let script = (w.continuation)(rate);
    let mut cont = Counts::default();
    let mut k = 0;
    let mut bytes = 0;
    let mut both = vec![a, b];
    run(&mut both, ch, &script, &mut clock, &mut cont, |ps, outs, n| {
        let (pa, pb) = (&ps[0], &ps[1]);
        assert!(pa.print_complete() && pb.print_complete(), "the declaration changed during the continuation");
        let (oa, ob) = (&outs[0], &outs[1]);
        if ob.iter().any(|c| c[..n].iter().any(|x| !x.is_finite())) {
            return Err(format!("block {k}: a non-finite output sample"));
        }
        if oa.iter().zip(ob).any(|(x, y)| x[..n].iter().zip(&y[..n]).any(|(x, y)| x.to_bits() != y.to_bits())) {
            return Err(format!("block {k}: the outputs differ"));
        }
        let ra = record(&**pa);
        if record(&**pb) != ra {
            return Err(format!("block {k}: the records differ"));
        }
        bytes += ra.len() * 8;
        k += 1;
        Ok(())
    })
    .map_err(at)?;
    if !(w.completed)(&both[0]) {
        return Err(at(format!("the continuation did not complete its obligation ({})", (e.describe)(&both[0]))));
    }
    report.history += hist.frames;
    report.continuation += cont.frames;
    report.calls += hist.calls + cont.calls;
    report.bytes += bytes;
    Ok(())
}

/// Every witness of `e` in every configuration; the first failure, else the report lines (each
/// printed as its witness completes).
fn harness<P: Plugin + ?Sized>(e: &Entry<P>) -> Result<Vec<String>, String> {
    for b in CHECKLIST {
        let witnessed = e.witnesses.iter().any(|w| w.boundary == b);
        let explained = e.inapplicable.iter().any(|(x, why)| *x == b && !why.is_empty());
        if witnessed == explained {
            return Err(format!("{}: boundary {b:?} needs exactly one of a witness or a code-backed explanation", e.id));
        }
    }
    let mut lines = Vec::new();
    for w in &e.witnesses {
        let mut report = Report::default();
        let t = std::time::Instant::now();
        for rate in RATES {
            for ch in CHANNELS {
                case(e, w, rate, ch, &mut report)?;
            }
        }
        // The elapsed time is reported, never asserted on.
        lines.push(format!(
            "{} {:?}: history {} frames, continuation {} frames, {} process calls, {} bytes recorded, {:.1} s",
            e.id,
            w.boundary,
            report.history,
            report.continuation,
            report.calls,
            report.bytes,
            t.elapsed().as_secs_f64()
        ));
        println!("{}", lines.last().map_or("", String::as_str));
    }
    Ok(lines)
}

// ---- The fixture entry (the permanent negative controls) ----

/// The field a variant leaves out of record and restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Omit {
    Nothing,
    Smoother,
    Line,
    Counter,
    Generator,
}

static OMIT: std::sync::Mutex<Omit> = std::sync::Mutex::new(Omit::Nothing);

/// The smoother's ramp and the line's length (the fixture's inventory spans).
const RAMP: Span = Span::Seconds(0.01);
const LINE: Span = Span::Seconds(0.005);
/// The counter's period: a click every this many samples since prepare or reset.
const PERIOD: usize = 300;

static FIXTURE_PARAMS: [transit_dsp::ParamInfo; 0] = [];
static FIXTURE_INFO: transit_dsp::PluginInfo = transit_dsp::PluginInfo {
    id: "fixture",
    name: "Fixture",
    short_name: "Fixture",
    category: transit_dsp::Category::Other,
    params: &FIXTURE_PARAMS,
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};

/// A smoother (a linear ramp to its target), a delay line, a counter since prepare or reset, and a
/// generator: each reaches the output.
struct Fixture {
    omit: Omit,
    rate: f32,
    ch: usize,
    max_block: usize,
    cur: f32,
    target: f32,
    steps: u32,
    line: Vec<f32>,
    pos: usize,
    count: u64,
    rng: u32,
}

impl Fixture {
    fn new() -> Box<Fixture> {
        Box::new(Fixture {
            omit: *OMIT.lock().unwrap(),
            rate: 0.0,
            ch: 0,
            max_block: 0,
            cur: 1.0,
            target: 1.0,
            steps: 0,
            line: Vec::new(),
            pos: 0,
            count: 0,
            rng: 1,
        })
    }

    fn ramp(&self) -> u32 {
        frames(&[RAMP], self.rate).saturating_sub(MAX_BLOCK) as u32
    }

    fn config(&self) -> Config<'static> {
        Config { entry: FIXTURE_INFO.id, sample_rate: f64::from(self.rate), max_block: self.max_block as u64, channels: self.ch as u64, content: 0 }
    }
}

impl Plugin for Fixture {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &FIXTURE_INFO
    }
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        (self.rate, self.max_block, self.ch) = (sample_rate, max_block, channels);
        self.line = vec![0.0; frames(&[LINE], sample_rate) - MAX_BLOCK];
        self.reset();
    }
    fn reset(&mut self) {
        (self.cur, self.steps, self.pos, self.count, self.rng) = (self.target, 0, 0, 0, 1);
        self.line.fill(0.0);
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id != "gain" {
            return false;
        }
        self.target = value;
        self.steps = self.ramp();
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "gain").then_some(self.target)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for i in 0..frames {
            let x = io.iter().map(|c| c[i]).sum::<f32>() / io.len().max(1) as f32;
            if self.steps > 0 {
                self.cur += (self.target - self.cur) / self.steps as f32;
                self.steps -= 1;
            }
            let delayed = std::mem::replace(&mut self.line[self.pos], x);
            self.pos = (self.pos + 1) % self.line.len();
            self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (self.rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            let click = if self.count.is_multiple_of(PERIOD as u64) { 0.25 } else { 0.0 };
            self.count += 1;
            let y = self.cur * (x + 0.5 * delayed) + 0.001 * noise + click;
            for c in io.iter_mut() {
                c[i] = y;
            }
        }
    }
    fn print_complete(&self) -> bool {
        true
    }
    /// The inventory: every field encoded (`omit`, the variant, is the test's switch, not state);
    /// `rate`, `ch` and `max_block` are configuration.
    fn fingerprint(&self, w: &mut Writer<'_>) {
        let Fixture { omit, rate: _, ch: _, max_block: _, cur, target, steps, line, pos, count, rng } = self;
        w.record(&self.config(), |w| {
            if *omit != Omit::Smoother {
                w.f32(*cur);
                w.f32(*target);
                w.u32(*steps);
            }
            if *omit != Omit::Line {
                w.f32s(line);
                w.usize(*pos);
            }
            if *omit != Omit::Counter {
                w.u64(*count);
            }
            if *omit != Omit::Generator {
                w.u32(*rng);
            }
        });
    }
    fn restore_print(&mut self, r: &mut Reader<'_>) -> bool {
        let config = self.config();
        let omit = self.omit;
        let mut line = std::mem::take(&mut self.line);
        let state = r.record(&config, |r| {
            let smoother = if omit != Omit::Smoother { Some((r.f32()?, r.f32()?, r.u32()?)) } else { None };
            let pos = if omit != Omit::Line {
                r.f32s(&mut line)?;
                Some(r.usize().filter(|&p| p < line.len())?)
            } else {
                None
            };
            let count = if omit != Omit::Counter { Some(r.u64()?) } else { None };
            let rng = if omit != Omit::Generator { Some(r.u32()?) } else { None };
            Some((smoother, pos, count, rng))
        });
        self.line = line;
        let Some((smoother, pos, count, rng)) = state else { return false };
        if let Some((cur, target, steps)) = smoother {
            (self.cur, self.target, self.steps) = (cur, target, steps);
        }
        if let Some(p) = pos {
            self.pos = p;
        }
        if let Some(c) = count {
            self.count = c;
        }
        if let Some(g) = rng {
            self.rng = g;
        }
        true
    }
}

fn line_len(rate: f32) -> usize {
    frames(&[LINE], rate) - MAX_BLOCK
}

/// The fixture's witnesses and explanations.
fn fixture() -> Entry<Fixture> {
    let none = |_: &Fixture| true;
    Entry {
        id: "fixture",
        make: Fixture::new,
        describe: |p| format!("ramp steps {}, count {}, generator {}", p.steps, p.count, p.rng),
        witnesses: vec![
            // 1: first blocks with a parameter change (the fixture takes no notes).
            Witness {
                boundary: Boundary::AfterPrepare,
                history: |_| vec![],
                continuation: |rate| vec![Step::Param("gain", 0.5), Step::Run(frames(&[RAMP, LINE], rate))],
                reached: |p| p.count == 0 && p.steps == 0,
                completed: |p| p.steps == 0 && p.count > 0,
            },
            // 2: the same after a reset in the middle of a ramp.
            Witness {
                boundary: Boundary::AfterReset,
                history: |rate| vec![Step::Param("gain", 0.25), Step::Run(frames(&[LINE], rate)), Step::Reset],
                continuation: |rate| vec![Step::Param("gain", 0.75), Step::Run(frames(&[RAMP, LINE], rate))],
                reached: |p| p.count == 0,
                completed: |p| p.steps == 0,
            },
            // 5: mid-ramp; the continuation changes the target again mid-ramp, then runs until the
            // ramp completes, plus one block.
            Witness {
                boundary: Boundary::MidRamp,
                history: |rate| vec![Step::Run(frames(&[LINE], rate)), Step::Param("gain", 0.2), Step::RunBlocks(frames(&[RAMP], rate) / 3, 1)],
                continuation: |rate| vec![Step::RunBlocks(17, 7), Step::Param("gain", 1.5), Step::Run(frames(&[RAMP], rate))],
                reached: |p| p.steps > 0,
                completed: |p| p.steps == 0,
            },
            // 10: prepared at another rate, boundary 1's obligation met there, then prepared at the
            // case's rate (T); the continuation meets boundary 1's obligation again.
            Witness {
                boundary: Boundary::SecondPrepare,
                history: |rate| {
                    let other = if rate == 48_000.0 { 96_000.0 } else { 48_000.0 };
                    vec![Step::Prepare(other), Step::Param("gain", 0.5), Step::Run(frames(&[RAMP, LINE], other)), Step::Prepare(rate)]
                },
                continuation: |rate| vec![Step::Param("gain", 0.9), Step::Run(frames(&[RAMP, LINE], rate))],
                reached: |p| p.count == 0 && p.line.len() == line_len(p.rate),
                completed: |p| p.steps == 0 && p.count > 0,
            },
            // The delay line: filled, then every position read (the line's span plus a block).
            Witness {
                boundary: Boundary::Entry("delay line"),
                history: |rate| vec![Step::Run(frames(&[LINE, LINE], rate) + 31)],
                continuation: |rate| vec![Step::Run(frames(&[LINE], rate))],
                reached: |p| p.line.iter().any(|&x| x != 0.0) && p.pos != 0,
                completed: none,
            },
            // The counter: past its period edges, crossing two of them with odd block lengths.
            Witness {
                boundary: Boundary::Entry("counter"),
                history: |_| vec![Step::Run(PERIOD + 137)],
                continuation: |rate| vec![Step::RunBlocks(frames(&[Span::Samples(2 * PERIOD)], rate), 7)],
                reached: |p| !p.count.is_multiple_of(PERIOD as u64),
                completed: none,
            },
            // The generator: advanced, then drawn from.
            Witness {
                boundary: Boundary::Entry("generator"),
                history: |_| vec![Step::Run(1_000)],
                continuation: |_| vec![Step::Run(64)],
                reached: |p| p.rng != 1,
                completed: none,
            },
        ],
        inapplicable: vec![
            (Boundary::VoicesStolen, "no voices: `Fixture` takes no notes (`note_on`, `event` are the trait defaults)"),
            (Boundary::ReleaseTail, "no voices, so no release tails: `Fixture` takes no notes"),
            (Boundary::ModeSwitch, "no modes or engines: `Fixture::process` has one path"),
            (Boundary::Flush, "no feedback and no flush threshold: `Fixture::process` holds no recursive filter"),
            (Boundary::InternalBlock, "no internal block: `Fixture::process` works sample by sample over the host block"),
            (Boundary::Pending, "nothing pending: `set_param` applies at once (its ramp is boundary 5's)"),
            (Boundary::Sustain, "no keys or pedals: `Fixture` takes no notes"),
        ],
    }
}

#[test]
fn the_full_fixture_passes_and_each_omission_variant_fails() {
    let run_with = |omit: Omit| {
        *OMIT.lock().unwrap() = omit;
        let r = harness(&fixture());
        *OMIT.lock().unwrap() = Omit::Nothing;
        r
    };
    run_with(Omit::Nothing).unwrap_or_else(|e| panic!("the full fixture failed: {e}"));
    for omit in [Omit::Smoother, Omit::Line, Omit::Counter, Omit::Generator] {
        match run_with(omit) {
            Ok(_) => panic!("the variant without {omit:?} passed: a field left out of record and restore went unnoticed"),
            Err(e) => println!("variant without {omit:?} fails as it must: {e}"),
        }
    }
}

/// The Macro Oscillator's declaration (ADR 0029, #742 slice 1 push 2): every witness of its set
/// in every configuration.
#[test]
fn the_macro_oscillator_record_is_complete() {
    harness(&macro_osc::entry()).unwrap_or_else(|e| panic!("{e}"));
}

/// A witness set must cover the checklist: a boundary with neither a witness nor an explanation
/// fails the harness before any case runs.
#[test]
fn an_unexplained_boundary_fails_the_harness() {
    let mut e = fixture();
    e.inapplicable.retain(|(b, _)| *b != Boundary::Flush);
    assert!(harness(&e).unwrap_err().contains("Flush"));
}

/// The registry entries and built-in MIDI effects: none declares a complete record yet (push 1 of
/// #742 leaves every declaration false); a declaration is constant across configurations and an
/// instance's life (checked here over a short life), and an entry that turns true must bring its
/// witness set into this harness, which this test then requires.
#[test]
fn the_harness_covers_every_entry_that_declares_a_complete_record() {
    let mut covered = Vec::new();
    for info in transit_dsp::plugins() {
        let mut seen = Vec::new();
        for rate in RATES {
            for ch in CHANNELS {
                let mut p = transit_dsp::create(info.id).unwrap();
                seen.push(p.print_complete());
                p.prepare(rate, MAX_BLOCK, ch);
                seen.push(p.print_complete());
                let mut io = vec![vec![0.1f32; MAX_BLOCK]; ch];
                p.process(&mut io, 97);
                seen.push(p.print_complete());
                p.reset();
                seen.push(p.print_complete());
            }
        }
        assert!(seen.iter().all(|&s| s == seen[0]), "{}: the declaration is not constant", info.id);
        if seen[0] {
            covered.push(info.id);
        }
    }
    for info in transit_dsp::midi_fx::REGISTRY {
        let mut b = transit_dsp::midi_fx::Builtin::new(info.id).unwrap();
        let first = b.effect_ref().print_complete();
        b.effect().reset();
        assert_eq!(b.effect_ref().print_complete(), first, "{}: the declaration is not constant", info.id);
        if first {
            covered.push(info.id);
        }
    }
    // The witness sets the harness has for registry entries.
    let witnessed: &[&str] = &["macro_oscillator"];
    assert_eq!(covered, witnessed, "every entry declaring a complete record has a witness set in this harness");
}
