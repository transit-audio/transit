<!-- transit-index
status: Open, needs a tester
follow-up: A tester's log with the new figures (the "recording start at input frame ... before the first retained frame" WARN) decides which clock disagrees; more than two inputs through PipeWire's ALSA PCMs need JACK (pw-jack) or a hw: device.
-->

Linux recording through cpal ALSA under PipeWire (#429). The "recording start fell outside retained input history" refusal a tester saw at a 2048-frame period is not an undersized history (it holds 4 s at any period and channel count): with consistent device clocks the start cannot precede the first input frame captured after arm, so it means the output's playback-time estimate and the input's capture-time estimate disagree on that machine; the refusal now logs the start, the first retained frame and the gap. A related failure at large periods (a start more than the input clock's 100 ms projection ahead failed to map) is fixed. An input on a device that offers several widths on ALSA (PipeWire's or PulseAudio's "default" and "pipewire" PCMs, plughw) opens stereo, else mono: those widths are an up- or downmix of one source, not separate inputs.
