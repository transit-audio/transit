//! DSP equivalence of the task graph under any scheduling condition (issue #210, ADR 0029
//! amendment section 8): seeded stall schedules, skips as the only difference, state continuity
//! across the holes.
//!
//! **The property.** For every keyed topology of `sidechain.rs` and `render_graph.rs` (and the
//! corpus sessions with a key edge added) and the continuity session, for each seed: the stall
//! schedule of the seed ([`Schedule::seeded`]: which strip is held in which block, for how many
//! blocks) is replayed on the graph path through two render workers and through none, and
//!
//! 1. the strips that missed a block are exactly the ones the graph counted (busy, late,
//!    unstarted, upstream-missed, by strip and block, from the per-strip figures): every silent
//!    strip has exactly one counted cause that block, and every counted skip is silent; a
//!    strip's own overrun falls only on a strip the schedule holds that block, and every held
//!    strip misses it;
//! 2. the graph render equals, bit for bit on every output sample of every block, the serial
//!    render with exactly those blocks muted ([`transit_mix::MixEngine::mute_next_block`]: a busy, unstarted or
//!    upstream-missed strip skipped, its keys not filled, its lines neither written nor idled, its
//!    MIDI held; a late strip rendered and its output dropped, as its worker's late render moves
//!    its state on), and the tail skips of the auxes keyed from a missed strip are the same;
//! 3. at every block boundary, every strip's state fingerprint ([`transit_mix::StripPrint`]: the plug-ins'
//!    state (the compressor's detector envelope, sidechain filters and lookahead lines, the test
//!    delays' and the continuity instruments' state), the key alignment and delay compensation
//!    lines (contents and write positions), and the instrument's delivered and pending MIDI) is
//!    the same in both renders, for every strip that is home (a strip a worker holds has no
//!    readable state until it is back).
//!
//! And across a graph change: a key edge added or removed while a worker holds an old task, the
//! plan refused until the task is home, its result retired unread, and the new graph's output and
//! state equal to the muted serial render's from its first block.
//!
//! **Realizing a schedule (E49).** Nothing here is a clock or a thread-timing outcome: the
//! schedule is the test's input. Every phase-1 strip carries a hold insert (unity gain) that,
//! armed, keeps a render worker inside the strip's render until the test releases it; a held
//! strip's cost history is an hour, so the callback never renders it inline; and the deterministic
//! deadline ([`transit_mix::MixEngine::hold_until_alone`]) is reached only once the held strips are the only
//! ones unresolved and each is claimed by a worker. Through two workers a held strip is then
//! `Late` in its first block and `Busy` after; through none it is `Unstarted`; its dependents are
//! upstream-missed. At most one strip is held at a time with two workers (the other worker stays
//! free for the rest), any number with none.
//!
//! **Seeds.** `TRANSIT_SCHED_SEEDS` (default 32) seeds per topology and worker count; the corpus
//! sessions, whose state is only partly readable, run an eighth of them (at least 4) over a longer
//! render. `TRANSIT_SCHED_SEED=<n>` runs that one seed. A failing seed is shrunk
//! ([`Schedule::shrink`]) and printed as a Rust literal with the first divergence; the minimal
//! schedule lands as a named regression test ([`regression`], see the template at the end).
//!
//! **Where state cannot be read** (the corpus sessions' equalizers, saturator, chorus, reverb,
//! limiter-mode compressor and synths have no fingerprint): the holds end `WINDOW` blocks before the end of the
//! render and every corpus case states its post-fault window, the longest tail and latency of its
//! plug-ins plus the plan's latency, rounded up to whole blocks; the render runs that long after
//! the last hold, and every sample of it is compared.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

#[path = "common/sched.rs"]
mod sched;

use sched::*;
use transit_model::Tap;
use transit_rt::worker_set::OverrunCause;
use transit_rt::worker_set::schedule::{Schedule, Shape};

/// Gate (#210): the built-in keyed topologies, through two workers and through none.
#[test]
fn the_graph_equals_the_muted_serial_render_under_seeded_stall_schedules() {
    let _on = graph_on();
    every(&builtin(), &seeds(false));
}

/// Gate (#210): the continuity session (live notes and pedals; held notes, pending and deferred
/// MIDI in the fingerprint), through two workers and through none.
#[test]
fn the_continuity_session_equals_its_muted_serial_render_under_seeded_stall_schedules() {
    let _on = graph_on();
    let (s, ids) = continuity();
    every(&[Topology::new("the continuity session", s, ids)], &seeds(false));
}

/// Gate (#210): the corpus sessions with a key edge added (their equalizers, saturator, chorus,
/// reverb, limiter-mode compressor and synths have no fingerprint: each case states its post-fault window).
#[test]
fn the_corpus_sessions_with_a_key_equal_their_muted_serial_render_under_seeded_stall_schedules() {
    let _on = graph_on();
    let seeds = seeds(true);
    let cases = corpus();
    assert!(cases.len() >= 2, "the released sessions with four or more live tracks");
    let mut tops = Vec::new();
    for (name, s) in cases {
        let mut top = Topology::new(&name, s, vec![]);
        let latency = {
            let mut rig = top.rig();
            rig.block();
            rig.mix.latency
        };
        top.window = post_fault_window(&top.session, latency);
        top.blocks = 40 + top.window;
        println!("{name}: holds in blocks 2..40, post-fault window {} blocks ({} samples)", top.window, top.window * BLOCK);
        tops.push(top);
    }
    every(&tops, &seeds);
}

/// Gate (#210): a key edge removed, and one added, while a worker holds an old task: the plan is
/// refused until the task is home, the old result is retired unread, and the new graph's output
/// and state equal the muted serial render's from its first block.
#[test]
fn a_graph_change_while_a_worker_holds_an_old_task_matches_the_muted_serial_render() {
    let _on = graph_on();
    for (name, from, to) in [("a key edge removed", true, false), ("a key edge added", false, true)] {
        let top = Topology::new(name, chain_with(from), vec![]);
        let next = Topology::new(name, chain_with(to), vec![]);
        let (shape, names) = shape(&top, 2);
        let bass = {
            let rig = top.rig();
            rig.mix.strip_index(track(&top.session, "Bass")).unwrap()
        };
        let change = Change { at: 20, session: next.session.clone() };
        for seed in seeds(false) {
            // The seed's schedule before the change, then Bass (a key source of the edge that
            // changes) held across it.
            let mut schedule = Schedule::seeded(seed, &Shape { last: 16, ..shape.clone() });
            schedule.holds.push(transit_rt::worker_set::schedule::Hold { block: 19, strip: bass, blocks: 1 });
            let g = graph_run(&top, 2, &schedule, Some(&change)).unwrap_or_else(|e| panic!("{name}, seed {seed}: {e}"));
            assert_eq!(g.conflicts, 0, "{name}, seed {seed}: the new graph's results carry its identity");
            assert!(g.late_results >= 1, "{name}, seed {seed}: the old task's result retired unread");
            assert_eq!(g.causes[19].iter().find(|c| c.0 == bass).map(|c| c.1), Some(OverrunCause::Late), "{name}, seed {seed}");
            let m = muted_run(&top, &g.mutes, Some(&change));
            if let Err(e) = compare(&top, &names, &g, &m) {
                panic!("{name}, seed {seed}:\n{}\n{e}", schedule.to_rust(&|s| names.get(&s).cloned().unwrap_or_default()));
            }
        }
    }
}

// ---- Regressions ----

/// The template a minimal schedule lands as (copy, name it after what it found, paste the
/// printed literal): here the key source of a chain held two blocks through two workers, then
/// the middle strip, then both ends through none.
#[test]
fn regression_a_chain_held_at_each_link() {
    let _on = graph_on();
    let top = Topology::new("a chain of two keys", chain_with(true), vec![]);
    let rig = top.rig();
    let [kick, bass, pad] = ["Kick", "Bass", "Pad"].map(|n| rig.mix.strip_index(track(&top.session, n)).unwrap());
    drop(rig);
    regression(&top, 2, &Schedule::new(&[(kick, 5, 2), (bass, 12, 1), (pad, 20, 3)]));
    regression(&top, 0, &Schedule::new(&[(kick, 5, 2), (bass, 6, 3), (pad, 20, 1)]));
}

/// Found by this gate (#174): an early key's strip held one block with no workers is taken back
/// unrendered at the deadline; its key entry's line must not have run for that block (it did:
/// the key was filled, early-key delay included, before the publish, so from then on the key ran
/// one block ahead of the strip's own signal). The keyed strip is Bass.
#[test]
fn regression_an_early_key_held_with_no_worker() {
    let _on = graph_on();
    let top = Topology::new("an early key", latent_kick_to_bass(0, 128, 0, Tap::Post, 0.0), vec![]);
    let bass = bass_index(&top);
    regression(&top, 0, &Schedule::new(&[(bass, 33, 1)]));
}

/// The same defect with both sides latent and the key early.
#[test]
fn regression_both_latent_the_key_early_held_with_no_worker() {
    let _on = graph_on();
    let top = Topology::new("both latent, the key early", latent_kick_to_bass(64, 128, 0, Tap::Post, 0.0), vec![]);
    let bass = bass_index(&top);
    regression(&top, 0, &Schedule::new(&[(bass, 38, 1)]));
}

/// The same defect with a pre-fader key from a silent fader, early.
#[test]
fn regression_a_pre_fader_key_from_a_silent_fader_early_held_with_no_worker() {
    let _on = graph_on();
    let top = Topology::new("a pre-fader key from a silent fader, early", latent_kick_to_bass(0, 128, 0, Tap::Pre, -80.0), vec![]);
    let bass = bass_index(&top);
    regression(&top, 0, &Schedule::new(&[(bass, 30, 1)]));
}
