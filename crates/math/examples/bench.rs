//! Speed of each seam function against the platform's (ADR 0055, 6.1): a recorded wall-clock bench, not
//! a CI gate (like `plugin_bench`). Each figure is the median of five runs, in nanoseconds per call over
//! 4,096 arguments in the function's audio-range domain. Run in release:
//! `cargo run --release -p transit-math --example bench`.

use std::hint::black_box;
use std::time::Instant;

use transit_math::consts::{DB_TO_LOG2, LOG2_TO_DB};
use transit_math::gain;
use transit_math::{f32 as m32, f64 as m64};

const N: usize = 4096;
const REPS: usize = 64;
const RUNS: usize = 5;

fn lin32(lo: f32, hi: f32) -> Vec<f32> {
    (0..N).map(|i| lo + (hi - lo) * (i as f32 / (N - 1) as f32)).collect()
}

fn lin64(lo: f64, hi: f64) -> Vec<f64> {
    (0..N).map(|i| lo + (hi - lo) * (i as f64 / (N - 1) as f64)).collect()
}

/// Median of five runs of `REPS` passes over `args`, in nanoseconds per call.
fn time<T: Copy, R>(args: &[T], f: impl Fn(T) -> R) -> f64 {
    let mut runs = [0.0f64; RUNS];
    for run in &mut runs {
        let t = Instant::now();
        for _ in 0..REPS {
            for &a in args {
                black_box(f(black_box(a)));
            }
        }
        *run = t.elapsed().as_nanos() as f64 / (REPS * args.len()) as f64;
    }
    runs.sort_by(f64::total_cmp);
    runs[RUNS / 2]
}

fn row(name: &str, seam: f64, platform: f64) {
    println!("{name:<28} {seam:>8.2} {platform:>10.2} {:>8.2}x", seam / platform);
}

fn main() {
    println!("{:<28} {:>8} {:>10} {:>9}", "function (ns per call)", "seam", "platform", "ratio");
    let trig32 = lin32(-10.0, 10.0);
    let exp32 = lin32(-20.0, 20.0);
    let pos32 = lin32(1.0e-4, 100.0);
    row("f32 sin", time(&trig32, m32::sin), time(&trig32, f32::sin));
    row("f32 cos", time(&trig32, m32::cos), time(&trig32, f32::cos));
    row("f32 tan", time(&trig32, m32::tan), time(&trig32, f32::tan));
    row("f32 exp", time(&exp32, m32::exp), time(&exp32, f32::exp));
    row("f32 exp2", time(&exp32, m32::exp2), time(&exp32, f32::exp2));
    row("f32 ln", time(&pos32, m32::ln), time(&pos32, f32::ln));
    row("f32 log2", time(&pos32, m32::log2), time(&pos32, f32::log2));
    row("f32 log10", time(&pos32, m32::log10), time(&pos32, f32::log10));
    row("f32 powf (x, 0.37)", time(&pos32, |x| m32::powf(x, 0.37)), time(&pos32, |x| x.powf(0.37)));
    row("f32 tanh", time(&exp32, m32::tanh), time(&exp32, f32::tanh));
    row("f32 atan2 (y, 0.5)", time(&trig32, |y| m32::atan2(y, 0.5)), time(&trig32, |y| y.atan2(0.5)));

    let trig64 = lin64(-10.0, 10.0);
    let exp64 = lin64(-20.0, 20.0);
    let pos64 = lin64(1.0e-4, 100.0);
    row("f64 sin", time(&trig64, m64::sin), time(&trig64, f64::sin));
    row("f64 cos", time(&trig64, m64::cos), time(&trig64, f64::cos));
    row("f64 tan", time(&trig64, m64::tan), time(&trig64, f64::tan));
    row("f64 exp", time(&exp64, m64::exp), time(&exp64, f64::exp));
    row("f64 exp2", time(&exp64, m64::exp2), time(&exp64, f64::exp2));
    row("f64 ln", time(&pos64, m64::ln), time(&pos64, f64::ln));
    row("f64 log2", time(&pos64, m64::log2), time(&pos64, f64::log2));
    row("f64 log10", time(&pos64, m64::log10), time(&pos64, f64::log10));
    row("f64 pow (x, 0.37)", time(&pos64, |x| m64::pow(x, 0.37)), time(&pos64, |x| x.powf(0.37)));
    row("f64 atan2 (y, 0.5)", time(&trig64, |y| m64::atan2(y, 0.5)), time(&trig64, |y| y.atan2(0.5)));

    // The gain conversions: the exact f64 path against today's platform form, and the pre-authorized
    // f32 fast form (exp2f and log2f of an f32 product, ADR 0055 6.1; not built in this slice).
    let db = lin32(-143.0, 48.0);
    let g: Vec<f32> = db.iter().map(|&d| gain::db_to_gain(d)).collect();
    let today_db_to_gain = |d: f32| 10f32.powf(d.min(200.0) / 20.0);
    let today_gain_to_db = |g: f32| (20.0 * g.abs().log10()).max(-144.0);
    row("db_to_gain (vs today)", time(&db, gain::db_to_gain), time(&db, today_db_to_gain));
    row("db_to_gain f32 fast form", time(&db, |d| m32::exp2(d * DB_TO_LOG2)), time(&db, today_db_to_gain));
    row("gain_to_db (vs today)", time(&g, gain::gain_to_db), time(&g, today_gain_to_db));
    row("gain_to_db f32 fast form", time(&g, |x| (LOG2_TO_DB * m32::log2(x.abs())).max(-144.0)), time(&g, today_gain_to_db));
    let pans = lin32(-0.99, 0.99);
    let today_pan = |p: f32| {
        let theta = (p + 1.0) * std::f32::consts::FRAC_PI_4;
        let e = -3.0 / -3.010_3;
        (theta.cos().max(0.0).powf(e), theta.sin().max(0.0).powf(e))
    };
    row("pan_gains -3 dB (vs today)", time(&pans, |p| gain::pan_gains(p, -3.0)), time(&pans, today_pan));
}
