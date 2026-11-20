//! ADR 0015 amendment (#431) Q6: a port group wider than `MAX_DEVICE_CHANNELS` (32). The JACK
//! backend asks for every port of a group with no cap (`audio-jack` `plan` and `capture_format`),
//! so a 40-port group opens 40 capture and 40 playback channels. The scripted backend stands in for
//! JACK's one client (the JACK backend's own request is tested over its fake server).

use super::*;
use crate::backend::native::NativeKind;
use ab::fake::{SliceSink, SliceSource};
use transit_model::{ChannelFormat, Route, TrackKind};

const WIDE: u16 = 40;
const BLOCK: usize = 128;

fn wide_group() -> ab::fake::FakeBackend {
    ab::fake::FakeBackend::new(ab::Format::new(48_000, WIDE, ab::SampleFormat::F32)).named("JACK", "Big").with_capture(WIDE, true)
}

/// Channel `c` of every capture frame holds `(c + 1) / 100`, so a monitored channel says which one it read.
fn capture_block() -> Vec<f32> {
    (0..BLOCK).flat_map(|_| (0..WIDE).map(|c| f32::from(c + 1) / 100.0)).collect()
}

/// A mono audio track monitoring `input`, routed to Main.
fn session(input: &str) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mon"));
    let track = s.track_mut(t).unwrap();
    track.mixer.input = Route::Hardware(input.into());
    track.mixer.input_monitor = true;
    s
}

/// Opens the 40-port group with its input set, pumps capture then render `blocks` times, and returns
/// the last output block (40 channels interleaved).
fn run(input: &str, blocks: usize) -> (Player, Vec<f32>) {
    let fake = wide_group();
    let cfg = AudioConfig { system: Some("JACK".into()), ..Default::default() };
    let player = Player::open_native(Arc::new(session(input)), &cfg, Box::new(fake.clone()), &fake_factory(false), NativeKind::Jack).unwrap();
    let recorder = player.open_input(&cfg).unwrap();
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    let format = ab::Format::new(48_000, WIDE, ab::SampleFormat::F32);
    let input = capture_block();
    let mut output = vec![0.0f32; BLOCK * usize::from(WIDE)];
    for _ in 0..blocks {
        assert!(fake.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: usize::from(WIDE) }));
        output.fill(0.0);
        assert!(fake.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: usize::from(WIDE) }));
        player.service().unwrap();
    }
    drop(recorder);
    (player, output)
}

fn channel_peak(output: &[f32], c: usize) -> f32 {
    output.chunks(usize::from(WIDE)).map(|f| f[c].abs()).fold(0.0, f32::max)
}

/// The table is capped at 32 in both directions; the device channels past 32 are opened by the
/// backend but carry silence out and are never read in; a route to "In 33" (past the cap) is
/// silent, and "In 32" is the 32nd channel.
#[test]
fn a_forty_port_group_is_bounded_at_thirty_two_channels_through_the_player() {
    let (player, output) = run("In 32", 24);
    let io = player.io_map();
    assert_eq!((io.inputs, io.outputs), (32, 32), "the table is capped at MAX_DEVICE_CHANNELS");
    assert!(io.path("In 33").is_none() && io.path("Out 33-34").is_none());
    // Channel c holds (c + 1) / 100, so the ratio to "In 1" names the channel read (the pan law cancels).
    let (_, first) = run("In 1", 24);
    let ratio = channel_peak(&output, 0) / channel_peak(&first, 0);
    assert!((ratio - 32.0).abs() < 0.01, "In 32 reads device channel 32, heard on Main: ratio {ratio}");
    for c in 32..usize::from(WIDE) {
        assert_eq!(channel_peak(&output, c), 0.0, "device output channel {} past the cap is silent", c + 1);
    }
    let (_, past) = run("In 33", 24);
    let ratio = channel_peak(&past, 0) / channel_peak(&first, 0);
    assert_eq!(ratio, 0.0, "a route past the cap is silent, never another channel: ratio {ratio}");
}
