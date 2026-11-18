//! Offline signal measurements; run with `cargo run --release --locked -p transit-dsp --example macro_osc_audit`.
//! No audio/MIDI devices or real-time assertions; public engine/converter APIs only.
use analysis::Spectrum;
use legacy::bright_params;
use std::error::Error;
use std::f64::consts::TAU;
use transit_dsp::Plugin;
use transit_dsp::plugins::macro_osc::{MODELS, MacroOscillator};
use transit_math::{f64 as m, fft};
mod analysis;
mod attribution;
mod engines;
mod legacy;
mod modulation;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const HOST: usize = 128;
const N: usize = 131_072;
const MOD_N: usize = 524_288;
const KEYS: [u8; 5] = [24, 48, 72, 96, 127];
const REPRESENTATIVES: [usize; 3] = [8, 10, 19];

fn db(power: f64) -> f64 {
    10.0 * m::log10(power)
}

fn ideal(key: u8) -> f64 {
    440.0 * m::exp2((f64::from(key) - 69.0) / 12.0)
}

// Eligibility describes these two knob settings, not every sound the model can make.
fn exclusion(model: usize, bright: bool) -> Option<&'static str> {
    match MODELS[model] {
        "Filtered analog" if bright => Some("wave/sub mixture; subharmonics"),
        "Six-op FM 1" | "Six-op FM 2" | "Six-op FM 3" => Some("patch-dependent six-operator ratios, detuning and envelopes"),
        "String machine" | "Chiptune" | "Chord" => Some("multiple chord pitches / arpeggiation"),
        "FM" => Some("noninteger FM ratio (default 12.16 semitones); feedback at maximum"),
        "Speech" => Some("speech/formants with possible unvoiced content"),
        "Swarm" => Some("detuned stochastic grains"),
        "Filtered noise" | "Particle noise" | "Snare" | "Hi-hat" => Some("noise or inharmonic percussion"),
        "Inharmonic string" | "Modal" => Some("inharmonic physical modes"),
        "Bass drum" => Some("pitch envelope and nonlinear percussion"),
        _ => None,
    }
}

fn set(p: &mut MacroOscillator, name: &str, value: f32) -> Result<()> {
    if !p.set_param(name, value) {
        return Err(format!("unknown parameter {name}").into());
    }
    Ok(())
}

fn render(model: usize, sr: u32, keys: &[u8], extra: &[(&str, f32)], skip: usize, len: usize) -> Result<Vec<f64>> {
    let mut p = MacroOscillator::new();
    set(&mut p, "model", model as f32)?;
    // The audit's recorded captures are at level -6 dB, the default when it ran (#753 moved the
    // default to -12 dB); set explicitly so the record still describes what this program does.
    set(&mut p, "level", -6.0)?;
    for &(name, value) in extra {
        set(&mut p, name, value)?;
    }
    p.prepare(sr as f32, HOST, 2);
    for &key in keys {
        p.note_on(0, key, 127);
    }
    let mut io = vec![vec![0.0; HOST]; 2];
    let mut out = Vec::with_capacity(len);
    // Always drive full 128-frame calls, including the last call. Capture by sample index.
    for start in (0..skip + len).step_by(HOST) {
        for ch in &mut io {
            ch.fill(0.0);
        }
        p.process(&mut io, HOST);
        for (i, &v) in io[0].iter().enumerate() {
            if !v.is_finite() || !io[1][i].is_finite() {
                return Err(format!("nonfinite output: model {model}, rate {sr}, frame {}", start + i).into());
            }
            if (skip..skip + len).contains(&(start + i)) {
                out.push(f64::from(v));
            }
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    println!("## Second-pass program output\n");
    println!(
        "Measurement only. Fresh deterministic instrument; velocity 127, level -6 dB, main left output, 128-frame host calls. Static captures: 1 s settling, N=131072. No devices. Original first-pass probes remain in legacy.rs; their original output is preserved in the record, superseded where stated below."
    );
    println!(
        "Coverage: all 24 original keyboard models, five keys, both knob settings; eligible rows also at Tune +0.1. Four rate repeats retain Virtual analog, FM and Inharmonic string. Seven tonal engines at 1x/4x, all five keys and both settings; quantization at C4 at two pre-conversion levels. Eight oscillator engines in the control comparison. Only the strongest static non-harmonic component per row is reported. No runtime-driven coverage reduction. Headroom repeats the first-pass probe unchanged."
    );
    attribution::run()?;
    engines::references()?;
    engines::quantization()?;
    analysis::calibration()?;
    modulation::run()?;
    modulation::envelopes()?;
    println!("\n### 9. Additional host rates: native 88200 and non-native 50000\n");
    println!(
        "50000 uses the 48000 voice with rate_offset=12*log2(48000/50000); samples are clocked at the host rate. Pitch search centers account for the voice note cap before the offset. All other rates below are native."
    );
    for sr in [44100, 96000, 88200, 50000] {
        attribution::keyboard(&REPRESENTATIVES, sr)?;
    }
    legacy::headroom_section()?;
    Ok(())
}
