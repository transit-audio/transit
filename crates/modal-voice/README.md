# transit-modal-voice

transit's modal voice ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#198 slice 2): a gated exciter-and-resonator voice ported to Rust from Émilie Gillet's MIT firmware.
`ModalVoice` is the firmware's part: a gate envelope drives three exciters summed under the
strength's accent into one of three resonators, then the space control's stereo spread and reverb.
Pure Rust, `#![forbid(unsafe_code)]`, no allocation once a `ModalVoice` is built (`ModalVoice::reset`
returns to the new state without allocating), one random generator per voice. The app plays it as
the Modal Voice instrument (`crates/dsp/src/plugins/modal_voice.rs`).

| Part | What it is |
|---|---|
| Bow | flow noise (a random sign-flipping signal) through a low-pass, driving eight bowed waveguides in the modal resonator and the resonator input |
| Blow | the firmware's noise sample played as grains, a reed-and-bore tube from two thirds of its level, a four-allpass diffuser |
| Strike | the firmware's nine percussion samples, a mallet pulse, a plectrum or bouncing particles, picked by its meta control; past 80% its raw signal bleeds into the output |
| Modal | 52 band-pass modes; geometry sets the partials' stretch, brightness and damping their decay, position the pickup comb; a side channel read at an LFO-swept position |
| String | one Karplus-Strong string with dispersion (stiffness, a curved bridge or noise, by geometry) |
| Strings | five strings tuned to one of eleven chords picked by geometry |
| FM pair | the firmware's hidden voice: two phase-modulation pairs at eight times the rate, a swept multimode filter, a rotating spatializer (replaces the exciters and resonator) |

## Where the code comes from

Written by transit from her C++ (`eurorack/elements/dsp` at `08460a69`, stmlib at `e3bd7c9c`); no
third-party Rust code is in it. The stmlib helpers it shares with the resonator come from
`transit-resonator`. Provenance, the file map and the choices the C++ leaves open:
[`VENDOR.md`](VENDOR.md). Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

## The render contract

`ModalVoice::process(performance, blow_in, strike_in, main, aux)` renders one block as the
firmware's `Part::Process`: all four slices have the same length, 1 to 16 frames
(`MAX_BLOCK_SIZE`), and one call is one control block (the envelope, the gate edges and the
exciters' control rates advance once per call; the module always renders 16 frames). The panel
controls (`Patch`, set with `patch_mut`) are 0 to 1, space 0 to 2; the module's CV scaler keeps
most at or below 0.9995 and the port clamps a table index the firmware would read past. Checked
with `debug_assert!`; in a release build mismatched lengths render the shortest prefix and an empty
slice renders nothing.

`PerformanceState` carries what the module's CV scaler would: the gate (the exciters' envelope
follows it, the strike fires on its rising edge, bow and blow sustain while it is high), the note
(MIDI semitones), the FM input (semitones added to the note) and the strength (0 to 1, the module's
pressure input: the accent through the firmware's 3 dB/V tables). The model and the FM pair voice
are chosen between blocks. `seed` scrambles three words into the per-unit values the module derives
from its serial number (the side channel's LFO, the reverb's diffusion and tone, the exciter
signature); unseeded, the part keeps `Init`'s values.

## Sample rate and pitch

32 kHz only, the firmware's rate: its filters, delay lengths, envelope rates and reverb assume it.
The frequency is read from the firmware's table of `440 * 2^((note - 69) / 12) / 32000` (capped at
12 kHz), so MIDI note 69 plays 440 Hz. `tests/pitch.rs` measures the modal bank, the string and the
FM pair at 32 kHz with the alias-refusing measurement of the E63 gate class, at A2, A4 and A5: the
modal bank and the FM pair within 0.1 cent; the string -0.85, -1.34 and -4.79 cents (the firmware's
own comb compensation, as on the resonator), held to 3, 3 and 8 cents. This is not a port error:
do not correct the string's tuning in the port without changing the oracle first. The instrument
runs the port at 32 kHz and converts its output at the edge, with its own pitch test at every
session rate.

## The gate

`tests/null_vs_oracle.rs` renders the three models and the FM pair (4 tests) over the oracle's
modal voice sweep (`transit_macro_osc_oracle::modal_voice_sweep`: 207 points of 250 ms: notes 36,
60 and 84; each of the fourteen controls and the strength at five levels around a center; each
exciter alone; two FM offsets; two serial seeds; both audio inputs patched; a gate pattern with a
strike, a release and a retrigger; 16-frame blocks; a fresh part and the same seed per point) next
to the oracle on the same runner. Both outputs must be bit-identical (the oracle's floor is
`-inf`, the margin zero, the bits compared), and the random state after each point must equal the
oracle's. With `TRANSIT_FULL_ORACLE=1` all 207 points run, otherwise every tenth.

`tests/scenarios_vs_oracle.rs` holds the same rule where the sweep does not reach: a 20-second
performance per configuration with every control moving, model and voice changes mid-run, every
block size from 1 to 16, 20 s of decay after a strike (the subnormal range), bypass and patched
inputs with a seeded part. A mutation of one reverb tap or of the bow's threshold fails the nulls;
turning the flush off fails the decay and FM pair scenarios (checked 2026-11-01).

```sh
TRANSIT_FULL_ORACLE=1 cargo test --locked -p transit-modal-voice -- --nocapture
```
