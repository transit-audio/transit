//! Ogg Opus export and import (#238): lengths after pre-skip, the spectrum of a 1 kHz sine at
//! 160 kb/s, the bitrate presets, the surround refusal, the sniffer and the extension map. The
//! writer's output is decoded by the crate's own reader, which is libopus 1.6.1's decoder (no
//! opusdec runs here). Nothing here reads a clock.

use transit_audio_io::{
    AudioBuffer, AudioError, BlockReader, EncodeOptions, FileEncoder, FileFormat, OPUS_BITRATES_KBPS, decode, detect_format, encode,
    encode_extension, encode_format_for, opus_granules, opus_tags, probe,
};

fn opus(kbps: Option<u16>) -> EncodeOptions {
    EncodeOptions { format: FileFormat::Opus, bitrate_kbps: kbps, ..EncodeOptions::default() }
}

/// A sine of `hz` at `amp` in every channel (the second channel a quarter period later).
fn sine(rate: u32, channels: usize, frames: usize, hz: f64, amp: f32) -> AudioBuffer {
    let channels = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate) + c as f64 * std::f64::consts::FRAC_PI_2).sin() as f32 * amp)
                .collect()
        })
        .collect();
    AudioBuffer { sample_rate: rate, channels }
}

/// Program-like material: a chord of three partials with a slow tremolo, so VBR has work to do.
fn music(rate: u32, channels: usize, frames: usize) -> AudioBuffer {
    music_with(rate, channels, frames, 0.05)
}

/// [`music`] with `noise` (peak to peak) of deterministic white noise.
fn music_with(rate: u32, channels: usize, frames: usize, noise_pp: f64) -> AudioBuffer {
    let channels = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| {
                    let t = i as f64 / f64::from(rate);
                    let trem = 0.6 + 0.4 * (2.0 * std::f64::consts::PI * 0.5 * t).sin();
                    let v = [220.0, 277.18, 329.63, 1760.0, 5274.0]
                        .iter()
                        .enumerate()
                        .map(|(k, f)| (2.0 * std::f64::consts::PI * f * t + c as f64 * 0.3 * k as f64).sin() / (k as f64 + 1.5))
                        .sum::<f64>();
                    // A little deterministic noise, so the encoder spends its bits as on a mix.
                    let noise = ((i as u64).wrapping_mul(6_364_136_223_846_793_005).wrapping_add((c as u64).wrapping_mul(1_442_695_040_888_963_407))
                        >> 40) as f64
                        / f64::from(1u32 << 24)
                        - 0.5;
                    (v * trem * 0.3 + noise * noise_pp) as f32
                })
                .collect()
        })
        .collect();
    AudioBuffer { sample_rate: rate, channels }
}

/// The 48 kHz length of `frames` frames at `rate`, as the writer converts.
fn at_48k(frames: usize, rate: u32) -> usize {
    ((frames as u64 * 48_000 + u64::from(rate) / 2) / u64::from(rate)) as usize
}

#[test]
fn a_bounce_decodes_to_its_own_length_after_pre_skip() {
    for rate in [44_100u32, 48_000] {
        for secs in [1.0f64, 10.0, 0.5] {
            for channels in [1usize, 2] {
                let frames = (secs * f64::from(rate)).round() as usize;
                let src = music(rate, channels, frames);
                let bytes = encode(&src, &opus(None)).unwrap();
                let want = at_48k(frames, rate);
                let (info, buf) = decode(&bytes, None).unwrap();
                let point = format!("{rate} Hz, {secs} s, {channels} ch");
                assert_eq!(info.format, FileFormat::Opus, "{point}");
                assert_eq!(info.sample_rate, 48_000, "{point}");
                assert_eq!(usize::from(info.channels), channels, "{point}");
                assert_eq!(buf.frames(), want, "{point}: decoded length");
                assert_eq!(probe(&bytes, None).unwrap().frames, want as u64, "{point}: probed length");
                // RFC 7845 4.5: the last granule position is pre-skip plus the length; the
                // pre-skip is libopus's lookahead (312 samples, 6.5 ms, for this configuration).
                let (pre_skip, last) = opus_granules(&bytes).unwrap();
                assert_eq!(pre_skip, 312, "{point}");
                assert_eq!(last, i64::from(pre_skip) + want as i64, "{point}");
            }
        }
    }
}

#[test]
fn the_streamed_file_is_the_whole_buffer_file_byte_for_byte() {
    for rate in [44_100u32, 48_000] {
        let src = music(rate, 2, rate as usize * 3 / 2);
        let whole = encode(&src, &opus(Some(128))).unwrap();
        for block in [1usize, 333, 1024, 4096] {
            let mut enc = FileEncoder::new(std::io::Cursor::new(Vec::new()), &opus(Some(128)), 2, rate, None).unwrap();
            let mut at = 0;
            while at < src.frames() {
                let n = block.min(src.frames() - at);
                let chunk: Vec<Vec<f32>> = src.channels.iter().map(|c| c[at..at + n].to_vec()).collect();
                enc.check_room(n as u64).unwrap();
                enc.write(&chunk, n).unwrap();
                at += n;
            }
            assert_eq!(enc.finish().unwrap().into_inner(), whole, "{rate} Hz in blocks of {block}");
        }
    }
}

// ---- The spectrum sweep ----

/// An in-place radix-2 FFT of `re` + i`im` (lengths a power of two).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        len <<= 1;
    }
}

const N: usize = 32_768;

/// The magnitude spectrum in dB (full scale sine = 0 dB) of `N` samples of `x` from `from`, under
/// a 4-term Blackman-Harris window.
fn spectrum_db(x: &[f32], from: usize) -> Vec<f64> {
    let w = |i: usize| {
        let p = 2.0 * std::f64::consts::PI * i as f64 / (N - 1) as f64;
        0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos()
    };
    let gain: f64 = (0..N).map(w).sum::<f64>() / 2.0;
    let mut re: Vec<f64> = (0..N).map(|i| f64::from(x[from + i]) * w(i)).collect();
    let mut im = vec![0.0; N];
    fft(&mut re, &mut im);
    (0..N / 2).map(|k| 20.0 * ((re[k].hypot(im[k]) / gain).max(1e-20)).log10()).collect()
}

/// The bands the decoded spectrum is checked in, by distance from the fundamental: the codec's
/// noise is shaped around the tone it codes (masking), so its floor depends on the distance.
const BANDS: [(f64, f64); 3] = [(15.0, 100.0), (100.0, 1000.0), (1000.0, 24_000.0)];

/// The codec's floor in each band at 160 kb/s, relative to the fundamental: the worst bin
/// measured over the sweep's points with libopus 1.6.1 (see the #238 result note), plus
/// [`FLOOR_MARGIN_DB`] for compiler and platform differences.
const FLOOR_MEASURED_DB: [f64; 3] = [-49.5, -58.3, -94.2];
const FLOOR_MARGIN_DB: f64 = 6.0;

/// One sweep point's figures.
struct Point {
    fundamental_err_db: f64,
    /// Per band: the highest bin relative to the fundamental, and its frequency.
    worst: [(f64, f64); 3],
}

/// Bounces a sine of `hz` at -6 dBFS for 2 s at `rate` in `channels` at 160 kb/s, decodes it, and
/// compares the decoded spectrum at 48 kHz with a 1 kHz source's (rendered at 48 kHz directly).
fn sine_point(rate: u32, channels: usize, hz: f64) -> Point {
    measure(&sine(rate, channels, 2 * rate as usize, hz, 0.5))
}

/// Bounces `src` at 160 kb/s, decodes it, and compares the decoded spectrum with a 1 kHz sine's.
fn measure(src: &AudioBuffer) -> Point {
    let channels = src.channels.len();
    let bytes = encode(src, &opus(Some(160))).unwrap();
    let (_, buf) = decode(&bytes, None).unwrap();
    let reference = sine(48_000, channels, 96_000, 1000.0, 0.5);
    let bin_hz = 48_000.0 / N as f64;
    let k0 = (1000.0 / bin_hz).round() as usize;
    let mut point = Point { fundamental_err_db: 0.0, worst: [(f64::NEG_INFINITY, 0.0); 3] };
    for c in 0..channels {
        // Half a second in, clear of the start.
        let got = spectrum_db(&buf.channels[c], 24_000);
        let want = spectrum_db(&reference.channels[c], 24_000);
        let peak = |s: &[f64]| s[k0 - 2..=k0 + 2].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (g, w) = (peak(&got), peak(&want));
        if (g - w).abs() > point.fundamental_err_db.abs() {
            point.fundamental_err_db = g - w;
        }
        for (k, &v) in got.iter().enumerate().skip(2) {
            let away = k.abs_diff(k0) as f64 * bin_hz;
            for (b, &(lo, hi)) in BANDS.iter().enumerate() {
                if away >= lo && away < hi && v - g > point.worst[b].0 {
                    point.worst[b] = (v - g, k as f64 * bin_hz);
                }
            }
        }
    }
    point
}

/// The figures of a point, one line.
fn line(rate: u32, channels: usize, p: &Point) -> String {
    let bands: Vec<String> =
        BANDS.iter().zip(&p.worst).map(|((lo, hi), (db, hz))| format!("{lo:.0}-{hi:.0} Hz away {db:.1} dB at {hz:.0} Hz")).collect();
    format!("{rate} Hz {channels} ch: fundamental {:+.3} dB; {}", p.fundamental_err_db, bands.join("; "))
}

/// Fails when a point's fundamental is off by more than 0.1 dB or a band rises over its floor.
fn check(rate: u32, channels: usize, p: &Point) -> Result<(), String> {
    if p.fundamental_err_db.abs() > 0.1 {
        return Err(format!("{rate} Hz {channels} ch: the fundamental is off by {:.3} dB", p.fundamental_err_db));
    }
    for (b, (db, hz)) in p.worst.iter().enumerate() {
        let floor = FLOOR_MEASURED_DB[b] + FLOOR_MARGIN_DB;
        if *db >= floor {
            return Err(format!("{rate} Hz {channels} ch: a bin at {hz:.0} Hz is {db:.1} dB from the fundamental (floor {floor} dB)"));
        }
    }
    Ok(())
}

#[test]
fn a_1_khz_sine_at_160_kbps_keeps_its_spectrum() {
    let mut points = Vec::new();
    for rate in [48_000u32, 44_100] {
        for channels in [1usize, 2] {
            points.push((rate, channels, sine_point(rate, channels, 1000.0)));
        }
    }
    eprintln!("{}", points.iter().map(|(r, c, p)| line(*r, *c, p)).collect::<Vec<_>>().join("\n"));
    for (rate, channels, p) in &points {
        if let Err(e) = check(*rate, *channels, p) {
            panic!("{e}");
        }
    }
}

#[test]
fn the_spectrum_check_catches_a_wrong_tone_and_a_spur() {
    // Negative controls: the same check fails a 1.1 kHz tone (the fundamental is missing) and a
    // 1 kHz tone with a spur 40 dB down at 5 kHz (a band over its floor).
    let wrong = sine_point(48_000, 1, 1100.0);
    assert!(check(48_000, 1, &wrong).unwrap_err().contains("fundamental"), "{}", line(48_000, 1, &wrong));
    let mut spur = sine(48_000, 1, 96_000, 1000.0, 0.5);
    for (i, v) in spur.channels[0].iter_mut().enumerate() {
        *v += (2.0 * std::f64::consts::PI * 5000.0 * i as f64 / 48_000.0).sin() as f32 * 0.005;
    }
    let p = measure(&spur);
    assert!(check(48_000, 1, &p).unwrap_err().contains("5000 Hz"), "{}", line(48_000, 1, &p));
}

// ---- Bitrates ----

#[test]
fn every_bitrate_preset_decodes_and_sizes_with_its_rate() {
    let src = music(48_000, 2, 480_000);
    let mut sizes: Vec<(u16, usize, f64)> = Vec::new();
    for &k in &OPUS_BITRATES_KBPS {
        let bytes = encode(&src, &opus(Some(k))).unwrap();
        let (_, buf) = decode(&bytes, None).unwrap();
        assert_eq!(buf.frames(), 480_000, "{k} kb/s decodes whole");
        // The whole file's rate over its 10 s (the headers are under 0.1 percent of it).
        sizes.push((k, bytes.len(), bytes.len() as f64 * 8.0 / 10.0 / 1000.0));
    }
    eprintln!("{}", sizes.iter().map(|(k, b, r)| format!("{k} kb/s: {b} bytes, {r:.1} kb/s")).collect::<Vec<_>>().join("\n"));
    for (k, _, kbps) in sizes {
        assert!((kbps / f64::from(k) - 1.0).abs() <= 0.10, "{k} kb/s came out at {kbps:.1} kb/s");
    }
}

#[test]
fn the_default_bitrate_is_160_stereo_96_mono_and_out_of_range_rates_are_refused() {
    let src = music(48_000, 2, 48_000);
    assert_eq!(encode(&src, &opus(None)).unwrap(), encode(&src, &opus(Some(160))).unwrap());
    let mono = music(48_000, 1, 48_000);
    assert_eq!(encode(&mono, &opus(None)).unwrap(), encode(&mono, &opus(Some(96))).unwrap(), "96 kb/s is the mono default");
    assert_ne!(encode(&mono, &opus(None)).unwrap(), encode(&mono, &opus(Some(160))).unwrap());
    assert_eq!(transit_audio_io::opus_bitrates_kbps(1), [96, 128, 160]);
    assert_eq!(transit_audio_io::opus_bitrates_kbps(2), OPUS_BITRATES_KBPS);
    for bad in [0u16, 5, 511, 1000] {
        assert!(matches!(encode(&src, &opus(Some(bad))), Err(AudioError::Unsupported(_))), "{bad} kb/s");
    }
}

// ---- Channels, tags, headers ----

#[test]
fn surround_is_refused_with_a_clear_error() {
    for channels in [3usize, 6, 8] {
        let src = music(48_000, channels, 4800);
        match encode(&src, &opus(None)) {
            Err(AudioError::Unsupported(m)) => assert!(m.contains("stereo"), "{m}"),
            other => panic!("{channels} channels: {other:?}"),
        }
        assert!(FileEncoder::new(std::io::Cursor::new(Vec::new()), &opus(None), channels, 48_000, None).is_err());
    }
}

#[test]
fn the_tags_carry_the_encoder_and_the_title() {
    let src = music(44_100, 2, 4410);
    let bytes = encode(&src, &EncodeOptions { title: Some("Night Drive".into()), ..opus(None) }).unwrap();
    let (vendor, comments) = opus_tags(&bytes).unwrap();
    assert_eq!(vendor, "libopus 1.6.1");
    assert!(comments.iter().any(|c| c.starts_with("ENCODER=transit ")), "{comments:?}");
    assert!(comments.contains(&"TITLE=Night Drive".to_string()), "{comments:?}");
    let untitled = encode(&src, &opus(None)).unwrap();
    assert!(!opus_tags(&untitled).unwrap().1.iter().any(|c| c.starts_with("TITLE=")));
    // The OpusHead keeps the session's rate (informational) and family 0.
    assert_eq!(&bytes[28..36], b"OpusHead");
    assert_eq!(u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]), 44_100);
    assert_eq!(bytes[46], 0, "channel mapping family 0");
}

#[test]
fn the_bit_depth_and_dither_do_not_change_an_opus_file() {
    let src = music(48_000, 2, 48_000);
    let a = encode(&src, &EncodeOptions { bit_depth: transit_audio_io::BitDepth::Int16, dither: true, ..opus(None) }).unwrap();
    let b = encode(&src, &EncodeOptions { bit_depth: transit_audio_io::BitDepth::Float32, dither: false, ..opus(None) }).unwrap();
    assert_eq!(a, b);
}

// ---- Sniffer, extensions, streamed import ----

#[test]
fn the_sniffer_tells_opus_from_other_ogg() {
    let bytes = encode(&music(48_000, 1, 4800), &opus(None)).unwrap();
    assert_eq!(detect_format(&bytes, None), FileFormat::Opus);
    assert_eq!(detect_format(&bytes, Some("ogg")), FileFormat::Opus, "the magic wins over the extension");
    // An Ogg stream whose first packet is not OpusHead (Vorbis) stays Ogg.
    let mut vorbis = bytes.clone();
    vorbis[28..36].copy_from_slice(b"\x01vorbis\0");
    assert_eq!(detect_format(&vorbis, None), FileFormat::Ogg);
    // Negative control: a truncated first page is not Opus.
    assert_eq!(detect_format(&bytes[..30], None), FileFormat::Ogg);
    assert_eq!(detect_format(b"", Some("opus")), FileFormat::Opus);
    assert_eq!(detect_format(b"", Some(".OPUS")), FileFormat::Opus);
}

#[test]
fn the_extension_maps_name_opus() {
    assert_eq!(encode_format_for("opus"), Some(FileFormat::Opus));
    assert_eq!(encode_format_for(".Opus"), Some(FileFormat::Opus));
    assert_eq!(encode_format_for("mp3"), None, "transit writes no MP3");
    assert_eq!(encode_format_for("ogg"), None);
    assert_eq!(encode_extension(FileFormat::Opus), "opus");
    assert_eq!(encode_extension(FileFormat::Aiff), "aif");
    assert!(transit_audio_io::ENCODE_EXTENSIONS.contains("opus"));
    assert!(!transit_audio_io::ENCODE_EXTENSIONS.contains("mp3"));
}

#[test]
fn a_block_reader_hands_out_exactly_the_decode() {
    let bytes = encode(&music(44_100, 2, 30_000), &opus(None)).unwrap();
    let (_, whole) = decode(&bytes, None).unwrap();
    let len = bytes.len() as u64;
    let mut r = BlockReader::open(Box::new(std::io::Cursor::new(bytes)), len, Some("opus")).unwrap();
    assert_eq!(r.info().format, FileFormat::Opus);
    assert_eq!(r.info().frames, whole.frames() as u64);
    let mut got: Vec<Vec<f32>> = vec![Vec::new(); 2];
    let mut block = Vec::new();
    loop {
        let n = r.read(&mut block, 1000).unwrap();
        if n == 0 {
            break;
        }
        for (g, b) in got.iter_mut().zip(&block) {
            g.extend_from_slice(b);
        }
    }
    assert_eq!(got, whole.channels);
    assert!(r.seek(10).unwrap());
    r.read(&mut block, 5).unwrap();
    assert_eq!(block[0], whole.channels[0][10..15]);
}

#[test]
fn a_corrupt_opus_file_is_an_error_not_a_panic() {
    let bytes = encode(&music(48_000, 2, 48_000), &opus(None)).unwrap();
    for at in [5usize, 30, 60, bytes.len() / 2, bytes.len() - 3] {
        let mut b = bytes.clone();
        b[at] ^= 0x5A;
        assert!(decode(&b, Some("opus")).is_err(), "a flipped byte at {at}");
    }
    // A file cut short: an error mid-page; cut at a page boundary it decodes what it holds.
    for cut in [bytes.len() / 2, bytes.len() / 3, 100] {
        if let Ok((_, buf)) = decode(&bytes[..cut], Some("opus")) {
            assert!(buf.frames() < 48_000, "a file cut at {cut} decodes less than the whole");
        }
    }
}
