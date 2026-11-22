//! The Filter's witness set (ADR 0029, amendment for #742, section 13; slice 2).
//!
//! An insert that reads its track's notes (`Plugin::event`, as the mix gives them): one paraphonic
//! gate (the table of held notes, 256 entries), two envelopes, an LFO, a follower, four smoothers,
//! the control-rate grid and the transport. The spans the inventory declares (`filter/record.rs`),
//! fed to the one duration calculator:
//! - `SMOOTH`: the smoothers' settling, 16 time constants of 20 ms;
//! - `RELEASE`: an envelope's release from full to idle (it stops below 1e-5, -100 dB, and its time
//!   is to -60 dB) at the witnesses' longest release, 150 ms: 5/3 of it;
//! - `GRID`: the control-rate grid, [`CONTROL`] samples;
//! - `LFO`: one cycle at the witnesses' slowest rate (2 Hz);
//! - `DECAY`: silence long enough for the filter memories and the follower to reach their flush
//!   endpoint;
//! - `HOLD`: a few control periods.

use super::*;
use transit_dsp::event::{CC_SUSTAIN, Message, NoteId, PluginEvent};
use transit_dsp::plugins::filter::{CONTROL, Filter, FilterObserved};

const SMOOTH: Span = Span::Seconds(16.0 * 0.020);
const RELEASE: Span = Span::Seconds(0.150 * 5.0 / 3.0);
const GRID: Span = Span::Samples(CONTROL as usize);
const LFO: Span = Span::Seconds(1.0 / 2.0);
const DECAY: Span = Span::Seconds(2.0);
const HOLD: Span = Span::Seconds(0.02);

type W = Witness<Filter>;

fn obs(p: &Filter) -> FilterObserved {
    p.observe()
}

fn span(spans: &[Span], rate: f32) -> usize {
    frames(spans, rate) - MAX_BLOCK
}

fn idle(p: &Filter) -> bool {
    obs(p).env_stage == 0 && obs(p).amp_stage == 0
}

/// A note-on with a channel and voice id at `at`.
fn on(channel: u8, key: u8, velocity: u8, voice: i32, at: u32) -> Step {
    Step::Event(at, PluginEvent { channel, note_id: voice, message: Message::NoteOn { key, velocity, id: NoteId::NONE } })
}

fn off(channel: u8, key: u8, voice: i32, at: u32) -> Step {
    Step::Event(at, PluginEvent { channel, note_id: voice, message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE } })
}

fn pedal(down: bool, at: u32) -> Step {
    Step::Event(
        at,
        PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control { cc: CC_SUSTAIN, value: if down { 127 } else { 0 } } },
    )
}

/// The common setup: every modulation source on, the amp envelope on.
fn setup() -> Vec<Step> {
    vec![
        Step::Param("cutoff", 800.0),
        Step::Param("resonance", 40.0),
        Step::Param("drive", 6.0),
        Step::Param("env_amount", 60.0),
        Step::Param("env_attack", 5.0),
        Step::Param("env_decay", 120.0),
        Step::Param("env_sustain", 30.0),
        Step::Param("env_release", 150.0),
        Step::Param("lfo_amount", 30.0),
        Step::Param("lfo_rate", 3.0),
        Step::Param("follow_amount", 20.0),
        Step::Param("key_track", 50.0),
        Step::Param("amp_on", 1.0),
        Step::Param("amp_attack", 3.0),
        Step::Param("amp_release", 120.0),
    ]
}

/// Boundary 1's obligation: the first blocks with notes and parameter changes, then every note
/// released and the envelopes run to idle.
fn first_blocks(rate: f32) -> Vec<Step> {
    vec![
        Step::On(60, 100, 10),
        Step::Run(span(&[HOLD], rate)),
        Step::Param("cutoff", 1500.0),
        on(1, 67, 80, 7, 3),
        Step::Run(span(&[HOLD], rate)),
        Step::Off(60, 0),
        off(1, 67, 7, 5),
        Step::Run(span(&[RELEASE, HOLD], rate)),
    ]
}

/// Every one of the table's 256 entries held (channels 0 and 1, every key; channel 1 with voice
/// ids), then one note more, which the full table cannot hold.
fn fill_the_table() -> Vec<Step> {
    let mut h = Vec::new();
    for key in 0..128u8 {
        h.push(Step::On(key, 1 + key % 127, u32::from(key) as usize % 64));
        h.push(on(1, key, 90, 1000 + i32::from(key), u32::from(key) % 64));
    }
    h.push(Step::Run(97));
    h.push(on(2, 60, 70, -1, 0));
    h.push(Step::Run(31));
    h
}

/// Every held note of [`fill_the_table`] released.
fn release_the_table(rate: f32) -> Vec<Step> {
    let mut c = Vec::new();
    for key in 0..128u8 {
        c.push(Step::Off(key, u32::from(key) as usize % 64));
        c.push(off(1, key, 1000 + i32::from(key), 63 - u32::from(key) % 64));
    }
    c.push(off(2, 60, -1, 0));
    c.push(off(2, 61, -1, 0));
    c.push(Step::Run(span(&[RELEASE, HOLD], rate)));
    c
}

fn witnesses() -> Vec<W> {
    vec![
        // 1: after prepare, before any process.
        W {
            boundary: Boundary::AfterPrepare,
            history: |_| vec![],
            continuation: |rate| [setup(), first_blocks(rate)].concat(),
            reached: |p| obs(p).clock == 0 && idle(p),
            completed: |p| idle(p) && obs(p).held == 0 && obs(p).clock > 0,
        },
        // 2: a reset with notes held and the envelopes sounding.
        W {
            boundary: Boundary::AfterReset,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::On(64, 90, 0), Step::Run(span(&[HOLD], rate)), Step::Reset]);
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).clock == 0 && idle(p) && obs(p).held == 0,
            completed: |p| idle(p) && obs(p).held == 0,
        },
        // 3: every entry of the table held and a note more (the table full); the continuation
        // strikes another note the table cannot hold, then releases every note until idle.
        W {
            boundary: Boundary::VoicesStolen,
            history: |_| [setup(), fill_the_table()].concat(),
            continuation: |rate| {
                let mut c = vec![on(2, 61, 50, -1, 9), Step::Run(span(&[HOLD], rate))];
                c.extend(release_the_table(rate));
                c
            },
            reached: |p| obs(p).held == 256,
            completed: |p| idle(p) && obs(p).held == 0,
        },
        // 4: inside the release tails; the continuation runs the release to its end.
        W {
            boundary: Boundary::ReleaseTail,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::On(55, 110, 3), Step::Run(span(&[HOLD], rate)), Step::Off(55, 1), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| vec![Step::Run(span(&[RELEASE, HOLD], rate))],
            reached: |p| obs(p).env_stage == 4 && obs(p).amp_stage == 4,
            completed: idle,
        },
        // 4, the other branch: a retrigger during the release, released again, run to idle (an
        // all-notes-off on the way, as the mix sends for a skipped block).
        W {
            boundary: Boundary::ReleaseTail,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::On(55, 110, 3), Step::Run(span(&[HOLD], rate)), Step::Off(55, 1), Step::Run(span(&[HOLD], rate))]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::On(57, 40, 20),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Off(57, 2),
                    Step::Run(span(&[HOLD], rate)),
                    Step::On(59, 64, 0),
                    Step::Run(47),
                    Step::AllNotesOff,
                    Step::Run(span(&[RELEASE, HOLD], rate)),
                ]
            },
            reached: |p| obs(p).env_stage == 4 && obs(p).amp_stage == 4,
            completed: |p| idle(p) && obs(p).held == 0,
        },
        // 5: mid-ramp (every smoother moving); the continuation changes every target again
        // mid-ramp with odd block lengths, then runs until every ramp completes, plus one block.
        W {
            boundary: Boundary::MidRamp,
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::On(62, 100, 0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("cutoff", 4000.0),
                    Step::Param("resonance", 90.0),
                    Step::Param("drive", 18.0),
                    Step::Param("output", -12.0),
                    Step::RunBlocks(120, 13),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::RunBlocks(17, 7),
                    Step::Param("cutoff", 200.0),
                    Step::Param("resonance", 5.0),
                    Step::Param("drive", 2.0),
                    Step::Param("output", 6.0),
                    Step::RunBlocks(33, 11),
                    Step::Run(frames(&[SMOOTH], rate)),
                ]
            },
            reached: |p| obs(p).settling.iter().all(|s| *s),
            completed: |p| obs(p).settling.iter().all(|s| !s),
        },
        // 6: after a mode switch with the dormant state non-default: the 24 dB slope's second
        // sections hold memory at 12 dB, the amp envelope and the LFO's sync off; the continuation
        // revisits every mode at 24 dB, the synced LFO with the transport, and the amp envelope.
        W {
            boundary: Boundary::ModeSwitch,
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("slope", 1.0),
                    Step::Param("mode", 2.0),
                    Step::On(48, 100, 0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("slope", 0.0),
                    Step::Param("mode", 1.0),
                    Step::Param("amp_on", 0.0),
                    Step::Run(span(&[HOLD], rate)),
                ]);
                h
            },
            continuation: |rate| {
                let mut c = vec![Step::Param("slope", 1.0)];
                for mode in [0.0, 1.0, 2.0, 3.0] {
                    c.extend([Step::Param("mode", mode), Step::Run(span(&[HOLD], rate))]);
                }
                c.extend([
                    Step::Param("lfo_sync", 1.0),
                    Step::Param("lfo_division", 6.0),
                    Step::Transport(Some(133.0)),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("amp_on", 1.0),
                    Step::Off(48, 0),
                    Step::Run(span(&[RELEASE, HOLD], rate)),
                ]);
                c
            },
            reached: |p| obs(p).second_section,
            completed: idle,
        },
        // 7: the flush boundary: a note and input, then silence until the filter memories and the
        // follower flush; the continuation runs on in silence, then a small input.
        W {
            boundary: Boundary::Flush,
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("slope", 1.0),
                    Step::On(50, 100, 0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Off(50, 0),
                    Step::Silent(span(&[DECAY], rate)),
                ]);
                h
            },
            continuation: |rate| vec![Step::Silent(997), Step::On(52, 30, 5), Step::Small(span(&[HOLD], rate))],
            reached: |p| idle(p) && !obs(p).second_section,
            completed: |p| !idle(p),
        },
        // 8: inside the control-rate grid (the clock off a multiple of 16); the continuation crosses
        // grid edges with odd block lengths, a parameter change and a note between them.
        W {
            boundary: Boundary::InternalBlock,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::On(60, 90, 0), Step::RunBlocks(span(&[HOLD], rate) / 7 * 7 + 5 * 7, 7)]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::RunBlocks(21, 7),
                    Step::Param("cutoff", 2400.0),
                    Step::RunBlocks(35, 5),
                    Step::On(67, 70, 2),
                    Step::RunBlocks(33, 11),
                    Step::RunBlocks(frames(&[GRID, GRID], rate), 3),
                ]
            },
            reached: |p| !obs(p).clock.is_multiple_of(CONTROL),
            completed: |p| !idle(p),
        },
        // 9: events pending at T (given before the next block: notes on several channels, an
        // offset past the next block's end, the pedal); the continuation delivers every one, plus
        // one block.
        W {
            boundary: Boundary::Pending,
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Run(span(&[HOLD], rate)), Step::On(64, 90, 40), on(3, 70, 60, 12, 9), pedal(true, 30), Step::Off(64, 300)]);
                h
            },
            continuation: |rate| vec![Step::Run(span(&[HOLD], rate)), off(3, 70, 12, 0), pedal(false, 4), Step::Run(span(&[RELEASE, HOLD], rate))],
            reached: |p| obs(p).queued == 4,
            completed: |p| obs(p).queued == 0 && idle(p),
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
                h.push(Step::On(70, 100, 0));
                h.push(Step::Run(77));
                h.push(Step::Prepare(rate));
                h
            },
            continuation: first_blocks,
            reached: |p| obs(p).clock == 0 && idle(p),
            completed: |p| idle(p) && obs(p).clock > 0,
        },
        // 11: a key released while the pedal is down, another held; the continuation keeps the
        // sustained note through processing, releases the held key, then the pedal, and runs to
        // idle.
        W {
            boundary: Boundary::Sustain,
            history: |rate| {
                let mut h = setup();
                h.extend([
                    pedal(true, 0),
                    Step::On(60, 100, 1),
                    Step::On(64, 90, 2),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Off(60, 3),
                    Step::Run(span(&[HOLD], rate)),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Run(span(&[HOLD], rate)),
                    Step::Off(64, 0),
                    Step::Run(span(&[HOLD], rate)),
                    pedal(false, 5),
                    Step::Run(span(&[RELEASE, HOLD], rate)),
                ]
            },
            reached: |p| obs(p).pedal && obs(p).held == 2,
            completed: |p| idle(p) && obs(p).held == 0 && !obs(p).pedal,
        },
        // The generator: the random LFO shape drawing at T; the continuation draws over two cycles.
        W {
            boundary: Boundary::Entry("generator"),
            history: |rate| {
                let mut h = setup();
                h.extend([Step::Param("lfo_shape", 4.0), Step::Param("lfo_rate", 2.0), Step::On(60, 100, 0), Step::Run(span(&[LFO], rate) / 2)]);
                h
            },
            continuation: |rate| vec![Step::Run(span(&[LFO, LFO], rate))],
            reached: |p| !idle(p),
            completed: |p| !idle(p),
        },
        // The synced LFO and the transport: anchored while playing at T; the continuation plays
        // on, jumps the tempo (a new anchor), changes the division, then stops.
        W {
            boundary: Boundary::Entry("transport"),
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("lfo_sync", 1.0),
                    Step::Param("lfo_division", 3.0),
                    Step::Transport(Some(100.0)),
                    Step::On(60, 100, 0),
                    Step::Run(span(&[LFO], rate)),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Run(span(&[HOLD], rate)),
                    Step::Transport(Some(150.0)),
                    Step::Run(span(&[LFO], rate)),
                    Step::Param("lfo_division", 0.0),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Transport(None),
                    Step::Run(span(&[LFO], rate)),
                ]
            },
            reached: |p| obs(p).anchored,
            completed: |p| !obs(p).anchored,
        },
        // The transport as last told, read before it is told again: playing in a bar grid that
        // starts off zero (an earlier meter change) with a bar division synced, T between blocks;
        // the continuation runs blocks with no `set_transport` (the synced LFO reads the last told
        // block position, bar origin and bar length), then told blocks, then stops (a stopped
        // transport is told once: the LFO runs on at the bar length last told).
        W {
            boundary: Boundary::Entry("transport not told"),
            history: |rate| {
                let mut h = setup();
                h.extend([
                    Step::Param("lfo_amount", 60.0),
                    Step::Param("lfo_sync", 1.0),
                    Step::Param("lfo_division", 0.0),
                    Step::BarOffset(1.5),
                    Step::Transport(Some(100.0)),
                    Step::On(60, 100, 0),
                    Step::Run(span(&[LFO], rate)),
                ]);
                h
            },
            continuation: |rate| {
                vec![
                    Step::Untold(span(&[HOLD], rate)),
                    Step::Run(span(&[HOLD], rate)),
                    Step::Param("lfo_division", 2.0),
                    Step::Transport(None),
                    Step::Run(span(&[LFO], rate)),
                ]
            },
            reached: |p| obs(p).anchored,
            completed: |p| !obs(p).anchored,
        },
    ]
}

pub(super) fn entry() -> Entry<Filter> {
    Entry {
        id: "filter",
        make: || Box::new(Filter::new()),
        describe: |p| format!("{:?}", obs(p)),
        witnesses: witnesses(),
        inapplicable: vec![],
        // The queue past `queued` and the note entries not in use.
        poison: Some(|p, k| p.poison_unobservable(k + 1)),
    }
}
