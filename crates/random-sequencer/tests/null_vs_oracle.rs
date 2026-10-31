//! The port against the oracle over the fixed sweep (ADR 0033 gate, #202): every point, the three
//! gates and the four voltages of every frame, bit for bit (the floor is exact:
//! `transit-macro-osc-oracle`'s `tests/sequencer_self_floor.rs`). A failure names the point and
//! the first frame that differs; it is a defect to hunt, never a run to repeat.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::sequencer::{self as oracle, SEQUENCER_BLOCK_SIZE};
use transit_macro_osc_oracle::sequencer_sweep::{Point, Render, SEED, points, render_point};
use transit_macro_osc_oracle::{Session, subset};
use transit_random_sequencer::{BLOCK_SIZE, Controls, Generators, LOOP_LENGTHS, SAMPLE_RATE, Scale, Y_DIVIDERS, scales};

fn port_controls(c: &oracle::Controls) -> Controls {
    Controls {
        t_model: c.t_model,
        t_range: c.t_range,
        t_rate: c.t_rate,
        t_bias: c.t_bias,
        t_jitter: c.t_jitter,
        deja_vu: c.deja_vu,
        deja_vu_length: c.deja_vu_length,
        t_deja_vu: c.t_deja_vu,
        x_deja_vu: c.x_deja_vu,
        t_pulse_width_mean: c.t_pulse_width_mean,
        t_pulse_width_std: c.t_pulse_width_std,
        x_control_mode: c.x_control_mode,
        x_range: c.x_range,
        x_register_mode: c.x_register_mode,
        x_register_value: c.x_register_value,
        x_spread: c.x_spread,
        x_bias: c.x_bias,
        x_steps: c.x_steps,
        x_scale: c.x_scale,
        y_spread: c.y_spread,
        y_bias: c.y_bias,
        y_steps: c.y_steps,
        y_divider: c.y_divider,
        y_range: c.y_range,
        t_reset: c.t_reset,
        x_reset: c.x_reset,
        t_external: c.t_external,
        xy_clock_source: c.xy_clock_source,
    }
}

fn port_scale(s: &oracle::Scale) -> Scale {
    Scale::new(s.base_interval, &s.degrees)
}

/// The port over one point, in blocks of `block` frames (1 to 5; the controls change per 5-frame
/// block of the sweep).
fn render_port(point: &Point, block: usize) -> Render {
    let mut g = Generators::new(SEED);
    if let Some((slot, scale)) = &point.scale {
        g.load_scale(*slot, &port_scale(scale));
    }
    let (t_clock, xy_clock) = point.clocks();
    let frames = point.frames();
    let blocks = frames / SEQUENCER_BLOCK_SIZE;
    let mut out = Render { gates: vec![[0; 3]; frames], voltages: vec![[0.0; 4]; frames] };
    for (k, (gates, voltages)) in out.gates.chunks_mut(block).zip(out.voltages.chunks_mut(block)).enumerate() {
        let at = k * block;
        let c = port_controls(&point.controls(at / SEQUENCER_BLOCK_SIZE, blocks));
        let t = t_clock.as_ref().map(|f| &f[at..at + gates.len()]);
        let x = xy_clock.as_ref().map(|f| &f[at..at + gates.len()]);
        g.process(&c, t, x, gates, voltages);
    }
    out
}

#[test]
fn the_constants_and_tables_match_the_oracle() {
    let session = Session::lock();
    let (block, rate) = session.sequencer_constants();
    assert_eq!(block, BLOCK_SIZE);
    #[allow(clippy::cast_precision_loss)] // 32000.
    let rate = rate as f32;
    assert_eq!(rate, SAMPLE_RATE);
    assert_eq!(session.sequencer_loop_lengths(), LOOP_LENGTHS.to_vec());
    assert_eq!(session.sequencer_y_dividers(), Y_DIVIDERS.to_vec());
    let presets = session.sequencer_preset_scales();
    assert_eq!(presets.len(), scales::PRESETS.len());
    for (o, p) in presets.iter().zip(scales::PRESETS.iter()) {
        assert_eq!(o.base_interval, p.base_interval, "{}", p.name);
        assert_eq!(o.degrees, p.degrees.to_vec(), "{}", p.name);
    }
}

#[test]
fn every_point_nulls() {
    let sweep = subset::strided("sequencer null", points());
    let mut failures = Vec::new();
    for point in &sweep {
        let expected = {
            let session = Session::lock();
            render_point(&session, point).unwrap()
        };
        let got = render_port(point, BLOCK_SIZE);
        if let Some(d) = expected.first_difference(&got) {
            failures.push(format!("{}: {d}", point.label()));
        }
    }
    assert!(failures.is_empty(), "{} of {} points differ:\n{}", failures.len(), sweep.len(), failures.join("\n"));
}

/// The oracle takes blocks of 1 to 5 frames; so does the port, to the same bits (the controls
/// held, so the block size is all that changes).
#[test]
fn shorter_blocks_null_too() {
    let session = Session::lock();
    for point in
        points().iter().filter(|p| p.motion == transit_macro_osc_oracle::sequencer_sweep::Motion::Static && p.reset_every.is_none()).step_by(23)
    {
        for block in [1, 2, 3, 4] {
            let mut s = session.sequencer(SEED).unwrap();
            if let Some((slot, scale)) = &point.scale {
                s.load_scale(*slot, scale).unwrap();
            }
            let (t_clock, xy_clock) = point.clocks();
            let frames = 30_000;
            let mut expected = Render { gates: vec![[0; 3]; frames], voltages: vec![[0.0; 4]; frames] };
            for (k, (g, v)) in expected.gates.chunks_mut(block).zip(expected.voltages.chunks_mut(block)).enumerate() {
                let at = k * block;
                let t = t_clock.as_ref().map(|f| &f[at..at + g.len()]);
                let x = xy_clock.as_ref().map(|f| &f[at..at + g.len()]);
                s.process(&point.base, t, x, g, v).unwrap();
            }
            let mut got = render_port(point, block);
            got.gates.truncate(frames);
            got.voltages.truncate(frames);
            assert!(expected.first_difference(&got).is_none(), "{} block {block}: {:?}", point.label(), expected.first_difference(&got));
        }
    }
}

/// A long run with deja vu: one minute of the shuffling and the looping deja vu over the drums and
/// the coin toss, port and oracle frame for frame.
#[test]
fn a_long_run_with_deja_vu_nulls() {
    let base = transit_macro_osc_oracle::sequencer_sweep::center();
    for (deja_vu, model) in [(0.8_f32, 2), (0.5, 0), (0.3, 1)] {
        let mut point = points().into_iter().next().unwrap();
        point.base = oracle::Controls { deja_vu, t_deja_vu: 1, x_deja_vu: 1, t_model: model, t_jitter: 0.3, x_steps: 0.8, ..base };
        point.frames = 32_000 * 60;
        let expected = {
            let session = Session::lock();
            render_point(&session, &point).unwrap()
        };
        let got = render_port(&point, BLOCK_SIZE);
        assert!(expected.first_difference(&got).is_none(), "deja vu {deja_vu} model {model}: {:?}", expected.first_difference(&got));
    }
}
