# The device-rate converter held the last sample of every rendered block (#525)

1. **Failure.** Whenever the converter between the session rate and the device rate was active
   (the device does not offer the session's rate and its default rate is used, or playback runs at
   a speed other than 1), the output frame that fell between the last frame of one rendered block
   and the first frame of the next repeated the last frame instead of interpolating toward the
   next. That is one wrong sample per rendered block, about one per device callback: on tonal
   material a faint buzz at the callback rate. A ramp played from a 44.1 kHz session on a 48 kHz
   device in 512-frame callbacks had 107 wrong output frames in 51,200; at half speed, or from
   48 kHz on a 96 kHz device, one per callback, each short by half a step.
2. **Mechanism.** `crates/playback/src/player/audio.rs` (`fill_inner`) rendered the next block only
   once the read phase had passed the whole pending block (`Phase::covers`). An output frame at a
   fractional position after the block's last frame had no next frame to read, so it used the last
   frame for both points of `a + (b - a) * fraction`. The output depended on where the stream was
   cut into blocks.
3. **Why checks missed it.** The converter's tests ran at equal rates, where every read is a whole
   frame, or compared a pattern within 1e-5. One test
   (`the_linear_converter_never_interpolates_from_unrendered_storage_after_a_short_loop_block`)
   guarded against reading unrendered buffer capacity and pinned the held frame as the right
   value. No test compared the output across block partitions or with a closed form.
4. **Regression test.** In `crates/playback/src/player/audio/tests/converter.rs`:
   `a_ramp_through_the_player_is_exact_at_every_output_frame` (the issue's case through the
   player's fake device: 44.1 to 48 kHz, 48 to 44.1 kHz, 48 to 96 kHz and half speed, every output
   frame on the closed form within one rounding and bit-identical to the reference),
   `the_player_output_is_independent_of_the_callback_partition`,
   `the_converter_output_is_independent_of_the_render_partition_at_every_rate_pair` (44.1, 48,
   88.2, 96 and 192 kHz in both directions, equal rates and a 44.1 to 47.999 kHz ratio, at speeds 1
   and 0.5, render blocks of 1, 7, 64, 480, 4096 frames and a mixed sequence, compared bit for bit
   with a closed-form reference) and `equal_rates_pass_every_bit_through_whatever_the_partition`
   (with `equal_rates_through_the_player_are_bit_exact_pass_through`). The short-loop test now
   expects the interpolation toward the loop's first frame. On the code before the fix (a
   `git archive` copy in a fresh target) the ramp test counts the issue's 107 wrong frames and the
   partition test fails; reverting the new render condition in the fixed code fails seven tests.
5. **Gate class.** A block-based stream stage gives output that is a function of its input stream
   alone: tested by bit-identical output over many block partitions against a closed-form
   reference, across the rate matrix, at the primitive and through the live player.
6. **Protocol change, owner, blind spot.** The converter now renders the next block as soon as an
   output frame interpolates into it and keeps the previous block's last frame per channel in
   fixed storage (`crates/playback/src/player/audio/phase.rs`, `Hold`), cleared on every start,
   locate, stop and transport end; a new device or stream starts with a new converter. A whole-frame
   position returns that frame exactly, so equal rates stay bit-exact pass-through. The playback
   gate owns the tests. Blind spots: the converter is still linear interpolation with no
   anti-alias filter, and its measured quality is pinned by
   `the_linear_converter_quality_is_measured_and_pinned` (see the known issue
   [device-rate-converter-quality](../known-issues/entries/device-rate-converter-quality.md));
   at a change of the Main or sink channel layout, the one output frame between the old plan's last
   block and the new plan's first interpolates from the old layout's last frame (channels it did
   not have read as silence).

Measured (tones at -6 dBFS, deterministic; level where the tone is heard on the device, and the
tone over everything else in the output):

| Session to device | Tone | Level | Tone over the rest |
|---|---|---|---|
| 44.1 to 48 kHz | 1 kHz | -0.015 dB | 62.4 dB (49.4 dB with the defect) |
| 44.1 to 48 kHz | 10 kHz | -1.50 dB | 20.4 dB |
| 44.1 to 48 kHz | 20 kHz | -6.34 dB | 3.1 dB |
| 48 to 44.1 kHz | 1 kHz | -0.012 dB | 63.9 dB |
| 48 to 44.1 kHz | 10 kHz | -1.26 dB | 22.2 dB |
| 48 to 44.1 kHz | 20 kHz | -5.28 dB | 5.6 dB |
| 48 to 44.1 kHz | 23 kHz (alias at 21.1 kHz) | -7.14 dB | (alias rejection 7.1 dB) |
| 48 to 96 kHz | 1 kHz | -0.009 dB | 59.4 dB |
| 48 to 96 kHz | 20 kHz | -4.02 dB | 4.6 dB |
| 96 to 48 kHz | 1 kHz and 20 kHz | 0.000 dB | above 130 dB (exact 2:1 pick) |
| 96 to 48 kHz | 30 kHz (alias at 18 kHz) | 0.000 dB | (alias rejection 0 dB) |
