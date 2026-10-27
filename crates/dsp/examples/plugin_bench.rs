//! Per-plugin realtime cost: `cargo run --release -p transit-dsp --example plugin_bench`
use std::time::Instant;

fn main() {
    let sr = 48_000.0;
    let secs = 10usize;
    for info in transit_dsp::plugins() {
        let Some(mut p) = transit_dsp::create(info.id) else { continue };
        p.prepare(sr, 512, 2);
        let mut io = vec![vec![0.0f32; 512]; 2];
        let t0 = Instant::now();
        let mut ph = 0.0f32;
        for _ in 0..(secs * 48_000 / 512) {
            for c in io.iter_mut() {
                for x in c.iter_mut() {
                    ph += 0.031;
                    *x = ph.sin() * 0.3;
                }
            }
            if info.is_instrument {
                p.note_on(0, 60, 100);
            }
            p.process(&mut io, 512);
        }
        let el = t0.elapsed().as_secs_f64();
        println!("{:<20} {:>7.3}% of one core (stereo, 48 kHz)", info.id, el / secs as f64 * 100.0);
    }
}
