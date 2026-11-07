# cpal and native WASAPI name equality (ADR 0046, slice 2)

The native WASAPI enumerator lists the same endpoints, defaults, buffer sizes and sample rates as cpal on the maintainer's machine, run 2026-11-07 at main `3486dff1` plus the slice 2 branch. This is the precondition [ADR 0046](../../adr/0046-retire-cpal-from-windows-builds.md) names for slice 3 (selection without fallback). The test is `backend::wasapi_names::cpal_and_native_wasapi_list_the_same_names` in `transit-playback`; it passed.

## Outputs

cpal outputs (6):

```
Speakers (2- Focusrite USB Audio) (default)
1 - LG HDR 4K (AMD High Definition Audio Device)
Speakers (Steam Streaming Microphone)
Speakers (Steam Streaming Speakers)
Digital Audio (S/PDIF) (High Definition Audio Device)
Speakers (Model 12)
```

native outputs (6):

```
Speakers (2- Focusrite USB Audio) (default)
1 - LG HDR 4K (AMD High Definition Audio Device)
Speakers (Steam Streaming Microphone)
Speakers (Steam Streaming Speakers)
Digital Audio (S/PDIF) (High Definition Audio Device)
Speakers (Model 12)
```

- cpal outputs: no duplicate names. native outputs: no duplicate names.
- Output default: cpal `Speakers (2- Focusrite USB Audio)`, native `Speakers (2- Focusrite USB Audio)`.
- Every output endpoint offers the same buffer sizes and sample rates in both: buffer sizes 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192 on all six; rate 48000 on all but `Speakers (Model 12)`, which is 44100.

## Inputs

cpal inputs (3):

```
Analogue 1 + 2 (2- Focusrite USB Audio)
Microphone (Steam Streaming Microphone)
Microphone (Model 12) (default)
```

native inputs (3):

```
Microphone (Model 12) (default)
Analogue 1 + 2 (2- Focusrite USB Audio)
Microphone (Steam Streaming Microphone)
```

The listings hold the same names; the native enumerator lists the default first, so the order differs.

- cpal inputs: no duplicate names. native inputs: no duplicate names.
- Input default: cpal `Microphone (Model 12)`, native `Microphone (Model 12)`.
- Every input endpoint offers the same buffer sizes and sample rates in both: buffer sizes 32 through 8192 as above; rate 48000 for `Microphone (Model 12)` and `Analogue 1 + 2 (2- Focusrite USB Audio)`, 44100 for `Microphone (Steam Streaming Microphone)`.

## Run

The `transit-playback` library tests: 5 passed, 0 failed, 1 ignored (`hardware_player_opens_native_wasapi`, which opens the default endpoint).
