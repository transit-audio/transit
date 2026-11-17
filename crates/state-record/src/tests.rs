//! Round trips, the encoding's known words, and every way a restore fails.

extern crate std;

use super::*;
use std::vec::Vec;

/// A sink that keeps every word (the transplant gate's lossless record).
#[derive(Default)]
struct Words(Vec<u64>);

impl Hasher for Words {
    fn write(&mut self, _: &[u8]) {
        panic!("the writer emits words only");
    }
    fn write_u64(&mut self, w: u64) {
        self.0.push(w);
    }
    fn finish(&self) -> u64 {
        0
    }
}

fn words(f: impl FnOnce(&mut Writer<'_>)) -> Vec<u64> {
    let mut sink = Words::default();
    let mut w = Writer::new(&mut sink);
    f(&mut w);
    let n = w.words();
    assert_eq!(n as usize, sink.0.len());
    sink.0
}

const CONFIG: Config<'static> = Config { entry: "fixture", sample_rate: 48_000.0, max_block: 256, channels: 2, content: 0 };

/// Every item, written then read back, with the reader exhausted at the end.
#[test]
fn every_item_round_trips() {
    let floats = [1.5f32, -0.0, f32::MIN_POSITIVE, 1.0e-42, -3.25];
    let bytes = *b"state record";
    let ints = [0u64, 1, u64::MAX, 1 << 56];
    let rec = words(|w| {
        w.u64(u64::MAX);
        w.u32(7);
        w.u8(255);
        w.usize(48_000);
        w.i64(i64::MIN);
        w.i32(-2);
        w.bool(true);
        w.bool(false);
        w.f32(-0.0);
        w.f64(core::f64::consts::PI);
        w.f32s(&floats);
        w.f32s(&floats[..4]);
        w.f32s(&[]);
        w.bytes(&bytes);
        w.bytes(&bytes[..8]);
        w.u64s(&ints);
        w.present(Some(0.25f32), |w, v| w.f32(v));
        w.present(None::<f32>, |w, v| w.f32(v));
        w.child(|w| {
            w.u16(3);
            w.child(|w| w.i8(-1));
        });
    });
    let mut r = Reader::new(&rec);
    assert_eq!(r.u64(), Some(u64::MAX));
    assert_eq!(r.u32(), Some(7));
    assert_eq!(r.u8(), Some(255));
    assert_eq!(r.usize(), Some(48_000));
    assert_eq!(r.i64(), Some(i64::MIN));
    assert_eq!(r.i32(), Some(-2));
    assert_eq!(r.bool(), Some(true));
    assert_eq!(r.bool(), Some(false));
    assert_eq!(r.f32().map(f32::to_bits), Some((-0.0f32).to_bits()));
    assert_eq!(r.f64(), Some(core::f64::consts::PI));
    let mut f = [0.0f32; 5];
    assert_eq!(r.f32s_len(), Some(5));
    r.f32s(&mut f).unwrap();
    assert_eq!(f.map(f32::to_bits), floats.map(f32::to_bits));
    let mut f4 = [0.0f32; 4];
    r.f32s(&mut f4).unwrap();
    assert_eq!(f4.map(f32::to_bits), [1.5f32, -0.0, f32::MIN_POSITIVE, 1.0e-42].map(f32::to_bits));
    r.f32s(&mut []).unwrap();
    let mut b = [0u8; 12];
    r.bytes(&mut b).unwrap();
    assert_eq!(&b, b"state record");
    let mut b8 = [0u8; 8];
    r.bytes(&mut b8).unwrap();
    assert_eq!(&b8, b"state re");
    let mut u = [0u64; 4];
    r.u64s(&mut u).unwrap();
    assert_eq!(u, ints);
    assert_eq!(r.present(Reader::f32), Some(Some(0.25)));
    assert_eq!(r.present(Reader::f32), Some(None));
    assert_eq!(r.child(|r| Some((r.u16()?, r.child(Reader::i8)?))), Some((3, -1)));
    assert!(r.finish());
}

/// The words themselves, worked out by hand from the table (no float arithmetic: bit patterns
/// only), so the encoding is the same on every platform.
#[test]
fn the_encoding_of_fixed_items_is_known() {
    let rec = words(|w| {
        w.u8(0xa5);
        w.i16(-2);
        w.bool(true);
        w.f32(1.0);
        w.f64(-2.0);
        w.f32s(&[1.0, -0.0, 0.5]);
        w.bytes(b"transit!x");
        w.uints(2, [9u64, 10].into_iter());
        w.child(|w| w.u64(1));
    });
    let expected: [u64; 22] = [
        0x0100_0000_0000_0001,
        0xa5,
        0x0200_0000_0000_0001,
        0xffff_ffff_ffff_fffe,
        0x0300_0000_0000_0001,
        1,
        0x0400_0000_0000_0001,
        0x3f80_0000,
        0x0500_0000_0000_0001,
        0xc000_0000_0000_0000,
        0x0600_0000_0000_0003,
        0x8000_0000_3f80_0000,
        0x3f00_0000,
        0x0700_0000_0000_0009,
        u64::from_le_bytes(*b"transit!"),
        u64::from(b'x'),
        0x0800_0000_0000_0002,
        9,
        10,
        0x0900_0000_0000_0002,
        0x0100_0000_0000_0001,
        1,
    ];
    assert_eq!(rec.as_slice(), &expected);
}

/// A child's count is measured by a counting pass that never visits a run's values.
#[test]
fn the_counting_pass_matches_the_written_words() {
    let big = std::vec![0.5f32; 100_001];
    let mut c = Writer::counter();
    c.child(|w| {
        w.f32s(&big);
        w.bytes(&[1, 2, 3]);
        w.u64s(&[1, 2]);
    });
    let rec = words(|w| {
        w.child(|w| {
            w.f32s(&big);
            w.bytes(&[1, 2, 3]);
            w.u64s(&[1, 2]);
        })
    });
    assert_eq!(c.words() as usize, rec.len());
    assert_eq!(rec.len(), 1 + 1 + 50_001 + 1 + 1 + 1 + 2);
}

#[test]
fn a_complete_record_carries_its_configuration_and_checks_it() {
    let rec = words(|w| w.record(&CONFIG, |w| w.u64(42)));
    assert_eq!(Reader::new(&rec).record(&CONFIG, Reader::u64), Some(42));
    for other in [
        Config { entry: "fixturf", ..CONFIG },
        Config { entry: "fixture2", ..CONFIG },
        Config { sample_rate: 44_100.0, ..CONFIG },
        Config { max_block: 128, ..CONFIG },
        Config { channels: 1, ..CONFIG },
        Config { content: 1, ..CONFIG },
    ] {
        assert_eq!(Reader::new(&rec).record(&other, Reader::u64), None, "{other:?}");
    }
    // An empty state payload is still a record with its configuration child.
    let empty = words(|w| w.record(&CONFIG, |_| {}));
    assert_eq!(Reader::new(&empty).record(&CONFIG, |_| Some(())), Some(()));
}

#[test]
fn a_restore_fails_on_every_listed_mismatch() {
    let one = words(|w| w.u64(300));
    // A tag other than the one expected.
    assert_eq!(Reader::new(&one).i64(), None);
    assert_eq!(Reader::new(&one).bool(), None);
    // A narrowed integer that does not fit.
    assert_eq!(Reader::new(&one).u8(), None);
    assert_eq!(Reader::new(&words(|w| w.i64(-129))).i8(), None);
    assert_eq!(Reader::new(&words(|w| w.i64(-128))).i8(), Some(-128));
    // A short stream.
    assert_eq!(Reader::new(&one[..1]).u64(), None);
    assert_eq!(Reader::new(&[]).u64(), None);
    let run = words(|w| w.f32s(&[1.0, 2.0, 3.0]));
    assert_eq!(Reader::new(&run[..2]).f32s(&mut [0.0; 3]), None);
    // A count other than the one expected.
    assert_eq!(Reader::new(&run).f32s(&mut [0.0; 2]), None);
    assert_eq!(Reader::new(&run).f32s(&mut [0.0; 4]), None);
    // Nonzero padding: an odd f32 run's high half, a byte run's tail.
    let mut bad = run.clone();
    bad[2] |= 1 << 32;
    assert_eq!(Reader::new(&bad).f32s(&mut [0.0; 3]), None);
    let mut bytes = words(|w| w.bytes(&[1, 2, 3]));
    bytes[1] |= 1 << 40;
    assert_eq!(Reader::new(&bytes).bytes(&mut [0; 3]), None);
    // An f32 scalar whose high half is set.
    let mut f = words(|w| w.f32(1.0));
    f[1] |= 1 << 32;
    assert_eq!(Reader::new(&f).f32(), None);
    // A bool out of range.
    assert_eq!(Reader::new(&[header(tag::BOOL, 1), 2]).bool(), None);
    // Non-finite floats under tags 4 to 6.
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(Reader::new(&words(|w| w.f32(v))).f32(), None);
        assert_eq!(Reader::new(&words(|w| w.f32s(&[0.0, v]))).f32s(&mut [0.0; 2]), None);
        assert_eq!(Reader::new(&words(|w| w.f64(f64::from(v)))).f64(), None);
    }
    // A child not consumed exactly.
    let child = words(|w| {
        w.child(|w| {
            w.u64(1);
            w.u64(2);
        })
    });
    assert_eq!(Reader::new(&child).child(Reader::u64), None);
    assert_eq!(Reader::new(&child).child(|r| Some((r.u64()?, r.u64()?))), Some((1, 2)));
    // A child whose header claims more words than the stream holds.
    assert_eq!(Reader::new(&[header(tag::CHILD, 5), header(tag::UINT, 1), 1]).child(Reader::u64), None);
    // Top-level exhaustion.
    let two = words(|w| {
        w.u64(1);
        w.u64(2);
    });
    let mut r = Reader::new(&two);
    assert_eq!(r.u64(), Some(1));
    assert!(!r.finish());
}

/// A child whose closure writes differently on its second pass is a bug in the record, caught.
#[test]
#[should_panic(expected = "other words than it counted")]
fn a_child_that_writes_other_words_than_it_counted_panics() {
    let mut calls = 0;
    words(|w| {
        w.child(|w| {
            calls += 1;
            for _ in 0..calls {
                w.u64(0);
            }
        })
    });
}
