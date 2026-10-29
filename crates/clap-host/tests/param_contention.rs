//! #78, CLAP side: a block the plug-in is not called for because its instance is held (as the main
//! thread holds it while it restarts or rescans the instance) keeps the host's values and says so,
//! until a processed block delivers them. A test binary of its own: holding every live instance
//! would skip the blocks of tests running beside it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::Plugin;

mod common;
use common::fixture_dir;

const GAIN: &str = "clap:org.soundcraft.test.gain";

/// One block of 0.5 through the plug-in.
fn block(p: &mut Box<dyn Plugin>) -> Vec<f32> {
    let mut io = vec![vec![0.5f32; 256], vec![0.5f32; 256]];
    p.process(&mut io, 256);
    io.swap_remove(0)
}

#[test]
fn a_value_set_while_the_instance_is_held_reaches_the_next_processed_block() {
    fixture_dir();
    let mut p = transit_clap_host::create(GAIN).unwrap();
    p.prepare(48_000.0, 256, 2);
    assert!(block(&mut p).iter().all(|&x| (x - 0.5).abs() < 1e-6), "the fixture starts at gain 1.0");
    assert!(!p.params_pending());
    transit_clap_host::while_instances_held(|| {
        assert!(p.set_param("7", 1.5));
        assert!(p.set_param_at(40, "7", 2.0));
        assert!(block(&mut p).iter().all(|&x| x.abs() < 0.6), "the held block is the gate's fade out");
        assert!(p.params_pending(), "the held block kept the values");
    });
    // The latest value (2.0), from the block start (#117 collapses the timed point). The first
    // 2 ms are the gate's fade back in.
    let out = block(&mut p);
    assert!(out[96..].iter().all(|&x| (x - 1.0).abs() < 1e-6), "{:?}", &out[90..100]);
    assert!(!p.params_pending(), "delivered");
}
