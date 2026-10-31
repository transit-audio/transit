//! Both public devices across a meter change: rendered audio, not only phase arithmetic.
#![allow(clippy::unwrap_used)]
use transit_dsp::Transport;

fn render(device: &str, division: usize, block: usize, change_meter: bool) -> Vec<f32> {
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
    let mut io = vec![vec![0.0; block]; 2];
    let mut out = Vec::new();
    // The bar-3 change is 1536 frames into playback: a boundary shared by 64, 512
    // and the 12-frame voice grid. This isolates #253 from snapshot timing (#254).
    for at in (0..13_824).step_by(block) {
        let samples = 192_000 - 1536 + at as i64;
        let quarters = samples as f64 / 24_000.0;
        let changed = change_meter && at >= 1536;
        let (numerator, bar, bar_start) = if changed {
            (3, 2, 8.0)
        } else if at >= 1536 {
            (4, 2, 8.0)
        } else {
            (4, 1, 4.0)
        };
        p.set_transport(&Transport {
            playing: true,
            tempo: 120.0,
            numerator,
            denominator: 4,
            samples,
            quarters,
            bar,
            bar_start,
            ..Transport::default()
        });
        for channel in &mut io {
            for (i, value) in channel.iter_mut().enumerate() {
                *value = ((at + i) % 97) as f32 / 128.0 - 0.375;
            }
        }
        p.process(&mut io, block);
        for (&left, &right) in io[0].iter().zip(&io[1]) {
            out.extend([left, right]);
        }
    }
    out
}

#[test]
fn synced_audio_agrees_within_a_millionth_across_a_meter_change() {
    for device in ["filter", "macro_oscillator"] {
        for division in 0..14 {
            let a = render(device, division, 64, true);
            let b = render(device, division, 512, true);
            assert!(a.iter().any(|x| x.abs() > 0.01), "{device}: audible output");
            assert!(a.iter().chain(&b).all(|x| x.is_finite()));
            let peak = a.iter().zip(&b).fold(0.0_f32, |p, (a, b)| p.max((a - b).abs()));
            assert!(peak <= 1e-6, "{device}, division {division}: peak residual {peak}");
        }
    }
}

#[test]
fn beat_divisions_render_the_same_bits_when_only_the_meter_changes() {
    for device in ["filter", "macro_oscillator"] {
        for division in 3..14 {
            for block in [64, 512] {
                let a = render(device, division, block, true);
                let b = render(device, division, block, false);
                assert!(a.iter().zip(&b).all(|(a, b)| a.to_bits() == b.to_bits()), "{device}, division {division}, block {block}");
            }
        }
    }
}
