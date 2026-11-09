//! ADR 0051 decision 4: the Resonator's Tonic and the Random Sequencer's Octave keep their stored
//! index and their sound; only the labels move to the UI's note namer. The Resonator's render at
//! the default Tonic (index 24) and at index 36, set the way a preset or automation sets it, is
//! pinned to hashes captured before the change (at the parent of the ADR 0051 label commit).
//!
//! The rendered path calls libm, whose last bit differs between MSVC's CRT, glibc and Apple's
//! libm, so the capture holds only on Windows (as `sampler_legacy_golden.rs`); the Windows CI job
//! enforces it. Never regenerate these from a new renderer.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use transit_dsp::Plugin;

const SR: f32 = 48_000.0;
const BLOCKS: [usize; 4] = [512, 37, 128, 300];

/// FNV-1a over the output's bits.
fn render(p: &mut dyn Plugin, frames: usize, ons: &[(usize, u8)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut io = vec![vec![0.0f32; 512]; 2];
    let (mut done, mut k, mut peak) = (0, 0, 0.0f32);
    while done < frames {
        let n = BLOCKS[k % BLOCKS.len()].min(frames - done);
        k += 1;
        for &(at, key) in ons {
            if (done..done + n).contains(&at) {
                p.note_on(at - done, key, 100);
            }
        }
        for c in &mut io {
            c.fill(0.0);
        }
        p.process(&mut io, n);
        for (l, r) in io[0].iter().zip(&io[1]).take(n) {
            for x in [*l, *r] {
                for b in x.to_bits().to_le_bytes() {
                    h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
                }
            }
            peak = peak.max(l.abs()).max(r.abs());
        }
        done += n;
    }
    assert!(peak > 0.001, "a golden of silence proves nothing");
    h
}

/// Sympathetic Strings at low Structure, where the strings drone at the tonic (it never moves the
/// played pitch, so the default model would not hear it).
fn resonator(tonic: Option<f32>) -> u64 {
    let mut p = transit_dsp::create("resonator").unwrap();
    assert!(p.set_param("model", 1.0) && p.set_param("structure", 10.0));
    if let Some(t) = tonic {
        assert!(p.set_param("tonic", t));
    }
    p.prepare(SR, 512, 2);
    render(p.as_mut(), 24_000, &[(0, 60), (6_000, 67), (12_000, 48)])
}

#[test]
fn the_resonator_sounds_the_same_at_its_stored_tonic_indices() {
    let default = resonator(None);
    assert_eq!(default, resonator(Some(24.0)), "the default is index 24");
    let c2 = resonator(Some(36.0));
    assert_ne!(default, c2, "the tonic is heard");
    assert_eq!((default, c2), (0x7239_19f8_f412_0fe0, 0x8444_832a_d820_8527), "golden: {default:#018x} {c2:#018x}");
}
