use super::*;
use analysis::{refine, shown};
use transit_dsp::plugins::macro_osc::to_f32;
use transit_macro_osc::engine::{Engine, EngineParameters, TriggerState};
use transit_macro_osc::utils::random::Random;
use transit_macro_osc::voice::to_i16;

pub(super) const TONAL: [usize; 7] = [0, 1, 5, 8, 9, 11, 13];

pub(super) fn make(model: usize) -> Result<Box<dyn Engine>> {
    use transit_macro_osc::{engine as e, engine2 as e2};
    Ok(match model {
        0 => Box::new(e2::virtual_analog_vcf_engine::VirtualAnalogVcfEngine::new()),
        1 => Box::new(e2::phase_distortion_engine::PhaseDistortionEngine::new(24)),
        5 => Box::new(e2::wave_terrain_engine::WaveTerrainEngine::new(24)),
        8 => Box::new(e::virtual_analog_engine::VirtualAnalogEngine::new(24)),
        9 => Box::new(e::waveshaping_engine::WaveshapingEngine::new()),
        10 => Box::new(e::fm_engine::FmEngine::new()),
        11 => Box::new(e::grain_engine::GrainEngine::new()),
        13 => Box::new(e::wavetable_engine::WavetableEngine::new()),
        _ => return Err(format!("no oscillator renderer for {}", MODELS[model]).into()),
    })
}

pub(super) fn parameters(sr: u32, key: f32, bright: bool) -> EngineParameters {
    let v = if bright { 1.0 } else { 0.5 };
    EngineParameters { note: key, harmonics: v, timbre: v, morph: v, accent: 1.0, a0_normalized: 55.0 / sr as f32, trigger: TriggerState::UNPATCHED }
}

pub(super) fn capture(model: usize, sr: u32, skip: usize, len: usize, block: usize, params: impl Fn(usize) -> EngineParameters) -> Result<Vec<f64>> {
    let mut e = make(model)?;
    e.init(sr as f32);
    e.reset();
    let mut rng = Random::new(0x21);
    rng.seed(0x21);
    let mut out = [0.0; 24];
    let mut aux = [0.0; 24];
    let mut enveloped = false;
    let mut result = Vec::with_capacity(len);
    // Both call sizes enter the capture from the identical 12-frame warmup state.
    // Otherwise the initial frequency ramp leaves an arbitrary permanent phase offset.
    for (start, block) in (0..skip).step_by(12).map(|n| (n, 12)).chain((skip..skip + len).step_by(block).map(|n| (n, block))) {
        e.render(&params(start), &mut rng, &mut out[..block], &mut aux[..block], &mut enveloped);
        for (i, &y) in out[..block].iter().enumerate() {
            if !y.is_finite() {
                return Err("nonfinite engine output".into());
            }
            if (skip..skip + len).contains(&(start + i)) {
                result.push(f64::from(y));
            }
        }
    }
    Ok(result)
}

fn invalid(model: usize, key: u8, bright: bool) -> Option<&'static str> {
    let f = ideal(key) / 48000.0;
    match model {
        0 if bright => Some("subharmonics and cutoff cap"),
        0 if f * 8.0 >= 0.25 => Some("cutoff >=0.25"),
        1 => Some("amount*(1-3.8*modulator_f) changes at every nonzero pitch; modulator/pw caps may also bind"),
        5 => Some("radius attenuation 1-8*f0 changes at every nonzero pitch"),
        8 if bright && (f * 16.0 >= 0.25 || f * 4.003 >= 0.25 || 0.005 < 2.0 * f * 4.003) => Some("sync/aux frequency or pulse-width cap"),
        8 if !bright && f >= 0.25 => Some("oscillator cap"),
        9 if f * (if bright { 5.5 } else { 3.0 }) > 0.5 / 16.0 || f * (if bright { 44.0 } else { 9.0 }) > 0.5 / 12.0 => {
            Some("tame attenuates shape or folder amount")
        }
        9 if bright && 0.95 > 1.0 - 2.0 * f => Some("slope pulse-width cap"),
        11 => Some("carrier_shape depends on f0; carrier/formant caps may also bind"),
        13 if 128.0 * f >= 1.0 => Some("differentiator cutoff reaches 1"),
        _ => None,
    }
}

fn binding(model: usize, key: u8, bright: bool) -> String {
    use transit_macro_osc::{
        resources::fm::LUT_FM_FREQUENCY_QUANTIZER,
        utils::{interpolate, units::semitones_to_ratio},
    };
    let f = ideal(key) / 48000.0;
    let v = if bright { 1.0 } else { 0.5 };
    let bound = |yes| if yes { "binds" } else { "clear" };
    match model {
        0 => format!("f0={f:.6}; cutoff {}", bound(f * m::exp2((v - 0.2) * 10.0) >= 0.25)),
        1 => {
            let ratio = f64::from(semitones_to_ratio(interpolate(&LUT_FM_FREQUENCY_QUANTIZER, v as f32, 128.0)));
            let fm = (0.5 * f * ratio).min(0.25);
            let fm4 = (0.125 * f * ratio).min(0.25);
            format!(
                "mod cap {}; pw cap {}; amount 1x={:.4},4x={:.4}",
                bound(0.5 * f * ratio >= 0.25),
                bound(0.5 + 0.49 * v > 1.0 - 2.0 * fm),
                8.0 * v * v * (1.0 - 3.8 * fm),
                8.0 * v * v * (1.0 - 3.8 * fm4)
            )
        }
        5 => format!(
            "attenuation zero {}; radius 1x={:.4},4x={:.4}",
            bound(f >= 0.125),
            0.1 + 0.9 * v * (1.0 - 8.0 * f).max(0.0) * (2.0 - (1.0 - 8.0 * f).max(0.0)),
            0.1 + 0.9 * v * (1.0 - 2.0 * f).max(0.0) * (2.0 - (1.0 - 2.0 * f).max(0.0))
        ),
        8 => format!(
            "sync cap {}; aux cap {}; saw pw cap {}",
            bound(f * if bright { 16.0 } else { 1.0 } >= 0.25),
            bound(f * if bright { 4.00231 } else { 1.0 } >= 0.25),
            bound(bright && 0.005 < 2.0 * f * 4.00231)
        ),
        9 => {
            let slope = 3.0 + (v - 0.5_f64).abs() * 5.0;
            let tame = |h: f64, order: f64| (1.0 - (f * h - 0.5 / order) / (0.5 - 0.5 / order)).clamp(0.0, 1.0).powi(3);
            let shape = tame(slope, 16.0);
            let folder = tame(slope * (3.0 + (v - 0.5).abs() * 2.0 * shape * 5.0), 12.0);
            format!(
                "osc cap {}; pw cap {}; shape attenuation={shape:.4}; folder attenuation={folder:.4}",
                bound(f >= 0.25),
                bound(0.5 + 0.45 * v > 1.0 - 2.0 * f)
            )
        }
        11 => format!(
            "carrier cap {}; formant cap {}; shape attenuation 1x={:.4},4x={:.4}",
            bound(f >= 0.125),
            bound(ideal(if bright { 108 } else { 66 }) / 48000.0 * if bright { 4.0 } else { 1.0 } >= 0.25),
            (1.0 - 24.0 * f).max(0.0),
            (1.0 - 6.0 * f).max(0.0)
        ),
        _ => format!("differentiator cutoff {}; static coordinate LP inactive", bound(128.0 * f >= 1.0)),
    }
}

pub(super) fn references() -> Result<()> {
    println!("\n### 4. Engine float reference, normalized-frequency limits first\n");
    println!(
        "Source inventory (normalized to engine rate unless explicitly internal). Inherited limits from the original module; no constants changed. Both rates are always rendered. No valid reference means no alias-power difference is claimed. Stable engine controls, LPG/limiter/conversion excluded; 12-frame calls, 1 s settle, 1x N=131072 at 48000 and 4x N=524288 at 192000. Equal duration and bin width; FFT maximum 2^19. Analyze 20..20000 Hz directly at each rate; no crude decimator added. The reference is a spectral comparison, not a phase null or proof that 4x is alias-free."
    );
    println!("\n| Engine | Limits and continuous rate dependencies |\n|---|---|");
    for (model, text) in [
        (
            0,
            "oscillator/master <=0.25; pw in [2*f,1-2*f]; cutoff=min(f0*2^((timbre-.2)*10),.25). Default cutoff binds at f0>=.03125; maximum includes sub at .501*f0.",
        ),
        (
            1,
            "2x internal render, sample-pair average; internal f0=.5*engine_f0; modulator=min(.25,f0*ratio); oscillator <=.25, pw limited by frequency; amount=8*timbre^2*(1-3.8*modulator_f), changes at all nonzero pitches, even below cap.",
        ),
        (
            5,
            "2x internal render, sample-pair average; radius=.1+.9*timbre*a*(2-a), a=max(1-8*f0,0), reaches zero attenuation at f0=.125. All nonzero timbres change sound with rate.",
        ),
        (
            8,
            "variable-shape and variable-saw frequencies <=.25; pw in [2*f,1-2*f] (saw applies below pw=.5). Maximum sync=16*f0; detuned saw about 4.00231*f0, pw=.005. Default has no detuning.",
        ),
        (
            9,
            "oscillator frequency [1e-6,.25], pw in [2*f,1-2*f]; tame uses max_f=.5/order, orders16 and12, cube(clamp(1-(f0*harmonics-max_f)/(.5-max_f),0,1)); shape/folder rolloff starts before oscillator clamp.",
        ),
        (
            11,
            "grainlet carrier<=.125, formant<=.25; shape=.33+(morph-.33)*max(1-24*f0,0); formant1=note(24+84*timbre), formant2=formant1*2^(-2+4*harmonics); DC blocker cutoff=.3*f0. AUX has separate oscillator limits, not analyzed.",
        ),
        (
            13,
            "differentiator cutoff=min(128*f0,1); gain=(.95-f0)/(131072*f0); coordinate LP clamp [0.01,0.1], inactive after static coordinates settle. Continuous gain is normalized by harmonic power; cutoff saturation invalidates reference.",
        ),
    ] {
        println!("| {} | {text} |", MODELS[model]);
    }
    println!(
        "Global engine note_to_frequency clamps note to [-119,136]; the Voice clamps the combined note to120 first. Wave terrain quadrature also caps its internal frequency at0.25. FM (used for drive/control probes, outside this seven-engine reference set) renders at4x through an eight-tap symmetric FIR downsampler. Binding details below are evaluated at1x; a binding1x cap need not bind at4x. Continuous changes alone can invalidate a reference even when every hard cap is clear."
    );
    println!(
        "\n| Engine | Knobs | Requested key / engine key | 1x residual dB/harmonics | 4x residual dB/harmonics | Excess 1x power dB/harmonics | Validity | 1x binding detail |\n|---|---|---|---:|---:|---:|---|---|"
    );
    for model in TONAL {
        for bright in [false, true] {
            for key in KEYS {
                let effective = key.min(120);
                let x = capture(model, 48000, 48000, N, 12, |_| parameters(48000, f32::from(effective), bright))?;
                let y = capture(model, 192000, 192000, N * 4, 12, |_| parameters(192000, f32::from(effective), bright))?;
                let s = Spectrum::new(&x, 48000)?;
                let t = Spectrum::new(&y, 192000)?;
                let center = if model == 11 { ideal(effective).min(6000.0) } else { ideal(effective) };
                let f0 = refine(&x, 48000, s.fundamental(center).0);
                let f1 = refine(&y, 192000, t.fundamental(ideal(effective)).0);
                let residual = |s: &Spectrum, f: f64| {
                    s.power
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            let hz = *i as f64 * s.bin;
                            (20.0..20000.0).contains(&hz) && !s.harmonic(hz, f, 9.0)
                        })
                        .map(|(_, p)| p)
                        .sum::<f64>()
                        / s.tonal(f)
                };
                let a = residual(&s, f0);
                let b = residual(&t, f1);
                let pitch_ok = !s.edge(center) && !s.weak_reference(s.band(f0, 8));
                let reason = if !pitch_ok || t.edge(ideal(effective)) || t.weak_reference(t.band(f1, 8)) {
                    Some("weak/search-edge fundamental")
                } else {
                    invalid(model, effective, bright)
                };
                let (four, excess) = if reason.is_none() {
                    (shown(b, 1.0), shown((a - b).max(0.0), 1.0))
                } else {
                    ("no valid reference".into(), "no valid reference".into())
                };
                println!(
                    "| {} | {} | {key}/{effective} | {} | {four} | {excess} | {} | {} |",
                    MODELS[model],
                    if bright { "maximum" } else { "default" },
                    if pitch_ok { shown(a, 1.0) } else { "unresolved fundamental".into() },
                    reason.unwrap_or("valid for static spectral comparison; excess is inferred alias power, not isolated error"),
                    binding(model, effective, bright)
                );
            }
        }
    }
    Ok(())
}

pub(super) fn quantization() -> Result<()> {
    println!("\n### 5. Quantization isolated before the Level knob\n");
    println!(
        "Default C4 engine float y; documented OUT gains [1,.7,.7,.8,.7,.7,.6]; LPG bypassed, no limiter for these positive-gain engines. Multiply y by pre-level and OUT gain * -32767 in f32 exactly as postprocessor, ideal=-post/32768 (includes converter scale), actual=real to_f32(to_i16(post)), error=actual-ideal. Report unwindowed total error RMS; error spectrum partitions DC, harmonic guards, and every remaining bin through Nyquist. At two pre-conversion levels: 0 and -40 dB. H1 error ratio uses the ideal fundamental. Clipped samples are counted; only zero-clipping rows isolate truncation. This is not the private Voice float after its live LPG; an accessor would be required for that exact signal, and none is added."
    );
    println!(
        "\n| Engine | Pre-level dB | Ideal H1 dBFS | Error RMS dBFS | Error harmonic dBFS | Error off-harmonic dBFS | Error DC dBFS | Error H1 dBc | Clipped samples |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
    for (model, gain) in [(0, 1.0), (1, 0.7), (5, 0.7), (8, 0.8), (9, 0.7), (11, 0.7), (13, 0.6)] {
        let x = capture(model, 48000, 48000, N, 12, |_| parameters(48000, 60.0, false))?;
        for (level, pre) in [(0, 1.0), (-40, 0.01)] {
            let mut ideal_signal = Vec::with_capacity(N);
            let mut error = Vec::with_capacity(N);
            let mut clipped = 0;
            for &y in &x {
                let post = (y as f32 * pre) * (gain * -32767.0);
                let ideal = -f64::from(post) / 32768.0;
                let integer = to_i16(post);
                clipped += usize::from(1.0 + post.trunc() < -32768.0 || 1.0 + post.trunc() > 32767.0);
                ideal_signal.push(ideal);
                error.push(f64::from(to_f32(integer)) - ideal);
            }
            let s = Spectrum::new(&ideal_signal, 48000)?;
            let e = Spectrum::new(&error, 48000)?;
            let f0 = refine(&ideal_signal, 48000, s.fundamental(ideal(60)).0);
            let rms = error.iter().map(|v| v * v).sum::<f64>() / N as f64;
            let harmonic = e.tonal(f0);
            let off = e.residual(f0, 9.0);
            let dc = e.power[..10].iter().sum::<f64>();
            println!(
                "| {} | {level} | {:.2} | {} | {} | {} | {} | {} | {clipped} |",
                MODELS[model],
                db(s.band(f0, 8)),
                shown(rms, 1.0),
                shown(harmonic, 1.0),
                shown(off, 1.0),
                shown(dc, 1.0),
                shown(e.band(f0, 8), s.band(f0, 8))
            );
        }
    }
    Ok(())
}
