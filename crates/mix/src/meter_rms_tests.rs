//! #395: the strip meter's RMS from known signals through the mix strip meter path, rendered
//! block by block (no wall clock; the window is a count of frames).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan_tests::*;
use super::*;
use transit_audio_io::AudioBuffer;
use transit_model::SourceId;

const LEN: usize = 96_000;

fn db(lin: f32) -> f32 {
    if lin > 0.0 { 20.0 * lin.log10() } else { f32::NEG_INFINITY }
}

fn near(got: f32, want_db: f32, what: &str) {
    assert!((db(got) - want_db).abs() < 0.05, "{what}: {} dBFS, want {want_db}", db(got));
}

/// Track `a`, stereo: a full-scale 1 kHz sine on the left, a half-scale square on the right.
/// Track `b`, stereo, plays nothing.
fn known_session() -> (Session, TrackId, TrackId) {
    let mut s = Session::default();
    let a = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let b = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let sine: Vec<f32> = (0..LEN).map(|i| (std::f32::consts::TAU * (i % 48) as f32 / 48.0).sin()).collect();
    let square: Vec<f32> = (0..LEN).map(|i| if (i / 24) % 2 == 0 { 0.5 } else { -0.5 }).collect();
    s.pool.insert(SourceId(395), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![sine, square] })));
    let id = s.new_clip_id();
    s.track_mut(a).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "known", SourceId(395), 0, 0, LEN as i64));
    (s, a, b)
}

fn meter(rig: &Rig, t: TrackId) -> StripMeter {
    rig.eng.meters().find(|(id, _)| *id == t).map(|(_, m)| m.clone()).unwrap()
}

#[test]
fn known_signals_read_their_rms_once_the_window_fills() {
    let (s, a, b) = known_session();
    let mut rig = Rig::new();
    rig.publish(&s);
    let window = RmsWindow::new(SR).window_frames() as usize;
    assert_eq!(window, 14_400, "300 ms at 48 kHz");
    let left = rig.render(window.div_ceil(BLOCK) + 8);
    let m = meter(&rig, a);
    near(m.rms[0], -3.01, "full-scale sine RMS");
    near(m.peak[0], 0.0, "full-scale sine peak");
    near(m.rms[1], -6.02, "half-scale square RMS");
    near(m.peak[1], -6.02, "half-scale square peak");
    assert_eq!(m.rms_channels, m.rms.to_vec(), "a stereo strip's pair is its channels");
    let quiet = meter(&rig, b);
    assert_eq!((db(quiet.rms[0]), db(quiet.rms[1])), (f32::NEG_INFINITY, f32::NEG_INFINITY), "silence");
    // The main bus shares the code path: its RMS is the RMS of the last window it played.
    let tail = &left[left.len() - window..];
    let want = (tail.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / window as f64).sqrt() as f32;
    let main = rig.eng.active_main_meter();
    assert!(want > 0.1, "the main mix plays the sine: {want}");
    near(main.rms[0], db(want), "main bus RMS");
    assert_eq!(main.rms_channels.len(), main.peaks.len());
}

#[test]
fn sub_blocks_each_count_and_the_reading_matches_whole_blocks() {
    let (s, a, _) = known_session();
    let mut rig = Rig::new();
    rig.publish(&s);
    // Uneven device blocks: the window counts frames, not blocks.
    let mut pos = 0i64;
    for frames in [100usize, 256, 7, 193].iter().cycle().take(200) {
        rig.eng.render_active(pos, *frames, &mut rig.out);
        pos += *frames as i64;
    }
    assert!(pos > 14_400);
    let m = meter(&rig, a);
    near(m.rms[0], -3.01, "sine, uneven blocks");
    near(m.rms[1], -6.02, "square, uneven blocks");
}

#[test]
fn measuring_rms_allocates_nothing_and_reset_zeroes_it() {
    let (s, a, _) = known_session();
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(8);
    let ((), stats) = transit_rt::testing::measure(|| {
        for _ in 0..80 {
            rig.eng.render_active(rig.pos, BLOCK, &mut rig.out);
            rig.pos += BLOCK as i64;
        }
    });
    assert!(stats.is_zero(), "rendering with the RMS meters allocated or freed: {stats:?}");
    assert!(meter(&rig, a).rms[0] > 0.5);
    let ((), stats) = transit_rt::testing::measure(|| rig.eng.reset());
    assert!(stats.is_zero(), "reset: {stats:?}");
    assert!(rig.eng.meters().all(|(_, m)| m.rms == [0.0; 2] && m.rms_channels.iter().all(|x| *x == 0.0)));
    assert_eq!(rig.eng.active_main_meter().rms, [0.0; 2]);
    // The windows start over: the first block after the reset reads that block alone, no history.
    let mut rig2 = Rig::new();
    rig2.publish(&s);
    rig.pos = 0;
    rig.render(1);
    rig2.render(1);
    assert_eq!(meter(&rig, a).rms, meter(&rig2, a).rms, "a reset window equals a fresh one");
}
