# Automation modes

Read this before touching how the mix reads automation lanes, or how the UI's write pass records
them. Lanes, points and modes live in `crates/model/src/automation.rs`; the mix reads them in
`crates/mix/src/automation.rs` and `crates/mix/src/touch.rs`; the write pass is
`crates/ui-studio/src/automation/pass.rs`.

## The modes

| Mode | Reads the lane | Writes |
|---|---|---|
| Off | No | Nothing |
| Read | Yes | Nothing |
| Touch | Yes, except a parameter whose control is held (below) | The held control, while it is held |
| Latch | Yes | The moved control, from the first move until Stop |
| Write | Not the Volume lane | Volume, and any moved control, while playing |

The mode menu offers Off, Read, Touch, Latch and Write (Logic's terms). The model also has
TouchLatch and Trim, which the menu leaves out.

## Touch suspends the lane while a control is held (#188)

Convention: Logic and Ableton agree. In Logic's Touch mode a touched control replaces its
automation for as long as it is held; on release the parameter returns to the recorded automation
over the Ramp Time set in Logic's Automation settings (Logic Pro User Guide, "Automation modes"
and "Automation settings"). Ableton Live, recording automation with the mouse, records only while
the button is held and lets the existing envelope play again from the release, which its manual
names "touch" behavior (Live Reference Manual, "Automation and Editing Envelopes",
https://www.ableton.com/en/manual/automation-and-editing-envelopes/); Live returns without a ramp.
transit follows both, with a short return ramp as in Logic. Latch and Write are unchanged.

How it works:

- The UI knows what is held: a mixer or header gesture while the pointer is down, or a plug-in
  parameter a hosted editor sent less than `HOSTED_HOLD_S` ago. Every frame, playing or not,
  `sync_touched` (`pass.rs`) hands the parameters held on Touch tracks to `Engine::set_touched`,
  which sets `Mixer::touched`. That field is runtime state like `record_arm` (ADR 0011): never
  saved, no undo step, not dirty, kept across undo and redo, and setting it does not end a running
  gesture or write pass. The changed document reaches the audio thread with the next publish, the
  same path every control move already takes.
- The mix does not read a held parameter's lane: Volume, Pan, send level, insert and instrument
  parameters and the trim lane give the control's static value while
  `touch::held(track, param)` is true (Touch and listed in `touched`). The write pass keeps writing
  the held value as before.
- Each strip keeps a fixed table of held and returning parameters (`TouchRamps`, 8 slots, plain
  `Copy` data). `render_active` advances it once per block before anything reads automation; a
  parameter that was held and no longer is starts its return at that block's first sample. Its
  value is linear from the static value to the lane's value, the lane's weight going from 0 at the
  release to 1 exactly `TOUCH_RETURN_S` of rendered samples later (9,600 at 48 kHz). The table moves
  with the strip into new plan tables (`move_strip`). No allocation, no lock.
- Faders read the ramp at each block's first and last sample, like every fader curve; pans on
  their 64-sample grid; sends at the block start. Plug-in parameters return in block steps (the
  block-start value), and the lane's breakpoints go out at their offsets again from the ramp's end.

The return length is the constant `TOUCH_RETURN_S` (200 ms, `crates/mix/src/touch.rs`). transit
has no Ramp Time setting yet; exposing one is #223.

Not covered:

- A VCA master's fader read from a member strip is suspended while held but has no return ramp
  there (its ramp table lives on the VCA's own strip).
- Mute lanes are not suspended (no mixer control holds them).
- Past 8 parameters held at once on one track, the extra ones are still not read while held but
  return without a ramp.
- The UI's control shows the lane's value again as soon as it is released, while the audio returns
  over the ramp.

Tests: `crates/mix/src/touch_tests.rs` (block-driven, measured in samples, no clock),
`crates/engine/src/lib.rs` `a_held_control_is_runtime_state`, and
`crates/ui-studio/src/automation/pass_tests.rs`
`holding_a_control_on_a_touch_track_tells_the_engine_until_release`.
