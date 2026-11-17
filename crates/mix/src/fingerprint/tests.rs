use super::*;

/// Samples with mixed bit patterns: signs, zeros, a subnormal, a NaN payload, infinities.
/// Nineteen of them: an odd run, so the record's zero-padded last word is covered.
const SAMPLES: [u32; 19] = [
    0x3f80_0000,
    0x8000_0000,
    0x0000_0001,
    0x7fc0_0001,
    0xbe99_999a,
    0x0000_0000,
    0x4049_0fdb,
    0x7f80_0000,
    0xff80_0000,
    0x3e80_0000,
    0x0000_0000,
    0x0000_0000,
    0x3f00_0000,
    0xbf00_0000,
    0x0080_0000,
    0x3c23_d70a,
    0x0000_0000,
    0xc2c8_0000,
    0x3f80_0000,
];

fn samples() -> Vec<f32> {
    SAMPLES.iter().map(|&b| f32::from_bits(b)).collect()
}

fn print_of(v: &[f32]) -> u64 {
    digest(|w| w.f32s(v))
}

/// Every single-bit change of every sample changes the print (the hasher's argument, through
/// the record's tag-6 run, exhaustively over this slice), and no two of the 608 changed slices
/// share a print.
#[test]
fn every_single_bit_of_every_sample_changes_the_print() {
    let base = samples();
    let mut seen = std::collections::HashSet::from([print_of(&base)]);
    for i in 0..base.len() {
        for bit in 0..32 {
            let mut v = base.clone();
            v[i] = f32::from_bits(v[i].to_bits() ^ (1 << bit));
            assert!(seen.insert(print_of(&v)), "sample {i}, bit {bit}: the print did not change, or collided");
        }
    }
}

/// The same for the hasher's byte path (kept for the `Hasher` contract), with a zero-padded tail.
#[test]
fn every_single_bit_of_a_byte_write_changes_the_print() {
    let bytes: Vec<u8> = (0u8..13).map(|b| b.wrapping_mul(37)).collect();
    let of = |b: &[u8]| {
        let mut h = PrintHasher::default();
        h.write(b);
        h.finish()
    };
    let mut seen = std::collections::HashSet::from([of(&bytes)]);
    for i in 0..bytes.len() {
        for bit in 0..8 {
            let mut b = bytes.clone();
            b[i] ^= 1 << bit;
            assert!(seen.insert(of(&b)), "byte {i}, bit {bit}: the print did not change, or collided");
        }
    }
    // A trailing zero byte is not the same input.
    let mut longer = bytes.clone();
    longer.push(0);
    assert_ne!(of(&bytes), of(&longer));
}

/// Negative control at the line level: a delay line that differs in one sample's lowest bit
/// has a different print; an idle line's contents are not part of its print (as before).
#[test]
fn a_delay_line_differing_in_one_sample_has_a_different_print() {
    let line = |live: bool, nudge: Option<(usize, usize)>| {
        let mut l = DelayLine::with_capacity(2, 61, 8);
        for (c, b) in l.bufs.iter_mut().enumerate() {
            for (k, s) in b.iter_mut().enumerate() {
                *s = (c * 100 + k) as f32 * 0.001;
            }
        }
        if let Some((c, k)) = nudge {
            l.bufs[c][k] = f32::from_bits(l.bufs[c][k].to_bits() ^ 1);
        }
        (l.live, l.idx, l.written) = (live, 5, 61);
        digest(|w| line_record(&l, w))
    };
    let base = line(true, None);
    for at in [(0, 0), (0, 7), (0, 68), (1, 0), (1, 30), (1, 34), (1, 68)] {
        assert_ne!(line(true, Some(at)), base, "sample {at:?}");
    }
    assert_eq!(line(false, Some((1, 34))), line(false, None), "an idle line hashes no contents");
}

/// The hash of a fixed record is a fixed function of its words, identical on every platform:
/// pinned over exact bit patterns only, no float arithmetic. This pins the hasher over the
/// record codec's words, not any strip's print (prints are compared only within one run). The
/// value was computed by a separate model of the codec's words and the steps in arbitrary-
/// precision integers, not captured from this code.
#[test]
fn the_hash_of_a_fixed_record_is_pinned() {
    let print = digest(|w| {
        w.u8(0xa5);
        w.u16(0x1234);
        w.u32(0xdead_beef);
        w.u64(0x0123_4567_89ab_cdef);
        w.usize(48_000);
        w.i32(-2);
        w.f32s(&samples());
        w.bytes(b"transit");
    });
    assert_eq!(print, 0xc2b3_fcae_9f47_d556, "the pinned hash of a fixed record");
}
