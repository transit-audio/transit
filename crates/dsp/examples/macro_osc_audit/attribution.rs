use super::*;
use analysis::{FLOOR_DB, refine, shown};

#[derive(Clone, Copy)]
struct Fold {
    k: usize,
    m: usize,
    rate: f64,
    miss: f64,
}

pub(super) struct Ranked {
    model: usize,
    ratio: f64,
    text: String,
}

fn fold(hz: f64, f0: f64, rate: f64, bin: f64) -> Fold {
    let mut best = Fold { k: 1, m: 0, rate, miss: f64::INFINITY };
    for k in 1..=4096 {
        for m in 0..=4 {
            let miss = ((k as f64 * f0 - m as f64 * rate).abs() - hz).abs() / bin;
            if miss < best.miss {
                best = Fold { k, m, rate, miss };
            }
        }
    }
    best
}

fn label(f: Fold) -> String {
    format!("{},{},{:.3}", f.k, f.m, f.miss)
}

fn peak_near(s: &Spectrum, hz: f64) -> (f64, f64) {
    let i = s.at(hz);
    (i.saturating_sub(2).max(1)..=(i + 2).min(s.power.len() - 2))
        .filter(|&j| s.power[j] >= s.power[j - 1] && s.power[j] >= s.power[j + 1])
        .map(|j| (s.frequency(j), s.band(j as f64 * s.bin, 8)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((f64::NAN, 0.0))
}

// Every component row (including drive) passes through the same attribution and level policy.
#[allow(clippy::too_many_arguments)]
fn component(
    model: usize,
    sr: u32,
    f0: f64,
    reference: f64,
    s: &Spectrum,
    shifted: &Spectrum,
    shifted_f0: f64,
    hz: f64,
    power: f64,
) -> (String, bool) {
    let host = fold(hz, f0, f64::from(sr), s.bin);
    let multiple = match model {
        1 | 5 => 2,
        10 => 4,
        _ => 1,
    };
    let internal = fold(hz, f0, f64::from(sr * multiple), s.bin);
    let chosen = if internal.miss <= host.miss + 1e-8 { internal } else { host };
    let strongest = s.strongest_harmonic(f0);
    let h = m::round(hz / f0).max(1.0);
    let nearby = s.band(h * f0, 8);
    let floor = power <= strongest * 1e-13;
    let class = if floor && (hz - h * f0).abs() <= 9.0 * s.bin && db(power / nearby) <= FLOOR_DB {
        "window floor"
    } else if floor {
        "unresolved (below floor)"
    } else if chosen.miss <= 0.25 {
        if chosen.m == 0 { "partial" } else { "alias (comb)" }
    } else {
        "unresolved"
    };
    let df = shifted_f0 - f0;
    let predicted = (chosen.k as f64 * shifted_f0 - chosen.m as f64 * chosen.rate).abs();
    let (moved, p) = peak_near(shifted, predicted);
    let (partial, pp) = peak_near(shifted, hz + h * df);
    let can_move = df.abs() > 0.05 * s.bin && !floor;
    let match_alias = can_move && chosen.m > 0 && chosen.miss <= 0.25 && (moved - predicted).abs() <= 0.5 * s.bin && p > power * 0.01;
    let match_partial = can_move && (partial - (hz + h * df)).abs() <= 0.5 * s.bin && pp > power * 0.01;
    let (shift_class, observed) = if match_alias && !match_partial {
        ("alias (shift)", moved)
    } else if match_partial && !match_alias {
        ("partial (shift)", partial)
    } else {
        ("unresolved", moved)
    };
    let motion = if can_move && observed.is_finite() {
        format!("{observed:.4}; {:.2}; {:.3}", (observed - hz) / df, (observed - predicted) / s.bin)
    } else {
        "unresolved (no resolved move)".into()
    };
    let levels = if floor {
        "below floor | below floor | below floor".into()
    } else {
        format!("{} | {:.2} | {}", shown(power, reference), db(power), shown(power, s.tonal(f0)))
    };
    let location = if floor { "below floor".into() } else { format!("{hz:.4}") };
    (
        format!(
            "{location} | {levels} | {} | {} | {class} | {shifted_f0:.4} | {motion} | {shift_class}",
            if floor { "n/a".into() } else { label(host) },
            if floor { "n/a".into() } else { label(internal) }
        ),
        match_alias && !match_partial,
    )
}

pub(super) fn keyboard(models: &[usize], sr: u32) -> Result<Vec<Ranked>> {
    let mut ranked = Vec::new();
    println!("\n### Keyboard attribution, {sr} Hz\n");
    println!(
        "| Model | Knobs | Key | f0 Hz | Cents | Fundamental dBFS | Pitch status | Component Hz | dBc | dBFS | dB/harmonic power | Host k,m,miss bins | Internal k,m,miss bins | Comb class | Shift f0 Hz | Shift component Hz; slope; miss bins | Shift class |\n|---|---|---:|---:|---:|---:|---|---:|---:|---:|---:|---|---|---|---:|---|---|"
    );
    for &model in models {
        for bright in [false, true] {
            for key in KEYS {
                let mut params = bright_params(bright);
                let x = render(model, sr, &[key], &params, sr as usize, N)?;
                let s = Spectrum::new(&x, sr)?;
                let cap = ideal(120) * if sr == 50000 { 50000.0 / 48000.0 } else { 1.0 };
                let center = ideal(key).min(cap);
                let center = if model == 11 { center.min(f64::from(sr) / 8.0) } else { center };
                let (guess, reference) = s.fundamental(center);
                let eligible = exclusion(model, bright).is_none();
                let edge = s.edge(center);
                let weak = s.weak_reference(reference);
                let f0 = if eligible && !weak && !edge { refine(&x, sr, guess) } else { guess };
                let status = if edge {
                    "search edge; not pitch"
                } else if weak {
                    "weak; not pitch"
                } else if !eligible {
                    "nominal-band proxy"
                } else if key > 120 || (model == 11 && ideal(key) > f64::from(sr) / 8.0) {
                    "clamp"
                } else {
                    "located"
                };
                // Render even eligible rows without a usable fundamental. Their shift cannot
                // support attribution, so it stays explicitly unresolved instead of a proxy.
                let shifted = if eligible {
                    params.push(("tune", 0.1));
                    Some(render(model, sr, &[key], &params, sr as usize, N)?)
                } else {
                    None
                };
                let tail = if let Some(y) = shifted.filter(|_| !weak && !edge) {
                    let t = Spectrum::new(&y, sr)?;
                    let f1 = refine(&y, sr, t.fundamental(f0).0);
                    let (hz, power) = s.strongest(|hz| !s.harmonic(hz, f0, 9.0));
                    let (text, confirmed) = component(model, sr, f0, reference, &s, &t, f1, hz, power);
                    if confirmed {
                        ranked.push(Ranked {
                            model,
                            ratio: power / s.tonal(f0),
                            text: format!(
                                "| {} | {} | {key} | {f0:.4} | {hz:.4} | {:.2} | {:.2} | {:.2} | alias (comb + shift) |",
                                MODELS[model],
                                if bright { "maximum" } else { "default" },
                                db(power / reference),
                                db(power),
                                db(power / s.tonal(f0))
                            ),
                        });
                    }
                    text
                } else {
                    format!(
                        "n/a | n/a | n/a | n/a | n/a | n/a | unresolved: {} | n/a | n/a | unresolved",
                        exclusion(model, bright).unwrap_or("weak or search-edge reference")
                    )
                };
                println!(
                    "| {} | {} | {key} | {f0:.4} | {:.3} | {} | {status} | {tail} |",
                    MODELS[model],
                    if bright { "maximum" } else { "default" },
                    1200.0 * m::log2(f0 / ideal(key)),
                    shown(reference, 1.0)
                );
            }
        }
    }
    Ok(ranked)
}

pub(super) fn run() -> Result<()> {
    println!("\n### 1-3. Fold arithmetic, pitch movement, and absolute levels\n");
    println!(
        "All original keyboard rows retained. K<=4096, m=0..4; nearest |k*f0-m*rate|, miss in FFT bins. Internal rate=2*host for Phase distortion AND Wave terrain, 4*host for FM, host for others. These equivalent combs cannot uniquely identify the generation stage. A <=0.25-bin match is partial for m=0, alias (comb) for m>0, conditional on this static patch being periodic. Window floor: within 9 bins of a harmonic and <=-130 dB relative to it; anything below -130 dB relative to the strongest harmonic is below floor. Others unresolved. dBFS uses RMS power; harmonic power sums the guarded harmonic bins through Nyquist, excludes DC. Weak/inharmonic/search-edge rows carry no component claim."
    );
    println!(
        "Tune +0.1 semitone is rendered for every eligible row, including weak/edge rows (unresolved). Pair a local maximum within 2 bins of the comb prediction, accept <=0.5-bin miss and no more than 20 dB loss. Also test the nearest integer partial's movement. Both matches, no resolved df0, or neither match => unresolved. Slope=(component move)/(f0 move), signed. A comb match alone is arithmetic evidence, not proof of its origin; a shift agreement strengthens it. Close/spurious pairings remain possible. Same guard and window in both captures."
    );
    let mut rows = keyboard(&(0..MODELS.len()).collect::<Vec<_>>(), 48000)?;
    rows.sort_by(|a, b| b.ratio.total_cmp(&a.ratio));
    println!(
        "\nRanked per-model maxima at 48000 Hz, among components supported by BOTH comb and shift, by component/harmonic power. The strongest component per row only; not total alias power or an audibility ranking. Clamped rows without pitch motion and excluded models are unranked.\n"
    );
    println!(
        "| Model | Knobs | Key | f0 Hz | Component Hz | dBc | dBFS | dB/harmonic power | Classification |\n|---|---|---:|---:|---:|---:|---:|---:|---|"
    );
    let mut seen = [false; 24];
    for row in rows {
        if !seen[row.model] {
            println!("{}", row.text);
            seen[row.model] = true;
        }
    }
    println!("\n### Drive attribution, FM ratio 1 sine, shaper on, 48000 Hz\n");
    println!(
        "| Key | Drive dB | f0 Hz | Component Hz | dBc | dBFS | dB/harmonic power | Host k,m,miss bins | Internal k,m,miss bins | Comb class | Shift f0 Hz | Shift component Hz; slope; miss bins | Shift class |\n|---:|---:|---:|---:|---:|---:|---:|---|---|---|---:|---|---|"
    );
    for key in [72, 96] {
        for drive in [0.0, 24.0] {
            let mut params = vec![("harmonics", 100.0 * 30.0 / 128.0), ("timbre", 0.0), ("morph", 50.0), ("shaper", 1.0), ("drive", drive)];
            let x = render(10, 48000, &[key], &params, 48000, N)?;
            params.push(("tune", 0.1));
            let y = render(10, 48000, &[key], &params, 48000, N)?;
            let s = Spectrum::new(&x, 48000)?;
            let t = Spectrum::new(&y, 48000)?;
            let f0 = refine(&x, 48000, s.fundamental(ideal(key)).0);
            let f1 = refine(&y, 48000, t.fundamental(ideal(key)).0);
            let (hz, p) = s.strongest(|hz| !s.harmonic(hz, f0, 9.0));
            println!("| {key} | {drive:.0} | {f0:.4} | {} |", component(10, 48000, f0, s.band(f0, 8), &s, &t, f1, hz, p).0);
        }
    }
    Ok(())
}
