//! The reader's cost (ADR 0056, decision 8.8): a recorded wall-clock bench, not a CI gate (like
//! transit-math's bench and `plugin_bench`). A stereo source at 48 kHz read for one second of output
//! (48,000 frames) at each rate of the record's budget list; each figure is the median of five runs, as a
//! percentage of one core (the time to read one second of stereo output divided by one second), next
//! to the budget `max(1, q/2)` percent. The budget covers the whole clip path (read, gain, fades, mix);
//! this measures the read alone. Run in release: `cargo run --release -p transit-resample --example bench`.

use std::hint::black_box;
use std::time::Instant;

use transit_resample::{Interleaved, SincTable, read_frame};

const RATE: usize = 48_000;
const RUNS: usize = 5;

fn noise(n: usize) -> Vec<f32> {
    let mut s: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32) / (1u32 << 23) as f32 - 1.0
        })
        .collect()
}

fn main() {
    let t = SincTable::shared();
    // Enough source for one second of output at q = 32, plus the halo.
    let frames = 32 * RATE + 2048;
    let data = noise(2 * frames);
    let src = Interleaved::new(&data, 2);
    const RATES: [f64; 6] = [0.5, 1.059_463_094_359_295_3, 2.0, 4.0, 16.47, 32.0];
    println!("transit-resample bench (ADR 0056 decision 8.8), read alone, release build");
    println!(
        "settings: output rate {RATE} Hz, channels 2 (interleaved), one output frame per read_frame call, {RATE} frames (one second) per run, median of {RUNS} runs, positions x = 1024.25 + n·q, rates q {RATES:?}"
    );
    println!("{:<10} {:>10} {:>10} {:>10}", "q", "ns/frame", "% core", "budget %");
    for q in RATES {
        let mut runs = [0.0f64; RUNS];
        for run in &mut runs {
            let mut out = [0.0f32; 2];
            let mut sum = 0.0f32;
            let start = Instant::now();
            for n in 0..RATE {
                let x = 1024.25 + n as f64 * q;
                read_frame(t, &src, black_box(x), black_box(q), &mut out);
                sum += out[0] + out[1];
            }
            *run = start.elapsed().as_secs_f64();
            black_box(sum);
        }
        runs.sort_by(f64::total_cmp);
        let secs = runs[RUNS / 2];
        println!("{q:<10.4} {:>10.1} {:>10.3} {:>10.2}", secs * 1e9 / RATE as f64, secs * 100.0, (q / 2.0).max(1.0));
    }
}
