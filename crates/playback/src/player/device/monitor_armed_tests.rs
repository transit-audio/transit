//! Options > Monitor Armed Tracks (#730) through a whole fake device: an armed audio track with
//! the option off is not heard, its take still holds the input, and the option turns it back on.

use super::tests::fake_factory;
use super::tests::{asio_config, model12, wav_data_frames};
use super::*;
use ab::fake::{SliceSink, SliceSource};
use transit_model::{ChannelFormat, TrackKind};

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
