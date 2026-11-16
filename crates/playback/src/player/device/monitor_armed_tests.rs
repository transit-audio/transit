//! Options > Monitor Armed Tracks (#730) through a whole fake device: an armed audio track with
//! the option off is not heard, its take still holds the input, and the option turns it back on.

use super::tests::fake_factory;
use super::tests::{asio_config, model12, wav_data_frames};
use super::*;
use crate::monitor_path::MonitorSettings;
use crate::player::audio::CountIn;
use ab::fake::{SliceSink, SliceSource};
use transit_audio_io::AudioBuffer;
use transit_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackId, TrackKind};

const BLOCK: usize = 256;

/// Pumps `blocks` capture blocks of 0.5 on every input with a render after each; the loudest output sample.
fn pump(fake: &ab::fake::FakeBackend, player: &Player, blocks: usize) -> f32 {
    let format = ab::Format::new(48_000, 12, ab::SampleFormat::F32);
    let input = vec![0.5f32; BLOCK * 12];
    let mut output = vec![0.0f32; BLOCK * 10];
    let mut loudest = 0.0f32;
    for _ in 0..blocks {
        assert!(fake.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 12 }));
        assert!(fake.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 10 }));
        loudest = output.iter().fold(loudest, |m, x| m.max(x.abs()));
        player.service().unwrap();
    }
    loudest
}

#[test]
fn an_armed_track_is_silent_with_the_option_off_its_take_still_holds_the_input_and_on_it_is_heard() {
    let fake = model12();
    let cfg = asio_config();
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mic"));
    s.track_mut(t).unwrap().mixer.record_arm = true;
    let player = Player::open_native(Arc::new(s), &cfg, Box::new(fake.clone()), &fake_factory(false), NativeKind::Asio).unwrap();
    let mut recorder = player.open_input(&cfg).unwrap();
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    let dir = transit_test_support::TestDir::new("monitor-armed-take");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("take.wav");
    recorder.arm(vec![(t, vec![0], path)]).unwrap();
    assert!(pump(&fake, &player, 24) > 0.1, "option on (the default), stopped: the armed track is heard");
    player.set_monitor_armed(false).unwrap();
    assert!(!player.requested_monitor_armed());
    assert_eq!(pump(&fake, &player, 24), 0.0, "option off, stopped: the armed track is not heard");
    let (takes, _) = recorder.finish();
    let bytes = std::fs::read(&takes[0].1).unwrap();
    assert!(wav_data_frames(&takes[0].1) >= 40 * BLOCK as u64, "the take held the input through both phases");
    assert!(bytes.iter().rev().take(BLOCK).any(|b| *b != 0), "the take's last samples are the input, not silence");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mono armed track (with a 0.5 clip when `clip`), a device opened with the saved option, its
/// input open and set; the scripted input is a deterministic ramp of the global frame, so two
/// runs fed alike hold the same samples.
struct Rig {
    fake: ab::fake::FakeBackend,
    player: Player,
    recorder: crate::record::Recorder,
    track: TrackId,
    frame: u64,
}

fn rig(option_on: bool, clip: bool) -> Rig {
    let fake = model12();
    let cfg = asio_config();
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mic"));
    s.track_mut(track).unwrap().mixer.record_arm = true;
    if clip {
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 400_000]] };
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(buf)));
        let id = s.new_clip_id();
        s.track_mut(track).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(id, "dc", SourceId(900), 0, 0, 400_000));
    }
    let settings = MonitorSettings { monitor_armed: option_on, ..MonitorSettings::default() };
    let player = Player::open_native_monitored(Arc::new(s), &cfg, Box::new(fake.clone()), &fake_factory(false), NativeKind::Asio, &settings).unwrap();
    let recorder = player.open_input(&cfg).unwrap();
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    Rig { fake, player, recorder, track, frame: 0 }
}

impl Rig {
    /// One capture block of the scripted input, one render block, one service step: the loudest output sample.
    fn block(&mut self, constant: Option<f32>) -> f32 {
        let format = ab::Format::new(48_000, 12, ab::SampleFormat::F32);
        let mut input = vec![0.0f32; BLOCK * 12];
        for (i, frame) in input.chunks_mut(12).enumerate() {
            let n = self.frame + i as u64;
            for (c, x) in frame.iter_mut().enumerate() {
                *x = constant.unwrap_or(((n * 37 + c as u64 * 11) % 200) as f32 / 400.0 - 0.25);
            }
        }
        self.frame += BLOCK as u64;
        let mut output = vec![0.0f32; BLOCK * 10];
        assert!(self.fake.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 12 }));
        assert!(self.fake.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 10 }));
        self.player.service().unwrap();
        output.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn blocks(&mut self, n: usize, constant: Option<f32>) -> f32 {
        (0..n).fold(0.0, |m, _| m.max(self.block(constant)))
    }
}

/// Review item 5: a device opened with the option off (a reopen mid-session) is silent for the armed
/// track from its first callback, not after the UI's next sync. The same device opened with the
/// option on hears the input, so the silence is not an empty plan.
#[test]
fn a_device_opened_with_the_option_off_never_monitors_the_armed_track() {
    let (mut off, mut on) = (rig(false, false), rig(true, false));
    let mut on_heard_at = None;
    for i in 0..60 {
        let (quiet, loud) = (off.block(Some(0.5)), on.block(Some(0.5)));
        assert_eq!(quiet, 0.0, "off, block {i}: silent from the first callback");
        if loud > 0.1 && on_heard_at.is_none() {
            on_heard_at = Some(i);
        }
    }
    assert!(on_heard_at.is_some(), "the option-on device hears the armed track");
}

#[derive(Clone, Copy, Debug)]
enum Start {
    Plain,
    CountIn,
    Punch,
}

/// The bytes of the take a pass leaves, recorded from the scripted input.
fn take_bytes(option_on: bool, start: Start) -> Vec<u8> {
    let mut r = rig(option_on, false);
    let dir = transit_test_support::TestDir::new("monitor-armed-pass");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("take.wav");
    if let Start::Punch = start {
        r.player.play(0, None, None).unwrap();
        r.blocks(10, None);
    }
    r.player.prepare_recording(&mut r.recorder, vec![(r.track, vec![0], path)]).unwrap();
    match start {
        Start::Plain => r.player.record(&r.recorder, Some(0), None, None).unwrap(),
        Start::CountIn => r
            .player
            .record_with_count_in(&r.recorder, Some(0), None, None, Some(CountIn { frames: 4 * BLOCK as i64, beat: BLOCK as i64, per_bar: 4 }))
            .unwrap(),
        Start::Punch => r.player.record(&r.recorder, None, None, None).unwrap(),
    };
    r.blocks(40, None);
    r.player.stop().unwrap();
    r.blocks(4, None);
    let (takes, _) = r.recorder.finish();
    let bytes = std::fs::read(&takes[0].1).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

/// Review item 2: the option never changes what the recorder is fed. A pass from the same scripted
/// input leaves the identical take with the option on and off, for each way a record can start.
#[test]
fn the_take_is_identical_with_the_option_on_and_off() {
    for start in [Start::Plain, Start::CountIn, Start::Punch] {
        let (on, off) = (take_bytes(true, start), take_bytes(false, start));
        assert!(on.len() > 44 + 20 * BLOCK * 3, "{start:?}: the take holds the input ({} bytes)", on.len());
        assert!(on.iter().rev().take(BLOCK).any(|b| *b != 0), "{start:?}: not silence");
        assert!(on == off, "{start:?}: the two takes differ ({} and {} bytes)", on.len(), off.len());
    }
}

/// Review item 2: option off while RECORDING, through the device: the armed track's input is not in
/// the output and its existing clip plays (0.5 against the input's 0.25: twice the level).
#[test]
fn recording_with_the_option_off_plays_the_clip_and_not_the_input() {
    let heard = |option_on: bool| {
        let mut r = rig(option_on, true);
        let dir = transit_test_support::TestDir::new("monitor-armed-rec");
        std::fs::create_dir_all(&dir).unwrap();
        r.player.prepare_recording(&mut r.recorder, vec![(r.track, vec![0], dir.join("take.wav"))]).unwrap();
        r.player.record(&r.recorder, Some(0), None, None).unwrap();
        r.blocks(20, Some(0.25));
        let level = r.blocks(20, Some(0.25));
        r.player.stop().unwrap();
        r.blocks(4, Some(0.25));
        let _ = r.recorder.finish();
        let _ = std::fs::remove_dir_all(&dir);
        level
    };
    let (on, off) = (heard(true), heard(false));
    assert!(on > 0.05, "option on: the input replaces the clip ({on})");
    assert!((off / on - 2.0).abs() < 0.05, "option off: the clip plays at twice the input's level (on {on}, off {off})");
}
