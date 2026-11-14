//! The silent device of a `--null-audio` run: the real Player over the silent backend, paced by a
//! stepped clock. It opens, renders blocks and plays without asking any system audio backend (the
//! thread-local count at `factory_for` and the cpal factory stays put), and it never opens an input.

use super::*;
use crate::backend::null::{self, StepClock};
use std::time::Duration;

/// One block of the silent device at its defaults (512 frames at 48 kHz), rounded up to the
/// nanosecond, so one step always makes exactly one more block due.
const BLOCK: Duration = Duration::from_nanos(10_666_667);

fn null_player() -> (Player, Arc<StepClock>) {
    let clock = StepClock::new();
    let player = Player::with_null_device_clock(Arc::new(Session::default()), Arc::clone(&clock) as Arc<dyn null::PaceClock>);
    clock.settle();
    (player, clock)
}

#[test]
fn the_null_device_opens_the_real_player_and_renders_paced_blocks_without_a_system_backend() {
    let asked = crate::backend::asks::seen();
    let (player, clock) = null_player();
    assert!(player.null_device() && !player.silent, "the open device, not the silent fallback: {:?}", player.initial_error());
    assert_eq!(player.initial_error(), None);
    assert_eq!((player.device_name.as_str(), player.device_system.as_str()), (null::DEVICE, null::SYSTEM));
    assert_eq!((player.device_rate, player.device_channels, player.device_buffer), (48_000, 2, Some(512)));
    assert_eq!(player.output_sample, Some(ab::SampleFormat::F32));
    assert!(player.output_latency.is_some());
    assert!(!player.native_backend && player.native_factory.is_none(), "no native backend is kept for a later input");
    assert_ne!(player.thread_priority(), scheduling::WorkerPriority::Pending, "the callback thread's promotion is observed");
    let beats = |p: &Player| p.fault.beats.load(Ordering::Relaxed);
    assert_eq!(beats(&player), 1, "the first block at once");
    clock.advance(BLOCK);
    assert_eq!(beats(&player), 2);
    clock.advance(BLOCK * 3);
    assert_eq!(beats(&player), 5, "one callback per block, paced by the clock alone");

    // The transport runs over it as over a real device.
    player.service().unwrap();
    player.play(0, None, None).unwrap();
    clock.advance(BLOCK);
    player.service().unwrap();
    clock.advance(BLOCK);
    assert!(player.is_playing(), "playing after the next blocks");
    assert!(player.position() > 0, "the position moves with the blocks");
    assert!(player.trouble().is_none());

    assert_eq!(crate::backend::asks::seen(), asked, "neither factory_for nor the cpal factory was asked");
    drop(player);
    assert_eq!(crate::backend::asks::seen(), asked, "nor at close");
}

#[test]
fn the_null_device_never_opens_an_input_anywhere() {
    let (mut player, _clock) = null_player();
    // Lift this crate's test guard (#363), so only the silent device's own refusal stands between
    // the input and the factories: counting fakes here, never real ones.
    player.fake_only = false;
    let cpal = Counting::default();
    let native_calls = AtomicUsize::new(0);
    let native_for = |_: &AudioConfig| -> Option<Box<dyn ab::BackendFactory>> {
        native_calls.fetch_add(1, Ordering::Relaxed);
        None
    };
    let refused = player.open_input_with(&AudioConfig::default(), &cpal, &native_for).err();
    assert_eq!(refused.as_deref(), Some(null::NO_INPUT), "recording is unavailable, and says so");
    assert_eq!(cpal.0.load(Ordering::Relaxed) + native_calls.load(Ordering::Relaxed), 0, "no input factory was asked");
    assert!(!player.holds_input() && !player.duplex());
}

/// A cpal-side factory that counts and gives nothing.
#[derive(Default)]
struct Counting(AtomicUsize);

impl ab::BackendFactory for Counting {
    fn backend(&self, _system: Option<&str>) -> Option<Box<dyn ab::Backend>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        None
    }
}
