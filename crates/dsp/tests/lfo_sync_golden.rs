//! Pre-#253 renders (bbd3721b): every division, both devices, three unchanged meters.
#![allow(clippy::unwrap_used)]

use transit_dsp::Transport;

fn render(device: &str, division: usize, numerator: u32, denominator: u32, block: usize) -> u64 {
    let mut p = transit_dsp::create(device).unwrap();
    let settings: &[(&str, f32)] = if device == "filter" {
        &[("lfo_sync", 1.0), ("lfo_retrigger", 0.0), ("lfo_amount", 75.0), ("cutoff", 800.0)]
    } else {
        &[
            ("voices", 0.0),
            ("model", 8.0),
            ("lfo1_sync", 1.0),
            ("lfo1_retrigger", 0.0),
            ("lfo2_sync", 1.0),
            ("lfo2_retrigger", 0.0),
            ("lfo2_shape", 1.0),
            ("mod1_source", 1.0),
            ("mod1_dest", 6.0),
            ("mod1_amount", 50.0),
            ("mod2_source", 2.0),
            ("mod2_dest", 14.0),
            ("mod2_amount", 50.0),
        ]
    };
    for &(id, value) in settings {
        assert!(p.set_param(id, value));
    }
    for id in if device == "filter" { &["lfo_division"][..] } else { &["lfo1_division", "lfo2_division"][..] } {
        assert!(p.set_param(id, division as f32));
    }
    p.prepare(48_000.0, 512, 2);
    p.note_on(0, 60, 100);
    let bar_quarters = f64::from(numerator) * 4.0 / f64::from(denominator);
    let first = ((4.0 * bar_quarters + 1.0) * 24_000.0) as i64;
    let mut io = vec![vec![0.0; block]; 2];
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for at in (0..12_288).step_by(block) {
        let samples = first + at as i64;
        let quarters = samples as f64 / 24_000.0;
        let bar = (quarters / bar_quarters).floor() as i64;
        p.set_transport(&Transport {
            playing: true,
            tempo: 120.0,
            numerator,
            denominator,
            samples,
            quarters,
            bar,
            bar_start: bar as f64 * bar_quarters,
            ..Transport::default()
        });
        for channel in &mut io {
            for (i, value) in channel.iter_mut().enumerate() {
                *value = ((at + i) % 97) as f32 / 128.0 - 0.375;
            }
        }
        p.process(&mut io, block);
        for sample in io.iter().flatten() {
            for byte in sample.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    hash
}

#[test]
fn every_division_keeps_the_pre_253_bits_in_a_single_meter() {
    let mut got = Vec::new();
    for device in ["filter", "macro_oscillator"] {
        for (n, d) in [(4, 4), (3, 4), (7, 8)] {
            for division in 0..14 {
                for block in [64, 512] {
                    got.push(render(device, division, n, d, block));
                }
            }
        }
    }
    let expected: &[u64] = include!("fixtures/lfo_sync_pre_253.rs");
    assert_eq!(got, expected, "pre-#253 render bits");
}
