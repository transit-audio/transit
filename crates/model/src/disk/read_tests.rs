//! Clip reads through the band-limited reader (ADR 0056 decision 8, issue #604): a stretch of 1 is
//! the direct index it always was, a disk-backed clip streams and renders what memory gives bit for
//! bit at any cache window, ring window and block cut, a stretched clip sits where the linear read
//! sat, and decision 8.4's bounds hold for a conformed clip and a sped-up one.

use super::tests::{Fake, memory_read};
use super::*;

/// `StreamMap::sample` as it was before #604 (two-point linear interpolation for a stretch other
/// than 1), kept to pin what a stretch of 1 reads and where a stretched clip sits.
fn old_sample(map: &StreamMap, k: Samples, mut get: impl FnMut(usize) -> Option<f32>) -> f32 {
    let rel = crate::clip::content_pos(map.looping, k);
    if (map.stretch - 1.0).abs() < 1e-9 {
        usize::try_from(map.offset.saturating_add(rel)).ok().and_then(&mut get).unwrap_or(0.0)
    } else {
        let f = map.offset as f64 + rel as f64 / map.stretch;
        let x0 = f.floor();
        let fr = (f - x0) as f32;
        let a = if x0 >= 0.0 { get(x0 as usize).unwrap_or(0.0) } else { 0.0 };
        let b = if x0 + 1.0 >= 0.0 { get((x0 as usize).saturating_add(1)).unwrap_or(0.0) } else { 0.0 };
        a + (b - a) * fr
    }
}

/// Two channels of `frames` frames of integer noise in [-1, 1), with a few -0.0 samples and the
/// extremes, so a value that is not the source frame itself shows in its bits.
fn noise(frames: usize) -> Vec<Vec<f32>> {
    let mut x: u32 = 0x1234_5678;
    (0..2)
        .map(|_| {
            (0..frames)
                .map(|i| {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    match i % 97 {
                        5 => -0.0,
                        11 => -1.0,
                        _ => (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0,
                    }
                })
                .collect()
        })
        .collect()
}

fn source(data: &[Vec<f32>], window: usize, cache: usize) -> Arc<DiskSource> {
    let data = Arc::new(data.to_vec());
    let frames = data[0].len() as u64;
    let channels = data.len();
    let opener: Opener = Arc::new(move || Ok(Box::new(Fake { data: Arc::clone(&data) }) as Box<dyn DiskFile>));
    DiskSource::with_windows(PathBuf::from("fake.wav"), channels, frames, 48_000, window, cache, opener)
}

fn stretched(clip: u64, offset: i64, stretch: f64, length: i64, looping: Option<ClipLoop>) -> StreamMap {
    StreamMap { clip, offset, looping, stretch, length }
}

#[test]
fn a_stretch_of_one_reads_the_source_frame_bit_for_bit_as_before() {
    let data = noise(4_000);
    let maps = [
        stretched(1, 0, 1.0, 4_000, None),
        stretched(2, 37, 1.0, 3_000, None),
        stretched(3, -20, 1.0, 500, None),
        stretched(4, 3_900, 1.0, 300, None),
        stretched(5, 100, 1.0, 3_000, Some(ClipLoop { start: 200, end: 1_000 })),
        stretched(6, 0, 1.0, 3_000, Some(ClipLoop { start: -50, end: 700 })),
    ];
    for m in maps {
        for k in -10..m.length + 10 {
            for ch in 0..2 {
                let want = old_sample(&m, k, |x| data[ch].get(x).copied());
                assert_eq!(memory_read(&m, &data, ch, k).to_bits(), want.to_bits(), "{m:?} channel {ch} at {k}");
            }
        }
    }
}

#[test]
fn a_stretched_clip_streams_from_disk_as_it_plays_from_memory() {
    let data = noise(8_000);
    let maps = [
        // A loop conformed from 133 to 130 bpm, ending past the file.
        stretched(1, 4_000, 133.0 / 130.0, 4_500, None),
        stretched(2, 0, 0.5, 3_000, None),
        stretched(3, 7, 1.5, 3_000, None),
        stretched(4, 0, 0.75, 3_000, Some(ClipLoop { start: -50, end: 700 })),
        stretched(5, 50, 1.25, 3_000, Some(ClipLoop { start: 100, end: 1_100 })),
        // The commands' bounds: q = 20 and q = 0.05.
        stretched(6, 300, 0.05, 400, None),
        stretched(7, 10, 20.0, 3_000, None),
        // A start before the file's first frame, and the direct index.
        stretched(8, -40, 0.9, 1_000, None),
        stretched(9, 100, 1.0, 3_000, Some(ClipLoop { start: 200, end: 1_000 })),
    ];
    // Ring window, cache window (8 is shorter than any tap span: a read loads what it needs), block.
    let cuts = [(64usize, 8usize, 7usize), (300, 1_500, 100), (1_000, 1_000, 64), (2_048, 65_536, 333)];
    for m in maps {
        let want: Vec<[f32; 2]> = (0..m.length).map(|k| [memory_read(&m, &data, 0, k), memory_read(&m, &data, 1, k)]).collect();
        for (window, cache, block) in cuts {
            let d = source(&data, window, cache);
            d.cue(&m, 0);
            d.pump();
            let mut k = 0;
            while k < m.length {
                let n = (block as i64).min(m.length - k);
                let v = d.stream(&m, k, n as usize).unwrap_or_else(|| panic!("{m:?} {window}/{cache}/{block}: short at {k}"));
                for i in k..k + n {
                    for (ch, w) in want[i as usize].iter().enumerate() {
                        assert_eq!(v.get(ch, i).to_bits(), w.to_bits(), "{m:?} ring {window}/{cache}/{block}, channel {ch} at {i}");
                    }
                }
                drop(v);
                k += n;
                d.pump();
            }
            assert_eq!(d.underruns(), 0);
            let sync = source(&data, window, cache);
            sync.with_sync_source(|src| {
                let mut out = [0.0f32; 2];
                for k in 0..m.length {
                    m.reader().read(k, src, 0, &mut out);
                    for (ch, (o, w)) in out.iter().zip(&want[k as usize]).enumerate() {
                        assert_eq!(o.to_bits(), w.to_bits(), "{m:?} sync {cache}, channel {ch} at {k}");
                    }
                }
            });
        }
    }
}

#[test]
fn a_stretched_clip_sits_where_the_linear_read_sat() {
    // An impulse at source frame 2,000, read by a clip from frame 100.
    let mut data = vec![vec![0.0f32; 6_000]];
    data[0][2_000] = 1.0;
    for stretch in [133.0 / 130.0, 0.5] {
        let m = stretched(1, 100, stretch, 5_000, None);
        let new: Vec<f32> = (0..m.length).map(|k| memory_read(&m, &data, 0, k)).collect();
        let old: Vec<f32> = (0..m.length).map(|k| old_sample(&m, k, |x| data[0].get(x).copied())).collect();
        let peak = |y: &[f32]| (0..y.len()).max_by(|&a, &b| y[a].abs().total_cmp(&y[b].abs())).unwrap_or(0);
        // The clip time the impulse maps to: 1,900 source frames at q = 1 / stretch.
        let center = 1_900.0 * stretch;
        assert_eq!(peak(&new), peak(&old), "stretch {stretch}: the same sample peaks");
        assert_eq!(peak(&new), center.round() as usize, "stretch {stretch}");
        // Every output is the kernel centered on the impulse, at the source position its clip time
        // maps to (no look-ahead): h(d / max(1, q)) / max(1, q) with d = x - 2000, within the table's
        // 1.7e-6 and f32 rounding.
        let q = m.rate();
        let wide = q.max(1.0);
        for (k, y) in new.iter().enumerate() {
            let d = 100.0 + k as f64 * q - 2_000.0;
            let want = transit_resample::kernel(d / wide) / wide;
            assert!((f64::from(*y) - want).abs() < 4e-6, "stretch {stretch}, clip time {k}: {y} against {want}");
        }
    }
    // At q = 2 from frame 101 the impulse falls halfway between clip times 949 and 950: the response
    // is symmetric about that point bit for bit, and not zero.
    let m = stretched(1, 101, 0.5, 5_000, None);
    let y: Vec<f32> = (0..m.length).map(|k| memory_read(&m, &data, 0, k)).collect();
    assert!(y[949] > 0.1, "{}", y[949]);
    for j in 0..40 {
        assert_eq!(y[949 - j].to_bits(), y[950 + j].to_bits(), "{j}");
    }
}

/// A clip of a `freq` Hz sine at -6 dBFS (phase `phase`) in a 48 kHz session, read at `stretch`
/// through `read`: the largest difference from the tone the clip should play (`ideal` gives it per
/// source position), relative to the tone's amplitude, in dB, over every clip time whose taps lie in
/// the file.
fn error_db(freq: f64, phase: f64, stretch: f64, ideal: impl Fn(f64) -> f64, read: impl Fn(&StreamMap, &[Vec<f32>], i64) -> f32) -> f64 {
    const FS: f64 = 48_000.0;
    const FRAMES: usize = 8_192;
    const AMP: f64 = 0.5;
    let data = vec![(0..FRAMES).map(|i| (AMP * (std::f64::consts::TAU * freq * i as f64 / FS + phase).sin()) as f32).collect::<Vec<f32>>()];
    let m = stretched(1, 0, stretch, (FRAMES as f64 * stretch) as i64, None);
    let q = m.rate();
    let margin = transit_resample::halo(q) as f64;
    let mut worst = 0.0f64;
    for k in 0..m.length {
        let x = k as f64 * q;
        if x < margin || x > FRAMES as f64 - 1.0 - margin {
            continue;
        }
        let want = AMP * ideal(x);
        worst = worst.max((f64::from(read(&m, &data, k)) - want).abs());
    }
    20.0 * (worst / AMP).log10()
}

fn new_read(m: &StreamMap, data: &[Vec<f32>], k: i64) -> f32 {
    memory_read(m, data, 0, k)
}

fn old_read(m: &StreamMap, data: &[Vec<f32>], k: i64) -> f32 {
    old_sample(m, k, |x| data[0].get(x).copied())
}

/// The worst over four tone phases of [`error_db`], for the reader and for the old linear read.
fn worst_db(freq: f64, stretch: f64, ideal: impl Fn(f64, f64) -> f64) -> (f64, f64) {
    let phases = [0.0, 0.7, 1.9, 2.6];
    let at = |read: fn(&StreamMap, &[Vec<f32>], i64) -> f32| {
        phases.iter().map(|&p| error_db(freq, p, stretch, |x| ideal(x, p), read)).fold(f64::NEG_INFINITY, f64::max)
    };
    (at(new_read), at(old_read))
}

#[test]
fn a_conformed_clip_meets_the_readers_passband_bound() {
    // Issue #604's case: a 133 bpm loop conformed to 130 bpm at 48 kHz (q = 0.977 < 1: decision 8.4's
    // passband bound of -73 dB holds for source tones up to 0.42 fs, 20.16 kHz).
    for freq in [1_000.0, 10_000.0, 20_000.0] {
        let tone = |x: f64, p: f64| (std::f64::consts::TAU * freq * x / 48_000.0 + p).sin();
        let (new, old) = worst_db(freq, 133.0 / 130.0, tone);
        eprintln!("conformed 133 to 130 bpm, {freq} Hz: error {new:.2} dB (linear read {old:.2} dB)");
        assert!(new <= -73.0, "{freq} Hz: {new:.2} dB over the -73 dB bound");
    }
    // A clip sped up to q = 2 (stretch 0.5): a tone that plays at 0.4 fs, inside the -78 dB band.
    let tone = |x: f64, p: f64| (std::f64::consts::TAU * 9_600.0 * x / 48_000.0 + p).sin();
    let (new, old) = worst_db(9_600.0, 0.5, tone);
    eprintln!("stretch 0.5, 9600 Hz (plays at 19.2 kHz): error {new:.2} dB (linear read {old:.2} dB)");
    assert!(new <= -78.0, "q = 2: {new:.2} dB over the -78 dB bound");
}

#[test]
fn a_sped_up_clip_rejects_what_would_play_above_the_new_nyquist() {
    // Decision 8.4's alias bound: a source tone that would play at 0.58 fs or above comes out at most
    // -78 dB (q >= 1.5) or -73.5 dB (1 < q < 1.5) relative to its level. The ideal output is silence.
    let silence = |_: f64, _: f64| 0.0;
    for (stretch, freq, bound) in [(0.5, 14_400.0, -78.0), (0.8, 23_040.0, -73.5), (0.25, 7_200.0, -78.0)] {
        let (new, old) = worst_db(freq, stretch, silence);
        eprintln!("stretch {stretch}, {freq} Hz (would play at {} Hz): output {new:.2} dB (linear read {old:.2} dB)", freq / stretch);
        assert!(new <= bound, "stretch {stretch}, {freq} Hz: {new:.2} dB over the {bound} dB bound");
    }
}

#[test]
fn a_clip_sped_up_past_the_readers_bound_reads_silence() {
    let data = noise(20_000);
    // q = 32 exactly is the reader's largest rate; a stretch of 1/40 (q = 40) is refused, as silence.
    let at_bound = stretched(1, 0, 1.0 / 32.0, 400, None);
    assert!((100..400).any(|k| memory_read(&at_bound, &data, 0, k) != 0.0));
    let past = stretched(2, 0, 1.0 / 40.0, 400, None);
    assert!((0..400).all(|k| memory_read(&past, &data, 0, k) == 0.0 && memory_read(&past, &data, 1, k) == 0.0));
    assert_eq!(MAX_RATE, 32.0);
}

#[test]
fn a_request_under_another_stretch_or_loop_is_not_taken_for_it() {
    let data = noise(8_000);
    let d = source(&data, 1_024, 4_096);
    let a = stretched(1, 0, 1.5, 3_000, None);
    let others = [StreamMap { stretch: 2.0, ..a }, StreamMap { looping: Some(ClipLoop { start: 0, end: 700 }), ..a }];
    for b in others {
        d.cue(&a, 0);
        d.cue(&b, 0);
        d.pump();
        assert!(d.stream(&a, 0, 64).is_some());
        assert!(d.stream(&b, 0, 64).is_some(), "{b:?}: its request was left waiting behind another mapping's");
    }
}

/// `channels` channels of [`noise`]-like integer noise, `frames` frames each.
fn noise_channels(channels: usize, frames: usize) -> Vec<Vec<f32>> {
    let mut x: u32 = 0x0bad_cafe;
    (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
                })
                .collect()
        })
        .collect()
}

/// Stream `m` from clip time `cue` through a disk source of `data` and compare every channel of every
/// frame with the in-memory read of that channel alone, bit for bit; then the offline reader.
fn streams_as_memory(data: &[Vec<f32>], m: StreamMap, window: usize, cache: usize, block: usize, cue: i64) {
    let d = source(data, window, cache);
    d.cue(&m, cue);
    d.pump();
    let mut k = cue;
    while k < m.length {
        let n = (block as i64).min(m.length - k);
        let v = d.stream(&m, k, n as usize).unwrap_or_else(|| panic!("{m:?} {window}/{cache}/{block} from {cue}: short at {k}"));
        for i in k..k + n {
            for ch in 0..data.len() {
                let want = memory_read(&m, data, ch, i);
                assert_eq!(
                    v.get(ch, i).to_bits(),
                    want.to_bits(),
                    "{} channels, {m:?}, ring {window}/{cache}/{block} from {cue}, channel {ch} at {i}",
                    data.len()
                );
            }
        }
        drop(v);
        k += n;
        d.pump();
    }
    assert_eq!(d.underruns(), 0);
    source(data, window, cache).with_sync_source(|src| {
        let mut out = vec![0.0f32; data.len()];
        let reader = m.reader();
        for k in cue..m.length {
            reader.read(k, src, 0, &mut out);
            for (ch, o) in out.iter().enumerate() {
                assert_eq!(o.to_bits(), memory_read(&m, data, ch, k).to_bits(), "{} channels, {m:?}, sync, channel {ch} at {k}", data.len());
            }
        }
    });
}

#[test]
fn every_channel_count_a_source_shorter_than_the_halo_and_a_cue_mid_clip_stream_as_memory_plays() {
    // Mono, three, six and forty channels (wider than the mix's 32-channel frame), cued at the clip's
    // start and in its middle.
    for channels in [1usize, 3, 6, 40] {
        let data = noise_channels(channels, 6_000);
        let maps = [stretched(1, 30, 133.0 / 130.0, 3_000, None), stretched(2, 0, 0.25, 1_000, Some(ClipLoop { start: 100, end: 700 }))];
        for m in maps {
            for (window, cache, block, cue) in [(256, 8, 61, 0), (1_024, 4_096, 300, 777)] {
                streams_as_memory(&data, m, window, cache, block, cue);
            }
        }
    }
    // A source of 10 frames, shorter than every halo (17 at q <= 1, 321 at q 20).
    let short = noise_channels(2, 10);
    for stretch in [0.05, 0.8, 1.25, 3.0] {
        let m = stretched(3, -5, stretch, 60, None);
        assert!((0..60).any(|k| memory_read(&m, &short, 0, k) != 0.0), "stretch {stretch}");
        streams_as_memory(&short, m, 64, 8, 7, 0);
        streams_as_memory(&short, m, 64, 8, 7, 23);
    }
}
