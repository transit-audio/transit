//! The scheduling-condition property with the opt-in tail, including returns and group buses.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[allow(dead_code)]
mod common;
#[allow(dead_code)]
#[path = "common/sched.rs"]
mod sched;
use sched::*;
use transit_mix::MixEngine;

/// S2: existing mute meaning, exact samples and DSP state fingerprints under seeded holds.
/// timing: far-deadline graph_run uses the explicit 60-second deadline and observable holds.
#[test]
fn s2_tail_scheduling_equivalence() {
    let _switch = graph_on();
    MixEngine::force_tail(true);
    let mut tops = builtin();
    tops.extend(corpus().into_iter().map(|(name, s)| Topology::new(&name, s, vec![])));
    every(&tops, &[174, 210, 17]);
    MixEngine::force_tail(false);
}
