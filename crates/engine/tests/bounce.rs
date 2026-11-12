//! Bounces stream the main mix or a bus to their file, exactly as rendered in memory.

use transit_audio_io::{BitDepth, EncodeOptions, FileFormat};
use transit_engine::io::{Bounce, bounce_stems};
use transit_time::Range;

fn scratch(name: &str) -> transit_test_support::TestDir {
    let dir = transit_test_support::TestDir::new(&format!("scratch-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn bounces_write_exactly_the_mix_and_report_its_levels() {
    let e = transit_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-exact");
    let path = dir.join("Mix.wav");
    let r = Range::new(s.sample_rate.samples(10.0) + 7, s.sample_rate.samples(13.0));
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: None, title: None };
    let verb = s.bus_by_name("Verb").unwrap().id;
    for bus in [None, Some(verb)] {
        let mut mix = match bus {
            Some(b) => transit_mix::render_bus(s, b, r, 1024),
            None => transit_mix::render_range(s, r, 1024),
        };
        assert!(mix.iter().flatten().any(|v| v.abs() > 1e-3), "{bus:?}");
        for normalize in [false, true] {
            let mut bounce = Bounce::new(e.session_arc(), &path, r, &opts, bus, normalize, false).unwrap();
            let mut progress = vec![bounce.progress()];
            let done = loop {
                if let Some(done) = bounce.step(|| false).unwrap() {
                    break done;
                }
                progress.push(bounce.progress());
            };
            let last = progress[progress.len() - 1];
            assert!(progress.len() > 100 && progress.is_sorted() && progress[0] < 0.01 && last > 0.99 && last <= 1.0, "{progress:?}");
            if normalize {
                transit_dsp::offline::normalize(&mut mix, -0.1, false);
            }
            let bytes = std::fs::read(&path).unwrap();
            let (_, file) = transit_audio_io::decode(&bytes, None).unwrap();
            assert_eq!(file.channels, mix, "{bus:?} normalize: {normalize}");
            let mut lm = transit_dsp::meter::LoudnessMeter::new(s.sample_rate.as_f64() as f32, mix.len());
            lm.process(&mix, mix[0].len());
            let levels = (transit_dsp::offline::peak_db(&mix), lm.integrated_lufs(), lm.true_peak_db());
            assert_eq!((done.bytes, (done.peak_db, done.lufs, done.true_peak_db)), (bytes.len() as u64, levels));
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 32-bit float bounce of a mix peaking at +6 dBFS keeps the peak (#519); a 24-bit and a 16-bit
/// bounce of the same mix clip at full scale, sample for sample as the integer writer always has.
#[test]
fn a_float_bounce_keeps_a_peak_above_full_scale_and_integer_bounces_clip_it() {
    let rate = 48_000u32;
    let mut s = transit_model::Session::new("Hot", transit_time::SampleRate::new(rate).unwrap());
    let t = s.add_track(transit_model::TrackKind::Audio, transit_model::ChannelFormat::Stereo, Some("Hot"));
    let peak = 10f32.powf(6.0 / 20.0);
    let ch: Vec<f32> = (0..4_800).map(|i| peak * (2.0 * std::f32::consts::PI * 100.0 * i as f32 / rate as f32).sin()).collect();
    let buf = transit_audio_io::AudioBuffer { sample_rate: rate, channels: vec![ch.clone(), ch] };
    let src = transit_engine::io::add_source(&mut s, "Hot", buf, None, FileFormat::Wav);
    let id = s.new_clip_id();
    transit_engine::edit::place_clip(&mut s, t, transit_model::Clip::audio(id, "Hot", src, 0, 0, 4_800));
    let e = transit_engine::Engine::new(s);
    let r = Range::new(0, 4_800);
    let mix = transit_mix::render_range(e.session(), r, 1024);
    let top = mix.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    assert!(top > 1.9, "the mix peaks near +6 dBFS: {top}");
    let dir = scratch("bounce-overs");
    let path = dir.join("Mix.wav");
    for (depth, bits) in [(BitDepth::Float32, 0), (BitDepth::Int24, 24), (BitDepth::Int16, 16)] {
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None, bitrate_kbps: None, title: None };
        let mut bounce = Bounce::new(e.session_arc(), &path, r, &opts, None, false, false).unwrap();
        while bounce.step(|| false).unwrap().is_none() {}
        let (_, file) = transit_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
        if bits == 0 {
            let as_bits = |m: &[Vec<f32>]| m.iter().map(|c| c.iter().map(|v| v.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>();
            assert_eq!(as_bits(&file.channels), as_bits(&mix), "the float bounce is the mix, bit for bit");
            continue;
        }
        let scale = f64::from(1u32 << (bits - 1));
        let quantize = |v: f32| ((f64::from(v.clamp(-1.0, 1.0)) * scale).round().clamp(-scale, scale - 1.0) / scale) as f32;
        let expect: Vec<Vec<f32>> = mix.iter().map(|c| c.iter().map(|&v| quantize(v)).collect()).collect();
        assert_eq!(file.channels, expect, "{depth:?}");
        let file_top = file.channels.iter().flatten().fold(0f32, |m, v| m.max(*v));
        assert_eq!(file_top, ((scale - 1.0) / scale) as f32, "{depth:?} clips at full scale");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bounce_too_big_for_its_format_fails_and_keeps_the_old_file() {
    let e = transit_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-too-big");
    let path = dir.join("Mix.aif");
    std::fs::write(&path, b"old").unwrap();
    let opts = EncodeOptions { format: FileFormat::Aiff, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: None, title: None };
    let err = Bounce::new(e.session_arc(), &path, Range::new(0, s.sample_rate.samples(3.9 * 3600.0)), &opts, None, false, false).err().unwrap();
    assert!(err.to_string().contains("4 GiB"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let err = bounce_stems(&e, &dir.to_string_lossy(), Range::new(0, s.sample_rate.samples(4.1 * 3600.0)), &opts).unwrap_err();
    assert!(err.to_string().contains("4 hours"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_an_unfinished_bounce_cancels_it_and_keeps_the_old_file() {
    let e = transit_engine::demo::demo_engine();
    let dir = scratch("bounce-cancel");
    let path = dir.join("Mix.wav");
    std::fs::write(&path, b"old").unwrap();
    let mut bounce = Bounce::new(e.session_arc(), &path, Range::new(0, 48_000), &EncodeOptions::default(), None, false, false).unwrap();
    assert_eq!(bounce.step(|| false).unwrap(), None);
    assert!(dir.join("Mix.wav.part").exists());
    drop(bounce);
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
