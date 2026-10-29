//! Every engine of the oracle produces finite, non-silent output over the sweep (#194), and the
//! wrapper's constants match the firmware's.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::residual::to_dbfs;
use transit_macro_osc_oracle::sweep::{engine_points, render_engine_point, render_voice_point, voice_points};
use transit_macro_osc_oracle::{BLOCK_SIZE, ENGINE_COUNT, Error, MAX_BLOCK_SIZE, Model, SAMPLE_RATE, Session};

/// Quieter than this over a whole engine-level sweep counts as silent (about 0.0001 of full scale).
const SILENT_DBFS: f64 = -80.0;

#[test]
fn every_engine_renders() {
    let session = Session::lock();
    let mut report = Vec::new();
    for model in Model::ALL {
        let mut peak = 0.0_f64;
        for point in engine_points() {
            let r = render_engine_point(&session, model, &point).unwrap();
            for (which, buf) in [("out", &r.out), ("aux", &r.aux)] {
                let bad = buf.iter().position(|s| !s.is_finite());
                assert!(bad.is_none(), "{} engine {} {which}: non-finite sample at {bad:?}", model.label(), point.label());
            }
            peak = r.out.iter().chain(&r.aux).fold(peak, |p, s| p.max(f64::from(s.abs())));
        }
        let mut voice_peak = 0i32;
        for point in voice_points() {
            let frames = render_voice_point(&session, model, &point).unwrap();
            voice_peak = frames.iter().fold(voice_peak, |p, f| p.max(i32::from(f.out).abs()).max(i32::from(f.aux).abs()));
        }
        report.push(format!("{:24} engine peak {:7.2} dBFS, voice peak {voice_peak}", model.label(), to_dbfs(peak)));
        assert!(to_dbfs(peak) > SILENT_DBFS, "{} is silent at the engine level: peak {:.2} dBFS", model.label(), to_dbfs(peak));
        assert!(voice_peak > 0, "{} is silent at the voice level", model.label());
    }
    println!("{}", report.join("\n"));
}

#[test]
fn the_constants_match_the_firmware() {
    let session = Session::lock();
    assert_eq!(session.block_size(), BLOCK_SIZE);
    assert_eq!(session.max_block_size(), MAX_BLOCK_SIZE);
    assert_eq!(session.engine_count(), ENGINE_COUNT);
    assert_eq!(Model::ALL.len(), ENGINE_COUNT);
    for (i, m) in Model::ALL.iter().enumerate() {
        assert_eq!(m.index(), i);
        assert_eq!(Model::from_index(i), Some(*m));
    }
}

#[test]
fn only_48_khz_and_bounded_blocks_are_accepted() {
    let session = Session::lock();
    assert_eq!(session.voice(44_100.0).err(), Some(Error::UnsupportedSampleRate));
    let mut voice = session.voice(SAMPLE_RATE).unwrap();
    let patch = transit_macro_osc_oracle::Patch::centered(Model::VirtualAnalog);
    let mods = transit_macro_osc_oracle::Modulations::default();
    let mut too_long = vec![transit_macro_osc_oracle::Frame::default(); MAX_BLOCK_SIZE + 1];
    assert_eq!(voice.render(&patch, &mods, &mut too_long), Err(Error::BadBlock));
    // The firmware divides by the block size, so an empty block is rejected before the C++ runs.
    assert_eq!(voice.render(&patch, &mods, &mut []), Err(Error::BadBlock));
    let mut engine = session.engine(Model::VirtualAnalog).unwrap();
    let params = engine_points()[0].parameters(0, 1);
    let (mut out, mut aux) = (vec![0.0; BLOCK_SIZE], vec![0.0; BLOCK_SIZE + 1]);
    assert_eq!(engine.render(&params, &mut out, &mut aux), Err(Error::BadBlock));
    assert_eq!(engine.render(&params, &mut [], &mut []), Err(Error::BadBlock));
}

/// The flush-to-zero mode is switched only inside the C++ calls: Rust code in a session runs in
/// its default floating-point mode, so a denormal result survives right after a render.
#[test]
fn rust_keeps_its_default_fp_mode_during_a_session() {
    let session = Session::lock();
    let mut engine = session.engine(Model::VirtualAnalog).unwrap();
    let params = engine_points()[0].parameters(0, 1);
    let (mut out, mut aux) = (vec![0.0; BLOCK_SIZE], vec![0.0; BLOCK_SIZE]);
    engine.render(&params, &mut out, &mut aux).unwrap();
    let denormal = std::hint::black_box(f32::MIN_POSITIVE) * std::hint::black_box(0.5_f32);
    assert!(denormal != 0.0 && denormal.is_subnormal(), "a denormal result was flushed: {denormal:e}");
}
