# transit-resonator

transit's resonator ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#198): a physical-modeling resonator with six models, ported to Rust from Émilie Gillet's MIT
firmware. `Resonator` is the firmware's part: up to four voices, each a bank of up to 64 modes, a
Karplus-Strong string with sympathetic strings or dispersion, or a two-operator FM voice; the
excitation filters, the noise-burst plucker, the strummed note allocation, a reverb for the
string-and-reverb model and the output limiter. Pure Rust, `#![forbid(unsafe_code)]`, no
allocation once a `Resonator` is built, one random generator per resonator. Not yet played by the
app: the instrument (voice allocation, MIDI, panel) is slice 3 of #198.

| Model | What it is |
|---|---|
| Modal | up to 64 band-pass modes; structure sets the partials' stretch, brightness and damping their decay, position the pickup |
| Sympathetic strings | a plucked string with sympathetic strings at intervals chosen by structure |
| Inharmonic string | a string with dispersion: stiffness (allpass), a curved bridge or delay noise, by structure |
| FM voice | two sine operators; structure sets the ratio, brightness the index, position the feedback; an envelope follower or the strum shapes it |
| Quantized sympathetic strings | sympathetic strings tuned to one of eleven chords |
| String and reverb | the inharmonic string through a stereo reverb |

## Where the code comes from

Written by transit from her C++ (`eurorack/rings/dsp` at `08460a69`, stmlib at `e3bd7c9c`); no
third-party Rust code is in it. Provenance, the file map and the choices the C++ leaves open:
[`VENDOR.md`](VENDOR.md). Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

## The render contract

`Resonator::process(performance, patch, input, out, aux)` renders one block as the firmware's
`Part::Process`: `input`, `out` and `aux` have the same length, 1 to 24 frames (`MAX_BLOCK_SIZE`),
and one call is one control block (the note filter and the strum advance once per call; the module
always renders 24 frames). The patch values are 0 to 1; the notes are finite. Checked with
`debug_assert!`; in a release build a longer slice is rendered as 24-frame blocks with the strum on
the first only, mismatched lengths render the shortest, an empty slice renders nothing.

`PerformanceState` carries what the module's CV scaler and strummer would: the strum, whether the
part excites itself (`internal_exciter`: nothing in the audio input), the tonic, the note over it,
FM, and the chord of the quantized model. Polyphony (1 to 4) and the model are set between blocks
and take effect, with a re-initialization of the voices, on the next block, as on the module.

## Sample rate and pitch

48 kHz only, the firmware's rate: its filters, delay lengths, envelope rates and reverb assume it.
Pitch is `SemitonesToRatio(note - 69) * 440 / 48000`, with no corrected rate (unlike the macro
oscillator), so MIDI note 69 plays 440 Hz. `tests/pitch.rs` measures every model's pitch at
48 kHz with the alias-rejecting measurement of the E63 gate class, at A2, A4 and A5: the modal and
FM paths within 0.2 cent; the strings within 0.8 cent at A2 and A4 (2.3 with the reverb's chorus)
and 1.3 to 2.5 cents flat at A5 (5.9 with the reverb), 6 to 7 cents flat at A6, which is the
firmware's own comb tuning (its delay compensation is approximate; the A6 figure is measured, not
gated); held to 3 cents at A2 and A4 and 8 at A5. This is not a port error: do not correct or
tighten the strings' tuning in the port without changing the oracle first. Other rates are the instrument's question (resampling, or a rate parameter with its
own pitch tests).

## The gate

`tests/null_vs_oracle.rs` renders every model at every polyphony (24 tests) over the oracle's
resonator sweep (`transit_macro_osc_oracle::resonator_sweep`: 159 points of 100 ms: notes 36, 60
and 84; the internal exciter strummed, an external impulse unstrummed and strummed; structure,
brightness, damping and position each at 0, 0.25, 0.5, 0.75 and 1; two FM offsets; 24-frame
blocks; a fresh part and the same seed per point) next to the oracle on the same runner. Both
outputs must be bit-identical (the oracle's floor is `-inf`, the margin zero, the bits compared),
and the random state after each point must equal the oracle's. All 24 pass (Windows x86-64 MSVC,
2026-10-30).

`tests/scenarios_vs_oracle.rs` holds the same rule where the sweep does not reach: model changes in
one part at every polyphony, polyphony changes under strums, the note filter under glides, jumps,
strums and an FM triangle, full-range macro ramps on every model, every block size from 1 to 24,
bypass and a continuous noise input, notes below the strings' delay line (the resampler) and at the
top of the range, and 20 s silences after a strum (subnormal decays).

```sh
cargo test --locked -p transit-resonator --test null_vs_oracle -- --nocapture
RESONATOR_NULL_POINTS="n60 s0.50" cargo test -p transit-resonator --test null_vs_oracle -- --exact modal_1 --nocapture
cargo test --locked -p transit-resonator --test scenarios_vs_oracle -- --nocapture
```

The second form prints the residual and the first differing sample of every point whose label
contains the filter.

## Flush-to-zero

The oracle runs the C++ with flush-to-zero, as the firmware's desktop test harness does
(`rings/test/rings_test.cc`); the module itself runs with FZ off (its firmware never writes FPSCR,
the Cortex-M4 resets with FZ off, and its GCC build fuses multiply-adds), so the reference is the
C++ as the desktop harness builds it, not the hardware's bits. Rust code
always runs in the default floating-point mode, and changing it under Rust is undefined behavior.
So the port's arithmetic is done in `float::F`, an `f32` whose `+`, `-`, `*` and `/` flush a
subnormal result to a zero of the same sign, as x86 FTZ does to every result, everywhere in the DSP
rather than at chosen spots. The 100 ms sweep never reaches the subnormal range; the long-silence
and model-change scenarios do, and fail with the flush turned off (checked 2026-10-30). Not
reproduced: x86's after-rounding tininess edge and AArch64's flushing of subnormal inputs (the
macOS oracle's FPCR.FZ), so a residual on macOS in a decaying tail is blamed on that first.
