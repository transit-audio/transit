# ADR 0027: A render with an External Instrument plays in real time through the live device

- **Status:** accepted 2026-10-25 (issue #71, ADR 0020 slice 3)
- **Date:** 2026-10-25
- **Reopen when:** freeze, bounce in place or stems by output path are asked to include hardware (this ADR covers the mix bounce and one stem per External Instrument); a driver drops capture blocks without the audio thread seeing a gap, so a render can be wrong without failing; or the ring (16 channel-seconds: 0.5 s at the 32-channel maximum) is not enough on a slow disk.

## Context

[ADR 0020](0020-external-instrument-device.md) puts a hardware synth in an instrument slot and compensates its round trip C in plan frames (decision 2; C = O + L_bench + B, B = 0 since the K22 fix). Slices 1 and 2 landed: the return plays through the strip with C reported at its head, and Measure stores C per machine. The synth sounds only while it plays, so an offline render cannot include it (decision 9). Until now `file.bounce_mix` refused a range with an unfrozen External Instrument unless the caller passed `without_external: true`.

## Decision

1. **A render that includes an External Instrument plays the session through the live Player at speed 1.** `file.bounce_mix {mode: "realtime"}`:
   - The Player plays from the range start minus a pre-roll of max(C, one block), at the open device's rate and buffer. MIDI goes to each port exactly as in live playback: the same #61 stamp P(R), no offset, the same C in the plan.
   - The audio thread copies each block the render needs into a ring allocated with the Player: Main's channels as the plan rendered them, then each return's device channels as the plan read them.
   - A writer thread places the frames. Session sample s of the mix is the frame rendered at s + A, A being the plan's delay compensation at the first captured block. Software tracks render in the same pass, so the bounce is what the user hears, without the metronome: the click is kept out of Main while a render is armed or capturing, as an offline render never has it (it is not delay-compensated).
   - With `stems: true`, each External Instrument's return is also written to `<mix> - <track>.<ext>`. Its sample s is the frame read at s + C, which is where slice 1 places it for monitoring (ADR 0020 decision 8, before the strip's inserts), scaled by the device's gain.
2. **Refusals, shown in the render dialog before starting** (decision 7's style):
   - "No MIDI port on Model D: choose one on the Out button"
   - "Choose the return input of Model D"
   - "Return In 7 of Model D is not on this interface"
   - "Model D is not measured: measure it, or render uncompensated (its notes land late by the round trip)". `uncompensated: true` accepts it and C is 0.
   - "No audio device", "Needs a duplex device", "Stop the transport to render in real time.", "A measurement is running: wait for it or cancel it.", "MODEL D missing: notes are not sent".
   - In real time: no bus source, no normalize, no fold-down.

   `mode: "offline_without_external"` (the old `without_external: true` still works) is the dialog's "Render without hardware (offline, faster)". With no mode, a range with an External Instrument is refused, naming both modes.
3. **Progress and safety.**
   - The dialog shows "Rendering in real time… bar N of M" with Cancel, and the transport commands are refused while the render plays.
   - Cancel stops playback and releases every External Instrument of the render through the output thread (note-off, Sustain off, CC 123). The partial files are removed before Cancel returns.
   - A device loss ends the render the same way and says so, and so does All MIDI Notes Off ("Render cancelled by All MIDI Notes Off. Nothing was written."), rather than letting the file finish with its notes cut off.
   - The audio thread does what live playback does, plus one copy into the preallocated ring: no allocation, no lock. A full ring drops the block, counts it, and fails the render with "Render failed: the file writer fell behind and 1 block of audio was dropped. Nothing was written: render again."
   - The render also fails, writing nothing, when a block does not follow the previous one (a locate or a loop), when playback stops early, or when A changes during the capture.
   - Files are written as `.part` and renamed when complete.
4. **Determinism and verification** (`crates/playback/src/player/device/realtime_render_tests.rs`, a fake duplex device and a fake MIDI port, pumped in lockstep with no sleeps that pace anything). The fake synth answers C frames after its note's render frame.
   - The bounce is the offline software render plus the return, sample for sample, on the grid within one frame; the stem holds the return at plan position - C at the device gain (-6 dB in the test); with Click on across a beat the bounce has no click energy.
   - Cancel mid-render releases the held note and leaves no file.
   - Each refusal is tested in the Player, the engine and the dialog.
   - An overrun fails the render with the message above.

## Consequences

- A session with a hardware synth bounces as heard, in real time, with one stored number per chain.
- Freeze, bounce in place and stems by output path of an unfrozen External Instrument are still refused or offline (K64). A bounce cannot be normalized in real time (K65). Dropouts are caught only as ring overruns and discontinuities the audio thread sees, not as driver xruns (K66).

## Alternatives rejected

- **Capture the return and mix it offline with the software tracks.** Two passes, and the bounce would not be what the user heard (plug-in state, automation, live sends).
- **Write the files on the audio thread or the UI thread.** The first blocks the callback on disk. The second ties the render to frame timing and dialogs.
- **Wait when the ring is full.** That blocks the audio thread. A file with a hole is worse than a failed render.

## Related

[ADR 0020](0020-external-instrument-device.md) (decisions 2, 7, 8, 9 and slice 3), [ADR 0023](0023-session-compatibility-markers.md), [ADR 0026](0026-cycle-recording-stacks-takes.md), `docs/contributing/knowledge/midi-output.md` (External Instrument), `audio-thread.md`, issue #71.
