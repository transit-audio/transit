//! The wavetable terrains (5 to 7) at the edges of the path's domain (#520).

use super::*;
use crate::engine::EngineParameters;

/// Terrain 5 reads the last bank: at x = 1.0 its second wave started at the table's end.
#[test]
fn the_last_bank_at_x_one_reads_inside_the_table() {
    for &x in &[1.0f32, 1.000_000_1, 1.003] {
        assert!(terrain_lookup_wt(x, 0.0, 2).is_finite(), "x {x}");
    }
}

/// Every bank, at the top of the x axis and past it, at every edge of the y axis: the lookup
/// stays inside the 192-wave table and returns a finite value.
#[test]
fn the_wavetable_terrain_lookup_stays_inside_the_table_at_the_path_edges() {
    let xs = [0.999_999_9f32, 1.0, 1.000_000_1, 1.003, 1.01, 1.031_75, 1.05, 2.0, 1.0e9, f32::INFINITY];
    let ys = [-1.0f32, -0.5, 0.0, 0.5, 1.0, 1.03, 1.031_25, 1.05, 2.0, 1.0e9, f32::INFINITY];
    for bank in 0..3 {
        for &x in &xs {
            for &y in &ys {
                let v = terrain_lookup_wt(x, y, bank);
                assert!(v.is_finite() || !x.is_finite() || !y.is_finite(), "bank {bank} x {x} y {y}: {v}");
            }
        }
    }
}

/// The lookup as ported before #520, returning `None` where it read past the table.
fn unpatched_lookup(x: f32, y: f32, bank: i32) -> Option<f32> {
    let sample = (y + 1.0) * 0.5 * 128.0;
    let wt = (x + 1.0) * 0.5 * 63.0;
    let sample_integral = sample as usize;
    let sample_fractional = sample - (sample_integral as f32);
    let wt_integral = wt as usize;
    let wt_fractional = wt - (wt_integral as f32);
    let read = |start: usize| -> Option<f32> {
        let w = WAV_INTEGRATED_WAVES.get(start..)?;
        let a = f32::from(*w.get(sample_integral)?);
        let b = f32::from(*w.get(sample_integral + 1)?);
        let c = f32::from(*w.get(sample_integral + 2)?);
        Some((b - a) + (c - b - b + a) * sample_fractional)
    };
    let start = (bank as usize * 64 + wt_integral) * 132;
    let xy0 = read(start)?;
    let xy1 = read(start + 132)?;
    Some((xy0 + (xy1 - xy0) * wt_fractional) * (1.0 / 1024.0))
}

/// Wherever the firmware's read stays inside the table (the next bank's first wave past the
/// top of banks 0 and 1, the next wave's first samples past the top of the y axis), the
/// patched lookup is the unpatched one bit for bit; past the table it is finite.
#[test]
fn the_patch_changes_only_reads_past_the_table() {
    let (mut same, mut patched) = (0, 0);
    for bank in 0..3 {
        for i in -1400..=1400 {
            let x = i as f32 / 1000.0;
            for j in -140..=140 {
                let y = j as f32 / 100.0 + 0.003;
                let v = terrain_lookup_wt(x, y, bank);
                match unpatched_lookup(x, y, bank) {
                    Some(u) => {
                        assert_eq!(v.to_bits(), u.to_bits(), "bank {bank} x {x} y {y}");
                        same += 1;
                    }
                    None => {
                        assert!(v.is_finite(), "bank {bank} x {x} y {y}");
                        patched += 1;
                    }
                }
            }
        }
    }
    assert!(patched > 0 && same > patched, "{same} unchanged, {patched} patched");
}

/// Below the top of the x axis nothing changes: the lookup there is the firmware's arithmetic
/// (the oracle nulls pin it; this pins the edge itself). At x = 1.0 exactly the firmware reads
/// the last wave at a zero fraction, which is the last wave; past it, terrain 5 (the last bank)
/// repeats its last wave.
#[test]
fn the_last_bank_past_its_top_edge_is_its_last_wave() {
    let bank = 2;
    for &y in &[-1.0f32, -0.3, 0.0, 0.7, 1.0] {
        let at_top = terrain_lookup_wt(1.0, y, bank);
        for &x in &[1.000_000_1f32, 1.02, 1.05, 1.5] {
            let past = terrain_lookup_wt(x, y, bank);
            assert_eq!(at_top.to_bits(), past.to_bits(), "y {y} x {x}: past the top repeats the last wave");
        }
        let below = terrain_lookup_wt(0.999_999_9, y, bank);
        assert!((below - at_top).abs() < 1.0e-3, "y {y}: continuous at the top ({below} vs {at_top})");
    }
}

/// Negative and NaN coordinates: `as usize` saturates a negative or NaN coordinate to index 0,
/// so the lookup reads the first wave (or sample) and extrapolates by the negative fraction;
/// NaN propagates to the result. Neither indexes outside the table.
#[test]
fn negative_and_nan_coordinates_index_the_first_wave() {
    for bank in 0..3 {
        for &(x, y) in &[(-1.0f32, 0.0f32), (-1.003, 0.0), (-2.0, -2.0), (0.0, -1.02)] {
            assert!(terrain_lookup_wt(x, y, bank).is_finite(), "bank {bank} x {x} y {y}");
        }
        assert!(terrain_lookup_wt(f32::NAN, 0.0, bank).is_nan());
        assert!(terrain_lookup_wt(0.0, f32::NAN, bank).is_nan());
    }
}

/// The path coordinates one block reaches, computed exactly as `render` computes them from the
/// engine's own path buffers and offset ramp. Returns (max x, max |y|).
fn block_coordinates(engine: &mut WaveTerrainEngine<'_>, p: &EngineParameters, n: usize) -> (f32, f32) {
    let mut offset_state = engine.offset;
    let offset = SimpleParameterInterpolator::new(offset_state, 1.9 * p.morph - 1.0, n);
    let mut out = [0.0f32; 24];
    let mut aux = [0.0f32; 24];
    let mut flag = false;
    engine.render(p, &mut Random::default(), &mut out[..n], &mut aux[..n], &mut flag);
    let (mut max_x, mut max_y) = (f32::MIN, 0.0f32);
    for i in 0..n {
        let x_offset = offset.update(&mut offset_state);
        for k in 0..2 {
            let x = engine.temp_buffer_1[2 * i + k] * (1.0 - f32::abs(x_offset)) + x_offset;
            max_x = max_x.max(x);
            max_y = max_y.max(engine.temp_buffer_2[2 * i + k].abs());
        }
    }
    (max_x, max_y)
}

/// The largest x and |y| the path reaches when the key of block `b` is `key(b)`, the engine
/// fed as the instrument feeds it: concert tuning, and the voice's note the mean of this block's
/// key and the last one's (`Voice::render`; the first block after power-on averages with 0).
/// Harmonics 0 keeps the render on terrain 0, so the search itself never takes the table read
/// (the path and the offset do not depend on the terrain).
fn reach(key: impl Fn(usize) -> f32, timbre: f32, morph: f32, blocks: usize) -> (f32, f32) {
    let mut engine = WaveTerrainEngine::new(crate::engine::MAX_BLOCK_SIZE);
    engine.init(48_000.0);
    let a0 = crate::A0 * crate::Tuning::Concert.pitch_scale();
    let mut p = EngineParameters { a0_normalized: a0, timbre, morph, harmonics: 0.0, ..Default::default() };
    let (mut max_x, mut max_y) = (f32::MIN, 0.0f32);
    let mut previous = 0.0;
    for b in 0..blocks {
        let k = key(b);
        p.note = (k + previous) * 0.5;
        previous = k;
        let (x, y) = block_coordinates(&mut engine, &p, 12);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (max_x, max_y)
}

/// Evidence for #520, not a gate: prints how far ordinary playing pushes the path. Run with
/// `--nocapture` to read it.
#[test]
fn how_far_ordinary_playing_pushes_the_path() {
    let (x, y) = reach(|b| if b < 11 { 84.0 } else { 36.0 }, 1.0, 1.0, 2000);
    println!("issue case (84 then 36 at block 11, Timbre 100, Morph 100): max x {x:.9} max |y| {y:.9}");

    // One note change: every pair, switched at each of 48 blocks (the path's phase at the change).
    let mut best = (f32::MIN, 0, 0, 0);
    let mut best_y = (0.0f32, 0, 0, 0);
    for hi in (24..=108).step_by(6) {
        for lo in (0..=96).step_by(6) {
            if lo >= hi {
                continue;
            }
            for switch in 1..=48 {
                let (x, y) = reach(|b| if b < switch { hi as f32 } else { lo as f32 }, 1.0, 1.0, 200);
                if x > best.0 {
                    best = (x, hi, lo, switch);
                }
                if y > best_y.0 {
                    best_y = (y, hi, lo, switch);
                }
            }
        }
    }
    println!(
        "one change, Timbre 100, Morph 100: max x {:.9} (notes {} then {} at block {}); max |y| {:.9} (notes {} then {} at block {})",
        best.0, best.1, best.2, best.3, best_y.0, best_y.1, best_y.2, best_y.3
    );

    // A legato random melody (a note every 3 to 40 blocks over notes 0 to 108), 120 s each.
    for &timbre in &[0.9f32, 0.95, 1.0] {
        for seed in 1..=4u32 {
            let blocks = 120 * 4000;
            let mut notes = Vec::with_capacity(blocks);
            let mut s = seed.wrapping_mul(2_654_435_761);
            while notes.len() < blocks {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let len = 3 + (s >> 8) as usize % 38;
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let note = ((s >> 8) % 109) as f32;
                notes.extend(std::iter::repeat_n(note, len));
            }
            let (x, y) = reach(|b| notes[b], timbre, 1.0, blocks);
            println!("random melody seed {seed}, Timbre {timbre}, Morph 100, 120 s: max x {x:.9} max |y| {y:.9}");
        }
    }

    // Vibrato: a slow pitch wobble changes the path's step every block.
    for &base in &[0.0f32, 12.0, 24.0, 36.0, 48.0] {
        for &depth in &[0.5f32, 2.0, 12.0] {
            let (x, y) = reach(|b| base + depth * (b as f32 * 0.02).sin(), 1.0, 1.0, 40_000);
            println!("vibrato around {base} by {depth}: max x {x:.9} max |y| {y:.9}");
        }
    }
}
