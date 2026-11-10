# E80: a separate input's channel count never reached the I/O table, and a test encoded a refusal the page should not send (#429)

1. **Failure.** On Linux (ALSA through cpal, PipeWire) a tester could not make an input path:
   Audio Devices > I/O paths read "Channels open on this device: 0 in" with Add disabled and no
   reason, while the input ran 32 channels. Unticking a path's only channel sent a path 0
   channels wide; the engine refused it eight times in the log ("a path is 1 to 16 channels
   wide, not 0"), and the refusal reached only the status line under the modal window.
2. **Mechanism.** On cpal the input is its own device (`capture_with_render` is false,
   `crates/playback/src/backend/cpal.rs`), so the output's open builds the I/O table with
   `inputs = 0` (`crates/playback/src/player/device/open.rs`) and nothing wrote the separate
   input's count into it (`Engine::set_io_map` was called only by `sync_io`'s adopt and by
   `setup.io_map` edits). The Channels picker sent `set_channels` with the toggled list even when
   it was empty (`crates/ui-studio/src/audio_devices_io.rs`), and `audio_devices/window.rs` put
   the refusal on `app.status` only.
3. **Why checks missed it.** Every I/O-table test ran over a duplex table (ASIO counts set
   directly, or a duplex fake), where the output's open knows the input count; no test drove the
   page over a backend whose input opens separately. The picker's test,
   `a_refused_edit_reaches_the_status_line_and_changes_nothing`, asserted that the engine refused
   the empty path the page sent, so it encoded the defect instead of asserting the page never
   sends it.
4. **Regression test.** `a_separate_input_gives_the_page_its_channel_count_and_input_paths` and
   `a_failed_input_open_says_why_on_the_card` (`crates/ui-studio/src/audio_devices/io_paths_tests.rs`)
   drive the app over a scripted non-duplex `FakeBackend` named ALSA with a 32-channel capture;
   with the `sync_io` adoption removed the first fails (`inputs` 0, expected 32).
   `the_picker_never_sends_a_path_without_channels` asserts the last channel's click sends no
   command and no refusal; with `channel_edit` sending the empty list it fails.
   `a_refused_edit_is_said_on_the_page_and_changes_nothing` now uses a command the page may
   legitimately send (an empty name) and asserts the refusal on the page.
   `set_channels_refuses_an_empty_list_and_keeps_the_path` keeps the engine's own refusal pinned.
5. **Gate class.** I/O table behavior is tested over both a duplex and a separate-input fake. A
   refusal test asserts that the UI never sends the invalid command; the engine's refusal of it is
   tested at the engine, not through the page.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`
   (By area). Owner: the maintainer. Blind spots: it is a review rule; no scan tells a duplex-only
   test from one that covers both kinds. CoreAudio, the other separate-input host, shares the code
   path but has no tester report yet. An input device narrower than the saved table drops the
   paths past its width (a separate issue).
