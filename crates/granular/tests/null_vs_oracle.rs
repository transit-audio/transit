//! Null tests of the granular texture processor at every quality against the C++ oracle (ADR 0033,
//! #199).
//!
//! Each quality and gate pattern is rendered over the oracle's granular sweep
//! (`granular_sweep::points`: the nine continuous parameters each at five levels around a center,
//! 37 points per pattern; free-running, frozen from the middle to three quarters, and triggered
//! every 25 blocks; 32 kHz; the firmware's 32-frame blocks; the module's memory sizes; a fresh
//! processor and the same seed and input per point, each point as long as the quality's recording
//! plus 0.25 s) by the port and by the oracle on the same runner. The int16 output must be
//! identical, 0 LSB: the bound is the oracle's measured floor for this runner family (`-inf`) plus a
//! margin of zero. The random state after each point must equal the oracle's, so the port draws
//! exactly as often as the firmware. Flush-to-zero, which the oracle runs with, is reproduced in
//! the port's own arithmetic (`transit_granular::float::F`); the test process keeps Rust's default
//! floating-point mode.
//!
//! Set `GRANULAR_NULL_POINTS` to print the residual and the first differing frame of every point
//! whose label contains its value (empty: every point).

#![forbid(unsafe_code)]

use transit_granular::{BufferSize, Granular, MAX_BLOCK_SIZE, Parameters, Quality};
use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::floor::RunnerFamily;
use transit_macro_osc_oracle::granular::PlaybackMode;
use transit_macro_osc_oracle::granular::{Parameters as OracleParameters, Quality as OracleQuality};
use transit_macro_osc_oracle::granular_sweep::{Gate, Point, SEED, input_signal, longest_point_frames, point_frames, points_in_mode, render_point};
use transit_macro_osc_oracle::residual::residual_i16;

/// The margin over the oracle's floor, dB. Zero: the port is held to bit identity.
const MARGIN_DB: f64 = 0.0;

fn bound_dbfs() -> f64 {
    RunnerFamily::current().self_floor_dbfs().unwrap_or(f64::NEG_INFINITY) + MARGIN_DB
}

fn port_quality(q: OracleQuality) -> Quality {
    Quality::ALL[q.index()]
}

fn port_parameters(p: &OracleParameters) -> Parameters {
    Parameters {
        position: p.position,
        size: p.size,
        pitch: p.pitch,
        density: p.density,
        texture: p.texture,
        dry_wet: p.dry_wet,
        stereo_spread: p.stereo_spread,
        feedback: p.feedback,
        reverb: p.reverb,
        freeze: p.freeze,
        trigger: p.trigger,
        gate: p.gate,
    }
}

fn render_port_point(quality: OracleQuality, point: &Point, input: &[[i16; 2]]) -> (Vec<[i16; 2]>, u32) {
    let mut processor = Granular::new(BufferSize::FIRMWARE, port_quality(quality));
    processor.seed(SEED);
    let input = &input[..point_frames(quality)];
    let blocks = input.len() / MAX_BLOCK_SIZE;
    let mut out = vec![[0_i16; 2]; input.len()];
    for (block, (i, o)) in input.chunks(MAX_BLOCK_SIZE).zip(out.chunks_mut(MAX_BLOCK_SIZE)).enumerate() {
        processor.process(&port_parameters(&point.parameters(block, blocks)), i, o);
    }
    (out, processor.rng_state())
}

fn flat(frames: &[[i16; 2]]) -> Vec<i16> {
    frames.iter().flatten().copied().collect()
}

fn null_quality(quality: OracleQuality, gate: Gate) {
    let filter = std::env::var("GRANULAR_NULL_POINTS").ok();
    let input = input_signal(longest_point_frames());
    let bound = bound_dbfs();
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for point in points_in_mode(PlaybackMode::Granular).into_iter().filter(|p| p.gate == gate) {
        let (oracle, oracle_rng) = {
            let session = Session::lock();
            let out = render_point(&session, quality, &point, &input).unwrap();
            (out, session.rng_state())
        };
        let (port, port_rng) = render_port_point(quality, &point, &input);
        let r = residual_i16(&flat(&port), &flat(&oracle)).unwrap();
        let label = format!("{} {}", quality.label(), point.label());
        let first = port.iter().zip(&oracle).position(|(a, b)| a != b);
        if filter.as_deref().is_some_and(|f| label.contains(f)) {
            println!("{label}: {r}, first differing frame {first:?}, rng port {port_rng:08x} oracle {oracle_rng:08x}");
        }
        if !r.within(bound) || first.is_some() {
            let at = first.map(|i| format!(", first at frame {i} (port {:?}, oracle {:?})", port[i], oracle[i])).unwrap_or_default();
            failures.push(format!("{label}: {r}{at}"));
        } else if port_rng != oracle_rng {
            failures.push(format!("{label}: identical output but the random state differs (port {port_rng:08x}, oracle {oracle_rng:08x})"));
        }
        compared += 1;
    }
    assert_eq!(compared, 37);
    assert!(failures.is_empty(), "{} of {compared} points differ:\n{}", failures.len(), failures.join("\n"));
}

macro_rules! null_tests {
    ($($name:ident: $quality:ident, $gate:ident;)*) => {
        $(
            #[test]
            fn $name() {
                null_quality(OracleQuality::$quality, Gate::$gate);
            }
        )*
    };
}

null_tests! {
    stereo_free: Stereo, Free;
    stereo_freeze: Stereo, Freeze;
    stereo_trigger: Stereo, Trigger;
    mono_free: Mono, Free;
    mono_freeze: Mono, Freeze;
    mono_trigger: Mono, Trigger;
    stereo_lofi_free: StereoLowFidelity, Free;
    stereo_lofi_freeze: StereoLowFidelity, Freeze;
    stereo_lofi_trigger: StereoLowFidelity, Trigger;
    mono_lofi_free: MonoLowFidelity, Free;
    mono_lofi_freeze: MonoLowFidelity, Freeze;
    mono_lofi_trigger: MonoLowFidelity, Trigger;
}
