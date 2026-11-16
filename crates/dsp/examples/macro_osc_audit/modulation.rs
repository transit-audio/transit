use super::*;
use analysis::{refine, shown};
use transit_dsp::plugins::macro_osc::voice::voice_gains;
use transit_instrument_core::{
    adsr::{Adsr, AdsrRates},
    lfo::{Lfo, Shape},
};

const CARRIER: f64 = 1237.3;
const RATES: [f32; 3] = [1.3, 11.3, 37.7];

fn params() -> Vec<(&'static str, f32)> {
    vec![
        ("harmonics", 100.0 * 30.0 / 128.0),
        ("timbre", 0.0),
        ("morph", 50.0),
        ("tune", (12.0 * m::log2(CARRIER / ideal(72))) as f32),
        ("shaper", 1.0),
        ("cutoff", 4000.0),
    ]
}

fn images(s: &Spectrum, carrier: f64, rate: f64) -> [f64; 4] {
    std::array::from_fn(|k| {
        let b = 4000.0 * (k + 1) as f64;
        let width = 2.0 * rate + 9.0 * s.bin;
        s.strongest(|hz| (hz - (carrier + b)).abs() <= width || (hz - (carrier - b).abs()).abs() <= width).1
    })
}

fn image_text(p: [f64; 4], reference: f64) -> String {
    p.map(|v| shown(v, reference)).join(" / ")
}

fn slope_text(previous: Option<(f32, [f64; 4])>, rate: f32, values: [f64; 4]) -> String {
    previous.map_or_else(
        || "n/a".into(),
        |(r, p)| {
            std::array::from_fn::<_, 4, _>(|k| {
                if p[k] <= 1e-13 || values[k] <= 1e-13 {
                    "below floor".into()
                } else {
                    format!("{:.2}", db(values[k] / p[k]) / m::log10(f64::from(rate / r)))
                }
            })
            .join(" / ")
        },
    )
}

// Same block-start phase recurrence as the public shaper; its within-block continuation is
// the per-sample reference. The public gain law and f32 multiply match the actual instrument.
fn external_lfo(base: &[f64], rate: f32, shape: Shape, dest: usize, skip: usize) -> (Vec<f64>, Vec<f64>) {
    let mut lfo = Lfo::new();
    lfo.retrigger();
    let inc = f64::from(rate) / 48000.0;
    let mut ideal = Vec::with_capacity(base.len());
    let mut held = Vec::with_capacity(base.len());
    for start in (0..skip + base.len()).step_by(12) {
        let fixed = lfo.value(shape) * 0.1;
        let mut sample_lfo = lfo;
        for i in 0..12 {
            let at = start + i;
            let v = sample_lfo.value(shape) * 0.1;
            let gain = |v| if dest == 6 { voice_gains(v, 0.0).0 } else { voice_gains(0.0, v).0 };
            if (skip..skip + base.len()).contains(&at) {
                let y = base[at - skip] as f32;
                ideal.push(f64::from(y * gain(v)));
                held.push(f64::from(y * gain(fixed)));
            }
            sample_lfo.advance(inc);
        }
        lfo.advance(inc * 12.0);
    }
    (ideal, held)
}

pub(super) fn run() -> Result<()> {
    println!("\n### 7. Modulation separated from the harmonic comb\n");
    println!(
        "FM ratio1 carrier 1237.3 Hz (Tune as printed in source), 48000 Hz, control rate 4000 Hz, sine LFO 1.3/11.3/37.7 Hz plus square 37.7 Hz. 1 s settling, N=524288. Pitch depth 1% (+/-0.24 semitone); other depths 10%. Harmonics/Morph use Timbre 20% for a nonzero effect. Image k=max local component around |carrier +/- k*4000|, width2*LFO+9 bins, k1..4. Slopes are dB/decade between adjacent sine rates, not assumed stepping classifications. Raw images can still contain intended modulation; the references below isolate update-interval effects."
    );
    println!(
        "\n| Destination | Shape | Rate Hz | Images 1/2/3/4 dBc | Growth 1/2/3/4 dB per decade | Baseline images dBc |\n|---|---|---:|---|---|---|"
    );
    for (dest, name) in [(1, "Pitch"), (2, "Harmonics"), (3, "Timbre"), (4, "Morph"), (6, "Level"), (14, "Pan left"), (7, "Cutoff")] {
        let mut base_params = params();
        if dest == 2 || dest == 4 {
            base_params.push(("timbre", 20.0));
        }
        let base = render(10, 48000, &[72], &base_params, 48000, MOD_N)?;
        let bs = Spectrum::new(&base, 48000)?;
        let carrier = refine(&base, 48000, bs.fundamental(CARRIER).0);
        let reference = bs.band(carrier, 8);
        let mut previous = None;
        for (shape, rate) in [(0, 1.3), (0, 11.3), (0, 37.7), (3, 37.7)] {
            let mut p = base_params.clone();
            p.extend([
                ("mod1_source", 1.0),
                ("mod1_dest", dest as f32),
                ("mod1_amount", if dest == 1 { 1.0 } else { 10.0 }),
                ("lfo1_rate", rate),
                ("lfo1_shape", shape as f32),
            ]);
            let x = render(10, 48000, &[72], &p, 48000, MOD_N)?;
            let s = Spectrum::new(&x, 48000)?;
            let im = images(&s, carrier, f64::from(rate));
            let ratios = im.map(|v| v / reference);
            println!(
                "| {name} | {} | {rate:.1} | {} | {} | {} |",
                if shape == 0 { "sine" } else { "square" },
                image_text(im, reference),
                if shape == 0 { slope_text(previous, rate, ratios) } else { "n/a".into() },
                image_text(images(&bs, carrier, f64::from(rate)), reference)
            );
            if shape == 0 {
                previous = Some((rate, ratios));
            }
        }
    }
    println!(
        "\nExact external Level/Pan reference: same unmodulated played patch times public voice_gains at each sample, with public Lfo and the same block-start phase recurrence. Held reconstruction must match the instrument; its max absolute sample error checks alignment and law. Difference actual-per-sample is the isolated held-gain error. Square's intended harmonics are included in the reference, so they cancel."
    );
    println!(
        "\n| Destination | Shape | Rate Hz | Held reconstruction max error dBFS | Held unequal samples | Difference RMS dBFS | Difference images 1/2/3/4 dBc | Growth dB/decade |\n|---|---|---:|---:|---:|---:|---|---|"
    );
    let base = render(10, 48000, &[72], &params(), 48000, MOD_N)?;
    let bs = Spectrum::new(&base, 48000)?;
    let carrier = refine(&base, 48000, bs.fundamental(CARRIER).0);
    let reference = bs.band(carrier, 8);
    for (dest, name) in [(6, "Level"), (14, "Pan left")] {
        let mut previous = None;
        for (shape, rate) in [(0, 1.3), (0, 11.3), (0, 37.7), (3, 37.7)] {
            let mut p = params();
            p.extend([("mod1_source", 1.0), ("mod1_dest", dest as f32), ("mod1_amount", 10.0), ("lfo1_rate", rate), ("lfo1_shape", shape as f32)]);
            let x = render(10, 48000, &[72], &p, 48000, MOD_N)?;
            let (ideal, held) = external_lfo(&base, rate, Shape::from_index(shape), dest, 48000);
            let max = x.iter().zip(&held).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
            let unequal = x.iter().zip(&held).filter(|(a, b)| a != b).count();
            let error: Vec<_> = x.iter().zip(&ideal).map(|(a, b)| a - b).collect();
            let es = Spectrum::new(&error, 48000)?;
            let im = images(&es, carrier, f64::from(rate));
            let ratios = im.map(|v| v / reference);
            println!(
                "| {name} | {} | {rate:.1} | {} | {unequal} | {} | {} | {} |",
                if shape == 0 { "sine" } else { "square" },
                shown(max * max, 1.0),
                shown(power(&error), 1.0),
                image_text(im, reference),
                if shape == 0 { slope_text(previous, rate, ratios) } else { "n/a".into() }
            );
            if shape == 0 {
                previous = Some((rate, ratios));
            }
        }
    }
    engine_controls()
}

fn power(x: &[f64]) -> f64 {
    x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64
}

fn engine_controls() -> Result<()> {
    println!(
        "\nOscillator engines only: 1-frame versus 12-frame public engine calls at 48000, parameter at call start. Both start from identical 12-frame warmup states (including modulation), avoiding unequal startup ramps. Default macros; carrier 1237.3; depth 0.1 for macros and 0.24 semitone for pitch; no voice/LPG/conversion/shaper. N=131072 after 1 s, rates 1.3/11.3/37.7. Error includes each engine's interpolators, frequency accumulation and per-call control smoothing (especially Wavetable); it is update-interval error, not solely a zero-order hold. No drums/speech/six-op per-call envelopes. Static difference RMS and image bands expose call-size drift even with constant controls. Modulation differences are not automatically classified as stepping when this baseline is material."
    );
    println!(
        "\n| Engine | Control | Hz | Static null RMS dBFS | Modulated difference RMS dBFS | Difference images dB/carrier | Growth dB/decade | Static difference images dB/carrier |\n|---|---|---:|---:|---:|---|---|---|"
    );
    for model in engines::TONAL.into_iter().chain([10]) {
        let note = (69.0 + 12.0 * m::log2(CARRIER / 440.0)) as f32;
        let base = |_| engines::parameters(48000, note, false);
        let a = engines::capture(model, 48000, 48000, N, 1, base)?;
        let b = engines::capture(model, 48000, 48000, N, 12, base)?;
        let null: Vec<_> = a.iter().zip(b).map(|(a, b)| a - b).collect();
        let ns = Spectrum::new(&null, 48000)?;
        let s = Spectrum::new(&a, 48000)?;
        let carrier = refine(&a, 48000, s.fundamental(CARRIER).0);
        let reference = s.band(carrier, 8);
        for control in ["Harmonics", "Timbre", "Morph", "Pitch"] {
            let mut previous = None;
            for rate in RATES {
                let parameter = |n: usize| {
                    let mut p = engines::parameters(48000, note, false);
                    let v = m::sin(TAU * f64::from(rate) * n as f64 / 48000.0) as f32;
                    match control {
                        "Harmonics" => p.harmonics += 0.1 * v,
                        "Timbre" => p.timbre += 0.1 * v,
                        "Morph" => p.morph += 0.1 * v,
                        _ => p.note += 0.24 * v,
                    }
                    p
                };
                let a = engines::capture(model, 48000, 48000, N, 1, parameter)?;
                let b = engines::capture(model, 48000, 48000, N, 12, parameter)?;
                let error: Vec<_> = a.iter().zip(b).map(|(a, b)| a - b).collect();
                let es = Spectrum::new(&error, 48000)?;
                let im = images(&es, carrier, f64::from(rate));
                let ratios = im.map(|v| v / reference);
                println!(
                    "| {} | {control} | {rate:.1} | {} | {} | {} | {} | {} |",
                    MODELS[model],
                    shown(power(&null), 1.0),
                    shown(power(&error), 1.0),
                    image_text(im, reference),
                    slope_text(previous, rate, ratios),
                    image_text(images(&ns, carrier, f64::from(rate)), reference)
                );
                previous = Some((rate, ratios));
            }
        }
    }
    Ok(())
}

pub(super) fn envelopes() -> Result<()> {
    println!("\n### 8. Envelopes after attack, isolated routes\n");
    println!(
        "All captures start 50 ms after note-on, past 10 ms ADSR attacks and the LPG attack; N=65536 at 48000. Filter uses the public Shaper design/filter_frame seam already in product: identical 0.25-peak 1237.3 Hz sine and per-sample envelopes; coefficients refreshed every 12 frames versus every frame. Cutoff 600 Hz, env amount0.5, decay 2000/6000 ms, sustain 0, amp held 1. Difference isolates coefficient update cadence; no new accessor. Matrix Level uses the identical public ADSR before each sample and voice_gains, compared with the played patch (shaper off, LPG held open, filter envelope source 4, amount 50%)."
    );
    println!(
        "\n| Route | Decay ms | Reference power dBFS | Difference RMS dBFS | Difference images 1/2/3/4 dB/reference | Held reconstruction max error dBFS |\n|---|---:|---:|---:|---|---:|"
    );
    for decay in [2000.0, 6000.0] {
        let a = filter_envelope(decay, 1);
        let b = filter_envelope(decay, 12);
        let error: Vec<_> = b.iter().zip(&a).map(|(b, a)| b - a).collect();
        let es = Spectrum::new(&error, 48000)?;
        println!(
            "| Filter envelope | {decay:.0} | {:.2} | {} | {} | n/a |",
            db(power(&a)),
            shown(power(&error), 1.0),
            image_text(images(&es, CARRIER, 20.0), power(&a))
        );
        let mut p = params();
        p.extend([("shaper", 0.0), ("filter_attack", 10.0), ("filter_decay", decay), ("filter_sustain", 0.0)]);
        let base = render(10, 48000, &[72], &p, 2400, 65536)?;
        p.extend([("mod1_source", 4.0), ("mod1_dest", 6.0), ("mod1_amount", 50.0)]);
        let x = render(10, 48000, &[72], &p, 2400, 65536)?;
        let rates = AdsrRates::new(10.0, decay, 0.0, 300.0, 48000.0);
        let mut env = Adsr::new();
        env.gate_on();
        let mut held = 0.0;
        let mut error = Vec::with_capacity(base.len());
        let mut maximum = 0.0_f64;
        for n in 0..2400 + base.len() {
            if n % 12 == 0 {
                held = env.level;
            }
            if n >= 2400 {
                let y = base[n - 2400] as f32;
                let exact = f64::from(y * voice_gains(env.level * 0.5, 0.0).0);
                let blocked = f64::from(y * voice_gains(held * 0.5, 0.0).0);
                error.push(x[n - 2400] - exact);
                maximum = maximum.max((x[n - 2400] - blocked).abs());
            }
            env.next(&rates);
        }
        let es = Spectrum::new(&error, 48000)?;
        println!(
            "| Envelope to Level | {decay:.0} | {:.2} | {} | {} | {} |",
            db(power(&base)),
            shown(power(&error), 1.0),
            image_text(images(&es, CARRIER, 20.0), power(&base)),
            shown(maximum * maximum, 1.0)
        );
    }
    lpg()
}

fn filter_envelope(decay: f32, block: usize) -> Vec<f64> {
    use transit_instrument_core::shaper::{FilterMods, Settings, Shaper};
    let settings = Settings {
        on: true,
        filter_env: AdsrRates::new(10.0, decay, 0.0, 300.0, 48000.0),
        env_amount: 0.5,
        cutoff_log2: transit_math::f32::log2(600.0),
        ..Settings::default()
    };
    let mut shaper = Shaper::new(0x21);
    shaper.note(&settings, 1.0, [0.0; 2]);
    let mut out = Vec::with_capacity(65536);
    for n in 0..2400 + 65536 {
        if n % block == 0 {
            shaper.design(&settings, 72.0, FilterMods::default());
        }
        let x = (0.25 * m::sin(TAU * CARRIER * n as f64 / 48000.0)) as f32;
        let y = shaper.filter_frame(&settings, true, x);
        if n >= 2400 {
            out.push(f64::from(y));
        }
    }
    out
}

fn lpg() -> Result<()> {
    use transit_dsp::plugins::macro_osc::to_f32;
    use transit_macro_osc::{
        SampleRate,
        voice::{Frame, Modulations, Patch, Voice},
    };
    println!(
        "\nLPG only through public Voice, FM ratio 1/no index, note 1237.3 Hz, decay 1, color 0/1; trigger patched, Level unpatched for ping, compared with same held Level 1. No shaper or matrix. Complex demodulation at known carrier: I=2*y*cos, Q=-2*y*sin. Sum I/Q spectral power near 4k/8k/12k/16k (width 20 Hz), divide by demodulated DC power. Baseline held render uses the same carrier and converter. Without a pre-conversion Voice seam, excess is envelope-associated structure (inferred stepping), not an exact interpolation-error null; quantization remains included."
    );
    println!("\n| LPG color | Demodulated images 1/2/3/4 dB/DC | Held images dB/DC | Status |\n|---:|---|---|---|");
    for color in [0.0, 1.0] {
        let render_voice = |ping: bool| {
            let mut voice = Voice::new(SampleRate::Hz48000);
            voice.init();
            voice.seed(0x21);
            let patch = Patch {
                engine: 10,
                note: (69.0 + 12.0 * m::log2(CARRIER / 440.0)) as f32,
                harmonics: 30.0 / 128.0,
                timbre: 0.0,
                morph: 0.5,
                decay: 1.0,
                lpg_colour: color,
                ..Patch::default()
            };
            let mods = Modulations { trigger: 1.0, trigger_patched: true, level: 1.0, level_patched: !ping, ..Modulations::default() };
            let mut frames = [Frame::default(); 12];
            let mut x = Vec::with_capacity(65536);
            for start in (0..2400 + 65536).step_by(12) {
                voice.render(&patch, &mods, &mut frames);
                for (i, f) in frames.iter().enumerate() {
                    if (2400..2400 + 65536).contains(&(start + i)) {
                        x.push(f64::from(to_f32(f.out)));
                    }
                }
            }
            x
        };
        let demod = |x: &[f64]| -> Result<[f64; 4]> {
            let i: Vec<_> = x.iter().enumerate().map(|(n, &y)| 2.0 * y * m::cos(TAU * CARRIER * (n + 2400) as f64 / 48000.0)).collect();
            let q: Vec<_> = x.iter().enumerate().map(|(n, &y)| -2.0 * y * m::sin(TAU * CARRIER * (n + 2400) as f64 / 48000.0)).collect();
            let si = Spectrum::new(&i, 48000)?;
            let sq = Spectrum::new(&q, 48000)?;
            let dc = si.band(0.0, 9) + sq.band(0.0, 9);
            Ok(std::array::from_fn(|k| {
                let f = 4000.0 * (k + 1) as f64;
                let sum =
                    |s: &Spectrum| s.power.iter().enumerate().filter(|(i, _)| (*i as f64 * s.bin - f).abs() < 20.0).map(|(_, p)| p).sum::<f64>();
                (sum(&si) + sum(&sq)) / dc
            }))
        };
        let ping = demod(&render_voice(true))?;
        let held = demod(&render_voice(false))?;
        println!("| {color:.0} | {} | {} | unresolved origin; envelope-associated structure only |", image_text(ping, 1.0), image_text(held, 1.0));
    }
    Ok(())
}
