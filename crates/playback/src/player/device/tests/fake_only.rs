//! #363: a test Player never opens an input on the machine's real devices. The counting factory
//! stands at the cpal adapter's boundary; it must stay at zero for a test Player, and it counts once
//! the guard is off (the control).

use super::*;

/// A factory that only counts how often it is asked for a backend, and never gives one.
#[derive(Default)]
struct Counting(AtomicUsize);

impl Counting {
    fn calls(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl ab::BackendFactory for Counting {
    fn backend(&self, _system: Option<&str>) -> Option<Box<dyn ab::Backend>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        None
    }
}

#[test]
fn a_test_player_never_opens_an_input_on_the_machines_devices() {
    let (player, _output) = super::prepared();
    let cpal = Counting::default();
    // The native factory a silent runtime's input would use on Windows: counted, never real.
    let native_calls = AtomicUsize::new(0);
    let native_for = |_: &AudioConfig| -> Option<Box<dyn ab::BackendFactory>> {
        native_calls.fetch_add(1, Ordering::Relaxed);
        None
    };
    assert_eq!(player.open_input_with(&AudioConfig::default(), &cpal, &native_for).err().as_deref(), Some(TEST_PLAYER_INPUT));
    assert_eq!(cpal.calls() + native_calls.load(Ordering::Relaxed), 0, "a device-free test Player asks no real backend");
    // A scripted native output whose input fails: its error, not the cpal adapter.
    let native = fake_factory(false).with_capture(2, false);
    let scripted = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fake_factory(false)).unwrap();
    native.fail_next_open(ab::OpenError::DeviceNotAvailable("scripted".into()));
    assert_eq!(scripted.open_input_with(&AudioConfig::default(), &cpal, &native_for).err().as_deref(), Some("scripted"));
    assert_eq!(cpal.calls() + native_calls.load(Ordering::Relaxed), 0, "nor does a scripted backend's Player");
    // Control: with the guard off the cpal boundary is reached (and the counting fake refuses).
    let (mut unguarded, _output) = super::prepared();
    unguarded.fake_only = false;
    assert!(unguarded.open_input_with(&AudioConfig::default(), &cpal, &native_for).is_err());
    assert_eq!(cpal.calls() + native_calls.load(Ordering::Relaxed), 1, "cpal off Windows, the native factory on Windows");
}
