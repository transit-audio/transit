//! The modal voice oracle's residual floor against itself (#198 slice 2): the same sweep with the
//! same seed renders bit-identical output twice in one process and again in a second process.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes); the port
//! gate cannot be set until it is fixed. Never rerun it to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::modal_voice::{
    MAX_CONTROL, MAX_SPACE, MAX_STRIKE_TIMBRE, MODAL_VOICE_BLOCK_SIZE, MODAL_VOICE_SAMPLE_RATE, ModalVoiceModel, ModalVoicePatch, Performance,
};
use transit_macro_osc_oracle::modal_voice_sweep::{self, Configuration, center, points, render_point};
use transit_macro_osc_oracle::residual::residual_f32;
use transit_macro_osc_oracle::subset;

const CHILD_ENV: &str = "TRANSIT_MODAL_VOICE_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "modal-voice-digest ";

#[test]
fn the_firmware_constants_match() {
    let session = Session::lock();
    assert_eq!(session.modal_voice_max_block_size(), MODAL_VOICE_BLOCK_SIZE);
    assert_eq!(session.modal_voice_sample_rate().to_bits(), MODAL_VOICE_SAMPLE_RATE.to_bits());
}

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let mut compared = 0usize;
    let all = points();
    let kept = subset::strided("modal voice self floor", all.clone());
    for configuration in Configuration::ALL {
        for point in &kept {
            let a = render_point(&session, configuration, point).unwrap();
            let b = render_point(&session, configuration, point).unwrap();
            for (which, x, y) in [("main", &a.main, &b.main), ("aux", &a.aux, &b.aux)] {
                let r = residual_f32(x, y).unwrap();
                assert!(r.is_exact(), "{} {} {which}: {r}", configuration.label(), point.label());
            }
            compared += 1;
        }
    }
    assert_eq!(compared, Configuration::ALL.len() * kept.len());
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        modal_voice_sweep::digest_lines(&session).unwrap()
    };
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .args(["child_prints_the_sweep_digests", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "child failed ({}):\n{stdout}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
    // libtest prints `test <name> ... ` without a newline before a captured-off test's output, so
    // the first digest shares its line with that header: match the prefix anywhere in a line.
    let there: Vec<&str> = stdout.lines().filter_map(|l| l.find(LINE_PREFIX).map(|i| &l[i + LINE_PREFIX.len()..])).collect();
    assert_eq!(there.len(), here.len(), "the child printed {} digest lines, expected {}", there.len(), here.len());
    let differing: Vec<_> = here.iter().zip(&there).filter(|(a, b)| a.as_str() != **b).map(|(a, b)| format!("here  {a}\nthere {b}")).collect();
    assert!(differing.is_empty(), "{} of {} points differ between processes:\n{}", differing.len(), here.len(), differing.join("\n"));
}

/// The second process of `a_second_process_renders_the_same_bits`. Does nothing unless that test
/// started it.
#[test]
#[ignore = "run as a child process by a_second_process_renders_the_same_bits"]
fn child_prints_the_sweep_digests() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let session = Session::lock();
    for line in modal_voice_sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

fn render(session: &Session, configuration: Configuration, patch: &ModalVoicePatch, blocks: usize) -> Vec<f32> {
    let mut part = session.modal_voice().unwrap();
    match configuration {
        Configuration::Model(m) => part.set_model(m),
        Configuration::EasterEgg => part.set_easter_egg(true),
    }
    let silence = [0.0_f32; MODAL_VOICE_BLOCK_SIZE];
    let mut out = Vec::new();
    for block in 0..blocks {
        let (mut m, mut a) = ([0.0_f32; MODAL_VOICE_BLOCK_SIZE], [0.0_f32; MODAL_VOICE_BLOCK_SIZE]);
        let performance = Performance { gate: block < blocks / 2, note: 60.0, modulation: 0.0, strength: 0.5 };
        part.process(&performance, patch, &silence, &silence, &mut m, &mut a).unwrap();
        out.extend_from_slice(&m);
        out.extend_from_slice(&a);
    }
    out
}

/// The bow's flow noise and the blow's granular restarts draw from the shared random state, so the
/// seed must reach the output.
#[test]
fn the_seed_reaches_the_exciters() {
    let session = Session::lock();
    let mut patch = center();
    patch.strike_level = 0.0;
    let at = |seed: u32| {
        session.seed(seed);
        render(&session, Configuration::Model(ModalVoiceModel::Modal), &patch, 200)
    };
    let (a, b, c) = (at(7), at(7), at(8));
    assert!(residual_f32(&a, &b).unwrap().is_exact());
    assert!(!residual_f32(&a, &c).unwrap().is_exact(), "a different seed must change the bow and blow noise");
}

/// Every configuration sounds at the center (prints the peak level of each).
#[test]
fn every_configuration_sounds() {
    let session = Session::lock();
    let center = points().into_iter().find(|p| p.label() == "n60 center").unwrap();
    for configuration in Configuration::ALL {
        let r = render_point(&session, configuration, &center).unwrap();
        let peak = r.main.iter().chain(&r.aux).fold(0.0_f32, |m, s| m.max(s.abs()));
        println!("{}: peak {:.1} dBFS", configuration.label(), 20.0 * peak.log10());
        assert!(peak > 1e-3 && peak.is_finite(), "{}: peak {peak}", configuration.label());
    }
}

/// Inputs the C++ would index a table with out of bounds, or convert to an integer out of range,
/// are refused before the call (FFI invariant 4). The sweep's own extremes are accepted.
#[test]
fn out_of_range_inputs_are_refused_before_the_cxx() {
    use transit_macro_osc_oracle::Error;
    use transit_macro_osc_oracle::modal_voice::{MODULATION_RANGE, NOTE_RANGE};
    let session = Session::lock();
    let mut part = session.modal_voice().unwrap();
    let ok = Performance { gate: true, note: 60.0, modulation: 0.0, strength: 0.5 };
    let silence = [0.0_f32; MODAL_VOICE_BLOCK_SIZE];
    let (mut m, mut a) = ([0.0_f32; MODAL_VOICE_BLOCK_SIZE], [0.0_f32; MODAL_VOICE_BLOCK_SIZE]);
    let mut run = |p: &Performance, q: &ModalVoicePatch| part.process(p, q, &silence, &silence, &mut m, &mut a);

    assert_eq!(run(&ok, &center()), Ok(()));
    let top = ModalVoicePatch {
        envelope_shape: MAX_CONTROL,
        bow_level: MAX_CONTROL,
        bow_timbre: MAX_CONTROL,
        blow_level: MAX_CONTROL,
        blow_meta: MAX_CONTROL,
        blow_timbre: MAX_CONTROL,
        strike_level: MAX_CONTROL,
        strike_meta: MAX_CONTROL,
        strike_timbre: MAX_STRIKE_TIMBRE,
        geometry: MAX_CONTROL,
        brightness: MAX_CONTROL,
        damping: MAX_CONTROL,
        position: MAX_CONTROL,
        space: MAX_SPACE,
    };
    let edges = Performance { note: *NOTE_RANGE.end(), modulation: *MODULATION_RANGE.end(), strength: 1.0, ..ok };
    assert_eq!(run(&edges, &top), Ok(()));
    let low = Performance { note: *NOTE_RANGE.start(), modulation: *MODULATION_RANGE.start(), strength: 0.0, ..ok };
    assert_eq!(run(&low, &ModalVoicePatch { space: 0.0, ..center() }), Ok(()));

    let bad_performances = [
        Performance { note: 160.5, ..ok },
        Performance { note: -48.5, ..ok },
        Performance { modulation: 60.5, ..ok },
        Performance { modulation: -61.0, ..ok },
        Performance { strength: 1.01, ..ok },
        Performance { strength: -0.01, ..ok },
        Performance { note: f32::NAN, ..ok },
        Performance { modulation: f32::INFINITY, ..ok },
        Performance { strength: f32::NAN, ..ok },
    ];
    for p in bad_performances {
        assert_eq!(run(&p, &center()), Err(Error::BadParameter), "{p:?}");
    }
    let bad_patches = [
        // Exactly 1.0 would make `Interpolate` read one past `lut_stiffness` (geometry),
        // `lut_fm_frequency_quantizer` (the easter egg's metas) or `lut_detune_quantizer` (its bow
        // level).
        ModalVoicePatch { geometry: 1.0, ..center() },
        ModalVoicePatch { blow_meta: 0.9996, ..center() },
        ModalVoicePatch { bow_level: 1.0, ..center() },
        ModalVoicePatch { strike_timbre: 0.996, ..center() },
        ModalVoicePatch { space: 2.01, ..center() },
        ModalVoicePatch { brightness: -0.01, ..center() },
        ModalVoicePatch { damping: f32::NAN, ..center() },
        ModalVoicePatch { position: f32::INFINITY, ..center() },
    ];
    for q in bad_patches {
        assert_eq!(run(&ok, &q), Err(Error::BadParameter), "{q:?}");
    }
    // Audio input samples must be finite and within the codec's -1 to 1.
    for bad in [f32::NAN, f32::INFINITY, 1.5, -1.01] {
        let mut samples = [0.0_f32; MODAL_VOICE_BLOCK_SIZE];
        samples[5] = bad;
        let (mut m, mut a) = ([0.0_f32; MODAL_VOICE_BLOCK_SIZE], [0.0_f32; MODAL_VOICE_BLOCK_SIZE]);
        assert_eq!(part.process(&ok, &center(), &samples, &silence, &mut m, &mut a), Err(Error::BadParameter), "blow {bad}");
        assert_eq!(part.process(&ok, &center(), &silence, &samples, &mut m, &mut a), Err(Error::BadParameter), "strike {bad}");
    }
}
