# E75: an input open looked up the native factory off Windows (#354)

1. **Failure.** The maintainer's Linux pre-push gate (WSL2 Ubuntu 22.04, the CI linux job's
   tests) failed `player::device::tests::fake_only::a_test_player_never_opens_an_input_on_the_machines_devices`
   (`crates/playback/src/player/device/tests/fake_only.rs`) on the cpal slice 3 branch
   (db807e7d) and on the landing chain built on it, at the control: with the test-Player guard
   off, one input open asked the backend boundaries twice (`left: 2, right: 1`). It passed on
   Windows and on main. Nothing was pushed.
2. **Mechanism.** Slice 3 gave a silent runtime's input its native factory as a parameter
   (`native_for`, `backend::native::input_factory` in production) and called it as an argument
   of `Recorder::open_on` (`crates/playback/src/record.rs`): `open_on(&config, cfg!(windows),
   native_for(&config), cpal)`. The lookup therefore ran on every platform, and off Windows its
   answer was dropped while cpal opened the input. Backtraces of the two counted calls in Linux:
   the native lookup from `player/device.rs:1003` (the argument), and the cpal adapter's
   `backend` from `record.rs:357` (`open_through`) via `record.rs:350` (`open_on`'s non-Windows
   branch). The test counted both. In production the lookup returns `None` off Windows before
   touching any host, so no device was enumerated or opened twice; the code still asked a
   boundary it had no use for, against its own doc comment (each platform asks one boundary).
3. **Why checks missed it.** The branch's tests ran on Windows only before the chain, where the
   native lookup is the one boundary and cpal is never asked, so the sum was 1. The control
   summed both counters, so it could not say which boundary was asked on which platform.
4. **Fix and regression test.** `Recorder::open_on` takes the native lookup as a closure and
   calls it only in its Windows branch (`Recorder::open_with` and `Player::open_input_with` pass
   `|| ...`). The guard test's control now asserts the exact pair per platform, `(cpal,
   native) == (0, 1)` on Windows and `(1, 0)` elsewhere; `the_recorder_on_windows_never_asks_cpal`
   asserts that the off-Windows open never calls the lookup. Reverting `open_on` to call the
   lookup eagerly fails both in Linux.
5. **Gate class.** Platform-conditional boundaries: a test that counts calls across a
   `cfg!(windows)` split asserts each boundary's exact count per platform, never a sum, and the
   Linux pre-push gate runs before a branch with such a split joins a chain.
