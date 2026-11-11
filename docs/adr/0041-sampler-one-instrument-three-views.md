<!-- transit-index
title: One Sampler instrument with three views (Instrument: zones, velocity layers, round robins, loops; Pads: per-pad sample, tune, reverse, envelope, choke groups, mute and solo, one stereo output; Slices: WAV cue point, transient and beat-grid markers mapped to pads and notes, Slice to MIDI in two timing modes); per-pad outputs and inserts through an Expand to rack command (ADR 0034), not inside the instrument; the Macro Oscillator's shaper and matrix reused; long samples through disk-backed sources (ADR 0035); WAV, AIFF, FLAC and SFZ in, no REX or proprietary kits; pitched playback first; the existing sampler is the seed and old sessions load
-->
# ADR 0041: One Sampler instrument with three views (Instrument, Pads, Slices)

- **Status:** accepted 2026-11-04 (proposed 2026-11-01; decided by the maintainer with the coordinator; slices filed); amended 2026-11-03; amended 2026-11-08, accepted; amended 2026-11-09, accepted, pad start note; amended 2026-11-11, accepted, the one-sample instrument retired (#455); amended 2026-11-11, accepted, kit presets (#467); amended 2026-11-11, decided by the maintainer, click plays a slice and double-click adds a marker (#469); amended 2026-11-11, decided by the maintainer, playheads on the Slices bench (#470)
- **Date:** 2026-11-01
- **Reopen when:** a pad needs its own insert chain or output often enough that "Expand to rack" is the common path rather than the exception; the shaper and matrix cannot serve per-pad or per-zone modulation; streaming long samples ([ADR 0035](0035-disk-backed-audio-sources.md)) cannot meet the voice count on the reference hardware; or a rack-native sampler ([ADR 0034](0034-racks-layering-instruments-through-track-groups.md)) makes the Pads view redundant.

## Context

transit has one small sampler (`crates/dsp/src/plugins/sampler.rs`): one sample across the keyboard, 32 voices, 4-point cubic interpolation, key and velocity ranges, an amp ADSR, a start point, forward or ping-pong loops and gate or one-shot playback. The host hands it the audio through `Plugin::set_sample`. It cannot play a drum kit, slice a loop, layer velocities or hold more than one sample.

Three requests converge on it. A beat slicer in the style of a REX loop player (#267: slices from WAV cue points or detected transients, mapped to keys, with a command that writes the notes that rebuild the loop). Pitched, multisampled sounds (zones, velocity layers, round robins, loops), which also covers the request for a General MIDI SoundFont instrument (#33). And sampling what the computer plays (#268), whose captured clip should become an instrument in one step.

Convention (transit follows what Ableton and Logic agree on, as recorded when this ADR was proposed). Both split sampling by task and not by product. Ableton's Simpler has Classic, One-Shot and Slicing modes in one device (slices by transient, beat, region or manually, played from a key range, with a Slice to New MIDI Track command), and its Sampler is the multisample instrument. Logic's Quick Sampler has Classic, One Shot and Slice modes in one instrument, and its Sampler is the multisample instrument. For drums both use a rack of samplers: Ableton's Drum Rack is a rack of chains each holding a Simpler or Sampler (with choke groups and per-pad outputs and inserts), and Logic's Drum Machine Designer is a track stack of Quick Samplers (with per-pad outputs, mute and solo, and a choke setting for hi-hat style groups). Both agree that the one-sample instrument has modes in the same panel and that per-pad routing is a rack feature. The maintainer wants the common case light: one picker entry, one panel, per-pad routing only when asked for.

The maintainer made the Sampler mandatory for the composition release before the performance milestone (2026-11-03): full song composition and triggering samples. Issues #269 through #272 are release work; #273 is not. This revision specifies those slices; it does not claim they are implemented. The format decision is independent of lossy export.

## Decision

1. **One instrument, one picker entry: Sampler**, id `sampler`. Instrument, Pads and Slices
   are views of the same patch, not playback modes. Changing views changes no routing, parameters
   or notes. Macro Oscillator remains the default instrument (ADR 0033).
2. **One bounded zone model.** A zone is one sample region, its key/velocity range, root,
   playback settings and optional layer/round-robin membership. A pad binds one zone to one
   trigger note; a slice supplies that zone's region in a shared source. Pads never stack a loose
   sample and a slice. Instrument view edits all zones, including pad-bound ones. Unbound zones
   remain audible in every view; Pads shows that they exist rather than silently hiding their sound.
3. **Pads and Slices first.** Four banks of sixteen pads, one voice per pad, choke groups and
   editable slices cover composition and triggering. Slice to MIDI creates an ordinary clip on
   this track. It is not an ADR 0036 MIDI effect and creates no generator in the device chain.
4. **Instrument view adds multisampling.** Key and velocity zones, fades, sequential or seeded
   random round robins, forward/ping-pong loops and forward-loop crossfades. SFZ is the open
   interchange in #271. SF2, release triggers and time stretching remain in #273.
5. **Per-voice shaping, one stereo output.** Reuse ADR 0033's shaper and 16-row matrix through
   a portable shared module, not a copy. Per-pad insert chains and routing belong to ADR 0034's
   rack member tracks, reached by Expand to rack. Expansion preserves choke or refuses atomically;
   it never silently drops a pad or a choke group.
6. **Realtime work is bounded.** Decode, analysis, media copying, priming and patch construction
   run off-thread. Long samples use an extension of ADR 0035's streamer for Sampler readers,
   described below; its existing clip API alone cannot play pitched zones. Missing data produces
   silence and diagnostics, never IO or a wait in process.
7. **Plain files.** WAV, AIFF and FLAC are audio inputs; kits are folders, with an optional
   open text mapping. No proprietary sample/kit parser or vendored SDK, including NI formats or
   REX. Other audio formats already accepted by the importer are not removed. SFZ import is not
   a commitment to General MIDI banks. Product names in this record describe conventions only.
8. **Old work keeps its sound.** Keep the existing `sampler` registry entry, parameter ids,
   automation addresses and `Insert::sample`. An old one-sample instrument is presented as one
   Instrument zone but retains its legacy renderer until explicitly converted by a structural
   patch edit. Opening, saving or switching views never performs that conversion.

## Decided questions

| Question | Decision and reason |
|---|---|
| Pad count and layout | 64 pads, banks A–D, 16 visible in a 4×4 grid. Number from bottom left across each row, then upward (the MPC and Maschine 4×4 convention). Four banks fit a useful loop without a crowded panel; they do not enlarge ADR 0034's rack limit. |
| Controller mapping | A 16-pad controller addresses visible slots 1–16 of the selected bank, one to one. A pad without a stored trigger note plays the patch's pad start note plus its index (see "Amendment 2026-11-09: the pad start note", which replaces the earlier fixed 60–123); bank changes do not rewrite stored notes. A press captures its destination and note identity until release, even across bank changes. Ordinary MIDI notes always address the stored trigger notes directly. |
| Choke groups | Off plus groups 1–16, labeled numerically; color is supplementary. Groups cross banks. An admitted trigger fades every other sounding pad in its group over exactly 32 output frames. This allows independent hi-hat and percussion groups without dependence on UI colors. |
| Pad voice model | Monophonic per pad; retrigger cuts the previous voice at the event frame and starts at the region entry with a fresh envelope. No layered retrigger option in #269. This keeps drums predictable and expansion reproducible. The short attack/declick can soften the new attack but does not promise a click-free hard cut. |
| Dragging slices | Allowed between pads, including banks. Move source/region/slice-id/name into an empty pad's zone; swap those contents for an occupied target. Pad identity, zone identity, trigger note, automation address and all playback/mixer settings stay at the destination. This is an explicit remap, one undo step, with no change to existing MIDI. Marker order stays chronological. No silent overwrite and no two sources on a pad. |
| Expand naming | Explicit pad name, else slice label, else sample stem, else `Pad A1` (bank and slot). Duplicate names get ` (2)`, ` (3)` in pad order. Names are labels, never event identities. |
| Choke after expansion | Add the narrowly scoped rack choke contract below in #272, an amendment to ADR 0034's member model. Parent events, members' own clips, direct live input and pad/audition hooks all enter one choke/solo admission stage. Members keep their ordinary composition inputs; none may bypass choke. Never drop choke with a warning: that would change an open/closed hi-hat pattern. |
| Velocity to slice start | Deferred to #273, including indirect matrix routes through Via. No Sample Start destination in the first four slices; start/end remain explicit editing controls. This avoids changing attacks on streamed notes and makes the deferral enforceable. |
| Pitch and different sample rates | Retain native-rate PCM and resample in process with the existing four-point cubic arithmetic. Playback step is `source_rate / session_rate * 2^(semitones/12)`, bounded to the existing maximum 256 source frames/output frame. Pads default to key tracking off, zones on. Rate conversion and pitch share one interpolator; no per-slice time stretch. Already converted legacy sources keep their stored PCM. |
| Zone/layer/round-robin bounds | 256 sample-bearing zones total, including pad zones and every round-robin alternative; at most 8 velocity layers per key-map group and 8 alternatives per layer. These are nested limits, not 256 × 8 × 8 sample slots. At most 8 layer voices start for one note; reject maps that could exceed that, before publication. |
| Loop behavior | Forward and ping-pong, half-open frame boundaries; optional linear overlap on forward loops only. Ping-pong plus crossfade is refused, not silently approximated. Exact rules below make render comparisons meaningful. |
| Save model | Reference original paths by default, offer Copy into session; preserve session-owned/recorded media. Kits are folders with WAV/AIFF/FLAC and optional SFZ. Autosave stores the entire patch and references, and preserves newly created media before publishing the reference. No sample bytes in JSON. |
| Presets and credits | Ship no sample kits or sample-dependent factory presets in these slices. An empty initialized Sampler is the default, not a playable preset. Users load plain folders or SFZ. Reuse transit's existing detector; no new external detector port or credit is introduced. Existing notices remain. |

### Playback, controls and modulation

New pads default to one-shot, key tracking off, root equal to their trigger note, level 0 dB,
pan center, tune 0, full region, reverse off, mute/solo off and choke Off. Zones created in
Instrument view default to Gate, key tracking on, root 60, keys 0–127 and velocities 1–127.
Per pad/zone controls are level (-48 to +12 dB), pan (-1 to +1, balance with unity at center),
tune (-48 to +48 semitones plus -100 to +100 cents), start/end (integer source frames), reverse,
One Shot/Gate, attack (0–5000 ms), decay (0–10000 ms), sustain (0–100%, default 100%), release
(0–10000 ms), and declick (0–10 ms). New defaults are attack 1 ms, decay 300 ms, release 150 ms,
declick 0. The extra sustain control avoids an implicit fixed decay target. Sample ends always
stop the voice; a one-shot ignores key-up and sustain, disables looping, and ends at its region
boundary. Gate loops while held or sustained, then follows release while continuing its loop.
All Sound Off cuts immediately; All Notes Off releases Gate voices and fades one-shots over
32 frames. Stop/seek/reset must also end one-shots; reset clears all voice and controller state.

For new patches, convert envelope durations to `round(ms * session_rate / 1000)` frames once
when settings change. Linear attack reaches 1 on its Ath frame; zero attack starts at 1.
Linear decay reaches sustain on its Dth frame; zero decay selects sustain immediately.
Release captures current gain G and emits `G * (1 - (j + 1)/R)` for j = 0..R-1, then retires;
zero release retires at the event. Choke uses that same rule with R = 32, applied after shaping
so filter tails cannot escape it. Declick multiplies the first/last F frames by the corresponding
linear ramp, F bounded by half the region's rendered length. The legacy ADSR, exponential
release threshold, interpolation boundaries and 32-voice selection remain unchanged in legacy
mode; a pure move must not replace them with these new envelope rules.

Pad retrigger matches the pad regardless of channel. Its replacement records the new host note
id/channel, so an old note-off cannot stop it. On a host without note ids, use the existing host
note table's matching semantics. Muted pads, or non-solo pads while any pad is solo, admit no
new voice and do not choke another pad. Muting a sounding pad uses the 32-frame choke fade.
At a shared offset preserve incoming event order; the last admitted trigger in a choke group
wins. Before allocating new zone voices, choose free slots in index order, then oldest release,
then oldest sounding voice, ties by slot index; stealing cuts immediately. Render sums in
stable zone/pad order, then voice age, rather than whichever slot happened to become free.

Unbound Instrument zones select by inclusive original key and velocity. Fade factors use
ADR 0034's `(k + 1)/(n + 1)` rule and scale velocity (round, minimum 1), not post-voice gain;
selection happens before fades. A key-map group has one key range, up to eight velocity layers,
and each layer one to eight mutually exclusive round-robin alternatives with identical ranges.
Different groups may overlap only within the eight-selected-layers-per-note bound, counting
any pad triggered by the same note as one layer. Pad-bound zones are selected only through
their binding, never a second time through the unbound-zone scan. Sequential
round robin starts at index 0; random uses a stored u32 seed and a defined xorshift32 step
(13, 17, 5), choosing `word % count`. Zero seed normalizes to 1. Advance once per admitted
layer trigger, not per rendered frame; reset/open/export restart counters and RNG. Selection
includes silent missing-media zones so relinking does not reorder the other alternatives.
Per-zone Velocity Sensitivity is 0–100%, default 70%, using the existing gain rule
`(1-sensitivity) + sensitivity*scaled_velocity/127` before the amp envelope. A zero-velocity
note-on is a note-off. Validate MIDI keys/velocities before indexing any selection table.
Pad-bound zones always have one key, one velocity layer and no round-robin group; turning one
into a multisample group explicitly unbinds it from its pad. Changing views alone never does.

There is one shaper/matrix settings block per zone (a pad edits its bound zone), with runtime
envelopes, filter memories and two LFOs per voice. Sources are the voice's key, velocity,
release velocity, channel controllers, note pressure, seeded note random, envelopes and LFOs,
plus the eight rack macros. Destinations are Pitch, Level, Pan, cutoff, resonance, filter-envelope
amount and both LFOs' rate/amount. No unrelated oscillator destinations or sample-start route
is exposed. Pitch changes playback rate, not duration compensation. The source envelope and
the shaper amp stage are one amp envelope, applied exactly once; enabling the filter does not
double-envelope a pad. With the shaper off and matrix empty the unshaped path is bit-exact.
Reuse shared shaper algorithms with an external amp-gain input for this path; retain ADR 0033's
default behavior in its adapters. Its held-note 12-frame switch fade remains, followed by the
Sampler's final choke gain.

Events act on their exact output frame. Modulation runs on a persistent 12-output-frame grid
plus note starts, independent of host block boundaries; amp/choke gains advance each sample.
Transport anchors and tempo/meter splits follow ADR 0038. Equivalence claims use identical
absolute event/parameter/transport schedules: ordinary host-block parameter automation is not
magically sample-accurate. Preserve ADR 0033's distinction between reproducibility from reset
and history-dependent random LFOs when locating into a song. Inserts receive the accepted
ordinary note stream through ADR 0038; internal voice stealing/choke is not a synthetic MIDI
note-off to a paraphonic insert.

### Slice sources and Slice to MIDI

Slicing has two modes inside the Slices view: **Auto** and **Manual**, each owning a separate
marker set for the same loop. Auto owns the detectors and its markers are read-only. Source
order is valid WAV cue points/labels, then **the existing**
`transit_dsp::offline::detect_transients`, then beat-grid division. Auto chooses the first
nonempty usable result; the user can explicitly choose any of those three sources. Cue
positions are source-frame indices: bound-check RIFF chunk sizes, sort, deduplicate, discard
out-of-range positions with a load note, retain labels by cue id, add region start and end.
Use `cue ` and `LIST/adtl/labl`; end is a boundary, not a zero-length slice. Source/sensitivity/
grid changes prepare a candidate Auto result off-thread; **Replace markers** explicitly accepts
regeneration of Auto only, as one undo step. Until accepted, the bench and its actions keep
using the current Auto set. Reopening restores the saved sets without rerunning detection.

Manual owns the user's markers, for placing every breakbeat chop by hand. Clicking empty bench
space adds a marker, dragging a marker moves it, and clicking a marker without dragging removes
it. The first marker is fixed at loop start S: it cannot move or be removed; loop end E is a
fixed boundary, not an editable slice start. **Start from Auto's markers** replaces Manual
with a copy of Auto's current accepted set, including labels; **Clear** leaves only S (and the
implicit end E), one full-loop slice. Selecting Manual for the first time starts with S only.
Switching Auto/Manual restores that mode's own set without changing either set. Touching a
marker in Auto copies Auto into Manual and switches to Manual before applying the gesture;
an empty-bench add in Auto does the same. This explicit edit replaces any previous Manual set,
with the copy, switch and gesture in one undo step. Touching S still switches/copies but cannot
move or remove S. Copy, Clear and each add/remove/complete drag are undoable; canceling a drag
restores its pre-gesture state, including any copy and switch.

The bench offers horizontal zoom at **1×, 2×, 4× and 8×** with horizontal scrolling, preserving
the center source position when zoom changes where the scroll bounds allow. The pointer/drag
position readout is in milliseconds relative to S, computed from native source frames and rate;
zoom and scrolling never change stored positions. **Snap to zero crossing** is a toggle,
default on, for Manual adds and moves. Round the requested position to the nearest source frame
(ties upward), then search within **±5 ms**, `floor(source_rate * .005)` frames on either side,
clipped to the loop and legal neighbor interval. A crossing is frame n where adjacent samples
n-1 and n change sign in any source channel (zero counts as nonnegative); do not sum stereo
channels. Choose the nearest crossing to the requested frame, ties to the earlier frame. If
none exists, use the requested frame; with snap off, use it directly. Missing source data
refuses a snapped edit with a status message rather than pretending to snap. Searches and any
source reads run off the audio thread.

Minimum spacing is **one native source frame** between adjacent boundaries, including S/E,
so close cue points and hand-placed flams remain possible. A move stays between its existing
neighbors; clamp its requested frame to that legal interval before searching. An add outside
(S,E) or on an existing marker is refused, never merged or silently relocated. The accepted
position must satisfy spacing after snapping. Moves keep stable slice ids and strict order.
Removing a marker merges into the preceding slice and frees the following binding; references
to the removed slice are reported. References distinguish the two sets, so an edit or replacement
of one set cannot mutate the other's identities or bindings. Copying preserves positions and
labels but creates Manual identities; replacing Manual reports/frees its removed bindings by
the same rule. Never overwrite Manual on detection or reopening.

**Slices to pads** and **Slice to MIDI** consume the set in the mode currently showing, never
a hidden set or an unaccepted detector preview. Switching modes alone does not remap pads or
rewrite existing MIDI. Slice to MIDI requires the shown set's current regions to have the
ascending pad/key mapping below; if absent or stale, it refuses and offers **Map slices to keys**
as a separate edit. Applying a set to pads previews any replaced slice bindings and preserves
the existing collision, capacity and undo rules.

The existing detector is transit code, a vibrato-suppressed spectral flux detector, not the
tentative 5 ms energy detector in the original proposal. Reuse its sanitized channel power
sum/average, Hann FFT window `next_power_of_two(clamp(floor(rate * .012), 64, 8192))`, hop N/4,
24 bands/octave, local peak neighborhood 30 ms, mean neighborhood 100 ms before/70 ms after,
30 ms minimum gap on the hop grid, and attack refinement. Sensitivity 0–1 (default .5) sets
`delta = 2.5 * 2^(4*(.5-sensitivity))`. Opposite-phase stereo cannot cancel. Retain these
parameters in #269; changing the detector for flams is a separate measured change with the
arrangement's existing transient tests, not an unreviewed second algorithm. Very close flams
can require manual markers. There is no automatic tempo estimator in this slice.

Beat division offers 1/4, 1/8, 1/16 and 1/32 notes, default 1/16. Use valid finite positive WAV
`acid` tempo metadata, else the session tempo at the insertion point, with a visible override;
use only tempo, not proprietary beat data. Derive each boundary from the absolute rational
division at that BPM and round to the nearest source frame, rather than accumulating rounded
steps. Clamp to the region, deduplicate, append end. All analysis runs off-thread, bounded by
the same media admission rules; long sources in #271 are read in overlapping analysis chunks
with sufficient detector neighborhoods and deterministic merge, not decoded wholesale.

Auto's Grid controls include **Division** and **Bars**. Bars defaults to **From tempo**, keeping
the metadata/insertion-BPM rule above. An explicit integer bar count 1–16 overrides the grid's
tempo: with insertion meter numerator u and denominator v, B = bars * u * 4/v quarter notes
span [S,E), and grid BPM = `60 * B * source_rate / (E-S)`. Show this derived BPM. Returning to
From tempo restores the BPM rule; neither control estimates tempo from audio or alters the
Manual set. Keep the rational boundary calculation, including partial final divisions, and
refuse a result over capacity instead of dropping markers. This grid override does not silently
change Slice to MIDI's separately chosen beat length or either clip timing mode below.

One loop per patch, at most **64 slices and 65 boundaries per set**, for both Auto and Manual.
Each set contains at most 64 slice starts including S, plus the end boundary E. The two saved
sets are alternatives, not 128 playable slices or extra pads. In Manual, an add that would
create the 65th slice leaves the set unchanged and says **Cannot add slice: 64 slices per loop**
in the status line. Auto regeneration and copy-from-Auto obey the same bound; no truncation.
The loop may coexist with loose sample pads,
but total occupied pads cannot exceed 64. Slicing previews any shortage before applying and
refuses excess instead of truncating. Defaults map chronological slice i to root+i, the root being the patch's pad start note unless a root is stored (replaced by Amendment 2026-11-09: it was 60);
root is selectable in 0..(128-count). A mapping edit may relocate the entire ascending run,
but refuses collisions with loose pads. Moving a slice between pads can break ascending order.
Slice to MIDI then refuses and offers the separate **Map slices to keys** edit: it does not
quietly remap a performance already recorded. For an ascending mapping, its chosen root must
match that mapping. The generated clip has exactly one note per slice, velocity 100, channel 1,
with key root+i and duration to the next boundary (last to loop end), minimum one tick.

For source region [S,E), source rate r, insertion sample P and chosen beat length B:

- **Fit loop length:** note boundaries in quarter notes are `Q(P) + B*(marker-S)/(E-S)`.
  B defaults to the region's duration times metadata BPM/60, else insertion BPM/60, and is
  editable. Store tick positions using the session PPQ, nearest tick, ties upward.
- **Keep original duration:** target sample boundaries are
  `P + round((marker-S)*session_rate/r)`; convert each through the current tempo map to ticks.
  This honors tempo changes already inside the range. Both modes store ordinary MIDI ticks;
  later tempo edits change playback duration, not the stored notes. A collapsed pair after
  rounding is refused with a zoom/remap message, not silently merged.

Placement defaults to the selection start, else the edit cursor, on this instrument track's
active playlist. It is one **Slice to MIDI** undo transaction including the sequence and clip.
For overlap, duplicate the active playlist, number covered material with `takes::number_covered`,
place the generated clip as a new take, and settle it with `takes::settle_new_takes`/the ordinary
ADR 0039 activation path. Keep whole old clips on the alternate playlist, preserve material
outside the range and keep the original playlist active. With no overlap, insert directly
without creating a take stack. Empty slices, frozen tracks, invalid notes, missing loop metadata
needed for the chosen calculation or capacity errors fail before changing the document. This
is composition editing, not recording, and never touches a capture journal. A captured clip
from #268 can feed the same load/region command without making #269 depend on system capture.

### Loop points and crossfade reference

Store start/end and loop boundaries as u64 native-source frames. Validate
`start <= loop_start < loop_end <= end <= source_frames`; a loop needs at least two frames.
Playback starts at start (reverse: end-1). Forward loops wrap at loop_end to loop_start;
reverse forward-loop playback wraps below loop_start to loop_end-1. Ping-pong traverses the
endpoints once, reflecting on the interval [loop_start, loop_end-1] without duplicating an
endpoint. Fractional phase uses the same reflection rule before cubic interpolation; taps map
through that loop rule, and non-loop taps outside the region read zero.
Keep an explicit first-traversal flag: material before loop_start (or after loop_end in reverse)
plays as the intro, not modulo the loop. Before the first wrap/reflection, non-crossfade taps
use the original region at the entry boundary; subsequent loop taps wrap/reflect. Compute
multi-wrap phase with modulo/reflection arithmetic, never a loop whose work grows with pitch.

Forward crossfade length X is an integer source-frame count, default 0, maximum
floor((loop_end-loop_start)/2). In the final X frames, read outgoing phase p and incoming phase
`loop_start + (p-(loop_end-X))`; evaluate both with the same cubic reader, then compute in this
order `a + (b-a)*w`, `w=(p-(loop_end-X))/X`. At wrap continue at loop_start+X, preserving
fractional overshoot; the effective period is loop length minus X, shown in help. Reverse uses
the mirrored coordinates and same arithmetic. X=0 bypasses the blend entirely. The independent
test reference implements these equations; a vague "no audible discontinuity" floor is not a
substitute for sample equality. Legacy loop arithmetic remains in the legacy path.

## Implementation

The maintainer-reviewed [Sampler panel mockup](../design/sampler-panel-mockup.html) is the
design note for slice 1 (#269): Pads has a fixed 4×4 grid column and a selected-pad strip;
Slices has the scrolling Auto/Manual bench; Instrument has a velocity axis and zones above
the keys they cover, with round-robin and velocity-split labels. Build to that layout, with
this ADR's text taking precedence wherever they differ. The mockup's simulated detection,
fractional-loop snap, first-marker handling and example clip position are not implementation
rules. Keep the specified detector/defaults, real sign-change snap, fixed S and ADR 0039
placement above. The full Instrument editor still belongs to #270 and SFZ to #271; the mockup
does not pull those features into #269. Replace it with real screenshots after slice 1 lands.

Progress (2026-11-04): the first #269 step has landed, the `crates/instrument-core` extraction
below (a pure move with re-exports at the old dsp paths, then its Tier B configuration and
bare-metal check in the linux CI job). The external amp-gain seam arrives with sampler-core.

Progress (2026-11-04, second step): `crates/sampler-core` has landed as a portable core (region
reader, cubic, loops, zones and pads, voices, envelopes, choke, the processing over borrowed
spans; Gate 1 at the core level and Gate 6's probes), with the shaper's external amp-gain seam in
`instrument-core` (a parallel entry point; `process` and every existing instrument's bits are
unchanged). The desktop adapter, model patch and compatibility path follow in job C.

### Crates and integration seams

| Owner | Work |
|---|---|
| `crates/sampler-core`, package `transit-sampler-core` (L1, Tier B) | Region reader, cubic interpolation, voices, selection, envelopes, loops, choke and processing trait over borrowed audio/event spans. Descriptive name, no vendor branding. `default = ["std"]`, no-default `no_std + alloc`, no model, file IO, egui or desktop Plugin dependency. **Landed: core; the desktop adapter follows in job C.** |
| `crates/instrument-core`, package `transit-instrument-core` (L1, Tier B) | Pure move of shared shaper/matrix, required SVF/LFO/envelope/curve primitives out of dsp; then add the external amp-gain seam separately. Both dsp and sampler-core depend on it, never sampler-core on dsp. Preserve old module re-exports and numeric behavior. Register layers and the primitive crate's standalone status; sampler-core has this explicit L1 dependency. **Landed: extraction; the external amp-gain seam (`amp_frame`, `filter_frame`, `amp_scale`, `rest_filter` beside an unchanged `process`).** |
| `crates/dsp/src/plugins/sampler/` and registry | Desktop adapter retains `SAMPLER_INFO`/`sampler`, Category Instrument, `is_instrument: true`, `audiosuite: false`; typed parameters, events, transport, rack macros, legacy compatibility and prepared sample-bank handoff. Split the existing file by concern as pure moves before adding code. **Landed: C1, adapter and handoff; the model patch follows in C2.** |
| `crates/model` / `crates/mix` | Serialized patch and source references; prepare the L2 disk-reader implementation behind an L1 borrowed reader interface and pass it down through the adapter. Model may depend on the L1 trait; L1 must not import `DiskSource`. Compile patch/mappings off-thread and retain instance generations, fingerprints, note-view and worker-retirement rules. **Landed: C2, patch, codec, commands (`Insert::sampler_patch` schema 1; the plan compiler owns each instance's publisher).** |
| `crates/engine` / `crates/audio-io` | Atomic load/edit commands, checked filesystem access, cue/tempo metadata, existing decoder reuse, off-thread detector, Slice to MIDI, media collection, SFZ and expansion. Extend the current `cmd/sampler.rs` path as well as `cmd/file.rs` imports. **Landed: C2, patch, codec, commands (`mix.sampler_new_patch`, zone, pad, seed and control commands with admission).** |
| `crates/ui-studio` | One built-in panel split into Instrument, Pads and Slices modules, same editor ownership as neighboring instruments; reusable shaper/matrix widgets. Selection/bank/view are UI state, not playback switches. |

The portable traits accept explicit sample rate, frame position, transport anchors and seeds,
and caller-prepared storage. Use `libm` on the no-default path; no lazy OS initialization,
thread, clock, logger or target-width atomic requirement in either new core. Desktop sample
ownership is `Arc` in the adapter; the processing core receives borrowed readers/handles and
never drops an owning sample reference. Both new crates' dependency edges disable default features and explicitly forward `std` from
their own `std` feature, so an incoming edge cannot undo the bare-metal configuration.
Preserve `Plugin::set_sample` for legacy instances; add a prepared multi-source bank seam
rather than squeezing zones through its single-source API.
Reuse Texture's validation-before-publication and off-thread WAV/restore discipline, not its
freeze capture of mutable audio: Sampler audio is immutable and needs no callback snapshot.

### Capacities and admission

| Resource | Fixed bound and behavior |
|---|---|
| Pads / banks | 64 / 4, 16 visible; unique trigger notes; no pad layering. |
| Zones / layers / round robins | 256 leaves / 8 layers per group / 8 alternatives per layer, all leaves counted in 256; up to 256 distinct sources. |
| Voices | 64 total across pads and unbound zones, including release/choke tails. At most one per pad; new Instrument patches use up to 64, legacy remains 32. No hidden extra tail voices. |
| Slices / markers | 64 / 65 per Auto or Manual set, one shared loop; both sets stored, only the shown set used by slicing commands. Manual has at most 64 starts including fixed loop start, plus end; refuse the 65th slice with a status line. Duplicate or invalid boundaries rejected; no extra playable-pad capacity. |
| Shaper / matrix | 64 voice states, two LFOs per voice, 16 rows per zone; settings for 256 zones allocated in prepare. |
| Events | 1024 per instance, existing 257-entry release reserve. Preserve stable sorting and the host's retry on refusal; no fresh Vec or sort allocation in processing. |
| Expanded-pad rack events (#272) | 1024 merged candidates per rack per block, 257 entries reserved for releases; 1024 delivery envelopes per member shared by ready and deferred work, also with 257 release entries. At most 16 envelopes per trigger (one on plus up to 15 peer chokes), 16,384 envelopes per rack. Separate fixed reset flags cannot be crowded out. Parent MIDI-effect storage retains ADR 0036's bounds. |
| Patch handoff | Two publication slots and two retirement slots, at most one pending publication, four bank-generation credits total. Adopt only if the old generation can be retired. Coalesce pending edits on the control side; a full retirement channel keeps the old patch playing. |
| In-memory sample | At most 30 seconds **and** 32 MiB decoded f32 per source; at most 128 MiB of distinct full PCM per patch. Reused slices do not duplicate PCM. Exceeding any bound uses disk in #271; before that, refuse visibly and retain the old patch/file. |
| Streamed attacks | First and last 4096 source frames of each zone region, including cubic guard taps, at most 256 × 2 × 4100 × 2 × 4 bytes (about 16 MiB); a short region shares its one cache. Prepared off-thread after edits. |
| Sampler stream pool | 64 reader leases per instrument, 4 windows per lease, 16,384 source frames per window plus 4 guard frames, stereo f32 (about 32 MiB); 256 fixed request entries. Allocate the pool off-thread only for an instance containing disk media. |
| Audio channels | New zones accept mono or stereo; reject higher channel counts with a conversion message. Legacy multichannel behavior stays unchanged. |

The memory limit for a Sampler source is the minimum of these bounds and ADR 0035's configured
`stream_threshold_frames` (10 minutes by default); reducing the setting can make a shorter
sample stream. Limits apply to decoded frames, with checked arithmetic, before reserving or
decoding. Admission accounts for resident attacks/rings separately. Existing large legacy
samples are grandfathered under the old loader limit until #271 can stream them equivalently;
do not refuse an old session merely because new-kit limits are smaller. Edits prepare a new
generation, so peak memory includes old and new generations. Four credits cover current,
up to two retained generations and one prepared candidate; a fifth preparation waits on the
control side. Share immutable PCM across generations. A failed reservation/import is an error,
not a partial patch. Before #271, aggregate-budget exhaustion refuses the edit; it never
evicts a sample that an older session or another track still needs.

Decode and source selection happen off-thread. Pad/zone structural edits stop affected voices
with the defined fade, then publish at a block boundary; unaffected voices may keep borrowing
the old generation until they finish. Retain that generation until all its voices are done,
and block further adoption when retirement credit is unavailable. No last `Arc` drop, buffer
free, file open, lock, allocation, or unbounded queue drain in process, event, parameter or reset.
Control changes are bounded numeric updates. Patch edits are not numeric automation events.
Expose fixed slot-based parameter ids (`zone001_level`, `zone001_pan`, zone-prefixed matrix ids),
never ids based on the current selection/name. A pad's controls address its stable bound zone;
there is no second alias with competing automation. Slot identities are not compacted on deletion;
explicit reuse clears or reassigns their automation in the same undo transaction. Prepare
parameter tracking outside the callback: thousands of zone controls do not fit mix's current
inline parameter cache. Existing unprefixed ids keep their legacy meanings and mappings.

### How a zone streams (required #271 extension to ADR 0035)

Today `DiskSource::stream(StreamMap, k, n)` has 8 slots/16 requests per source, monotonic clip
time, and PCM already mapped by `StreamMap::sample` with linear interpolation. It cannot supply
64 independent retriggers, reverse traversal, a cubic reader or changing pitch. Calling
`DiskSource::prime` on note-on would perform IO in the callback. Neither shortcut is allowed.

Extend the same process-wide `transit-disk-stream` service and the `DiskFile`/`BlockReader`
backend with a **raw-source window reader**, alongside the existing clip reader. Keep the clip
8/16/2-second contract unchanged. Sampler leases belong to voice slots, not source slots, so
64 voices of one source have 64 leases and cannot evict clip streams or one another. Requests
contain instance generation, voice generation, source identity, window origin and direction.
An edit/retrigger changes generation; stale replies never satisfy it. Use ADR 0035's published
head/busy protocol and atomic f32-bit storage, with fixed windows and no callback allocation.
This is new implementation work, not an assertion that `disk.rs` already exposes this API.

The core computes source phase at each output frame, including pitch changes, reverse,
ping-pong and both crossfade heads. It requests the raw pages containing the four cubic taps
of each head; four windows cover the two adjacent pages for each of two heads. Retain pages
in use while topping up/recueing; requests for anticipated pages follow direction and bounded
step, with current reads taking priority. If a requested page is absent, output zero for that
voice on that frame, advance its phase/envelopes normally and request the current generation.
Never replay a stale window or pause a voice until data arrives. Count one Disk underrun per
voice per process call that missed data, matching the existing clip-block diagnostic unit;
counts may depend on host blocks, while successful audio must not. File failures also post a
control-side message naming the media. No numeric throughput promise replaces a hardware check.

The control side preloads region attack/tail caches on load, plus primes known scheduled starts
before Play/Locate; a live note immediately reads its prepared attack while the same streamer
fills its lease. The finite cache is not a promise that an arbitrarily slow disk will catch up,
especially at high pitch. Loop/reverse/pitch jumps use the same miss rule. Start/end edits prepare
new caches before publication. Peaks reuse `DiskSource::peaks` and its validated sidecar.
Sampler references must use the new reader even when a timeline clip shares the file: never
read the empty `SourceAudio::buffer` of a disk source (the existing ADR 0035 Sampler limitation).

Offline bounce uses a **new fallible synchronous raw-reader seam** in #271, alongside
`DiskSource::with_sync`, on the offline thread. Today's `with_sync`/`Cache::get` logs open/read
errors and returns `None`, conflating failure with absent samples; it cannot supply this
contract unchanged. The new reader distinguishes out-of-region taps (zero) from open/read
errors and unexpected EOF inside the declared source. Latch the first error in the reader;
the offline driver checks that status after each render block and checks finish before
publishing the export. An error fails export with the media named and publishes no successful
output. Preserve the existing timeline-clip
API and behavior. Feed raw samples through **the same core region/interpolation/loop function**
as memory and live windows. No second resampling or `StreamMap::sample` interpolation is
inserted. A fully fed stream equals memory sample for sample; an underrun intentionally does not.

Native PCM WAV/AIFF can stream at their own rate through this reader; the Sampler's cubic step
does the rate conversion. This amends ADR 0035's session-rate-only restriction for Sampler reads,
not timeline clips. Long FLAC or other nonseekable input is decoded in bounded blocks to an
immutable native-rate float WAV/RF64 working file off-thread before admission, with cancellation
and disk-full errors leaving the old patch intact. This is conversion before use, not compressed
streaming in the callback. The original path and working-file reference are both retained;
short decoded audio uses the same f32 values. Add bounded decode/transcode support where the
current whole-file byte importer would exceed the limits. K91's other-rate/compressed **take**
placement is not declared fixed by this Sampler work.
Native-rate sample admission is a separate mode of the shared import preparation, not a call
to `cmd/sampler.rs`'s current unconditional `io::at_rate`. Timeline import and old managed PCM
keep their rate policy. All slice/loop/cue positions refer to the admitted source's native frames.

### Session format, media and SFZ

Add optional `Insert::sampler_patch` with schema version 1, 256 fixed logical zone ids, pad
bindings, source references, choke groups, layer/RR groups and seed, the loop's separate Auto
and Manual slice ids/markers/labels, selected slicing mode, snap toggle, detector settings,
grid division/bar override, root and beat length. Both sets persist with the same source and
loop region, including the hidden set, through Save, Save As and autosave. Physically serialize
only populated entries and nondefault controls. Every field has `serde(default)` (optionals default None, arrays/maps
empty, seed 1, zone root 60, no loop root (it reads the pad start note; replaced by Amendment 2026-11-09), controls as above); unknown fields are accepted and retained by the new
patch codec. Unknown schema versions retain their raw object and media references, report
unavailable and play silence; never rewrite them into an empty patch. Missing/invalid source
references stay in the patch, report silence per zone and can be relinked. Validate all counts,
ids, lengths, finite values and ranges before allocating render data. Reject an invalid new
edit; on load preserve an invalid patch opaquely with a load note instead of discarding it.

Default the new mode to Auto, snap to on, division to 1/16 and bar override to absent (From
tempo). Missing sets default to empty serialized storage, resolved to S plus implicit E when
a loop exists; no loop means no markers. For a patch from a build with a single marker set,
load that set as Manual, select Manual and preserve its ids/labels/bindings exactly; initialize
Auto to S only and do not redetect. This migration preserves edits without guessing whether
the old set came from a detector. Explicitly saved two-set state wins over legacy single-set
fields. New fields use `serde(default)` and never `deny_unknown_fields`, per the
[session-format rules](../contributing/knowledge/session-format.md). Register and emit
`sampler_slicing_modes` in `Session::needs`/`KNOWN_NEEDS` when this loop state is used, alongside
`sampler_patch`, so an older marker-aware reader that lacks the two-set extension warns.
Such a reader may lose the hidden set/mode on resave; pre-marker builds cannot warn. Neither
this migration nor these defaults create a patch for a legacy `Insert::sample` instrument.

No patch field means legacy behavior from `Insert::sample` and the existing 19 parameter ids.
Do not write a new patch merely by opening an old session. New patches leave `Insert::sample`
unset so a previous build does not play an arbitrary first sample as the entire kit. Add
`sampler_patch` to `Session::needs`/`KNOWN_NEEDS` when used (ADR 0023); add `rack_choke` when
expansion writes that feature. `format_version` remains 1. Previous builds ignore the new
fields and may play silence/lose them on save; marker-aware builds warn, pre-marker builds
cannot. This is forward readability, not a promise of backward playback or lossless old-build
resave. Document that warning and test the previous reader's actual behavior.

Extend source persistence with a defaulted ownership field: absent means the existing managed
source behavior; explicitly referenced media keeps its path and is not rewritten by ordinary
Save. Resolve session-relative paths against the session directory and kit-relative paths
against the mapping file; use checked roots, never the process working directory. Copy into
session collects originals and required working PCM under `Audio Files/`, uses collision-safe
immutable names and only swaps references after all writes succeed. Byte imports and captured
clips have no durable external path: materialize managed WAV first. Keep old media for undo,
backups and recovery; clearing a pad never deletes a file. Missing files keep their references
on Save, Save As and autosave. Autosave includes patch/maps/markers and every managed dependency;
external paths remain references and are reported if unavailable on recovery. It saves no
running voices, stream cursors, controller state or RR progress. A temporary decode cache alone
is never the only persisted copy of a user's sample. A capture-to-Sampler action does not
consume or remove the recorded clip/take.

Reuse audio-io's existing WAV/AIFF/FLAC decoding and the asynchronous import transaction pattern
in `cmd/file.rs`; these decoders already exist. Kit folder import lists supported audio in
case-folded filename order with original spelling as tie-breaker, assigns free pads ascending,
and previews/refuses over-capacity. An SFZ in the folder is an explicit mapping choice, not
auto-executed metadata. #271 supports `<global>`, `<group>`, `<region>` inheritance for `sample`,
`key`, `lokey`, `hikey`, `lovel`, `hivel`, `pitch_keycenter`, `tune`, `transpose`, `volume`, `pan`,
`offset`, `end`, `loop_mode` (no_loop, one_shot, loop_continuous, loop_sustain), `loop_start`,
`loop_end`, `seq_length`, `seq_position`. SFZ's inclusive end indices convert to half-open
boundaries with checked addition. `loop_sustain` exits looping on note-off while release plays
out; store this as an explicit per-zone loop-release flag, default continue-loop for manual
Gate zones. Reject contradictory sequence groups and maps over the stated layer/zone bounds.
Unknown opcodes, include directives, release triggers and unsupported loop modes appear in an
import report before applying supported material; no claim of full SFZ compatibility. Limit
mapping input to 1 MiB, 4096 tokens and 4096 bytes per path/label, and reject overflow atomically.
No recursive includes or arbitrary script execution. Missing samples retain named silent zones.

### Expand to rack (#272)

Expand only a pad-only patch with 1–16 occupied pads, across any banks. Refuse before changing
anything for more pads, unbound Instrument zones, frozen/recording tracks, an existing rack
member (nested macro-source propagation is not defined here), or a destination
that cannot retain the track's routing/automation. The refusal names the problem; no partial
expansion and no automatic bank nesting. This keeps ADR 0034's fixed 16-member distribution.

The original track id becomes `TrackKind::Folder` with its bus and `Track::rack`. Preserve all
playlists, takes, clip ids, track routing, sends, fader and inserts on it; remove its instrument.
Create ordinary Instrument member tracks, `Track::folder` pointing to that folder, outputs to
its bus, one Sampler per occupied pad. Each `RackMember` has a one-note key zone and full
velocity zone, both without fades; its Sampler retains the original pad id, trigger, region,
settings and random stream identity. Carry mute/solo as pad controls inside the member
Samplers, not track solo semantics that would alter the whole arrangement. Cross-member solo
mask belongs to the rack and uses original pad ids. Expose these through the member UI.

Remap every pad parameter lane, MIDI mapping and rack macro target to its member's
parameter address; global instrument controls map to every affected member or a preserved
post-sum control as appropriate. Apply the old Sampler's output level once after the sum,
before the original insert chain, using a normal Gain insert; refuse if that requires a ninth
insert or any mapping cannot be expressed without loss. Preserve operation order, with members
summed in pad order. New-patch Sampler output level must use the same gain arithmetic and
sample-by-sample smoother as that Gain insert from #269 onward; snap saved initial values
under ADR 0038 and transfer its automation without changing units. This is not a retrofit of
the legacy sampler's per-voice output gain. Members use unity output gain. Preserve the
unexpanded voice random stream using stored original zone/pad identity, not a new member id.
The stopped command's undo/redo is one transaction restoring the entire
track/rack, playlists, mappings and source references. Expansion is a topology edit: require
transport stopped and release live held notes; no claim of seamless voice continuation.

**Rack choke amendment required in this slice:** add default-Off choke group (1–16) and stable
pad identity to `RackMember`, plus a default-disabled Sampler-pad mode on `Rack`. Implement the
single rack event stage described by ADR 0036, extended to **every ingress to these members**,
on the thread that publishes strip jobs, before any member's new block is dispatched:

- Collect the parent's clips, chase and live input once; run the parent's MIDI effects once,
  then select destinations by the rack zones. Remove the existing per-member replay of the
  parent's clips and MIDI effects for this mode.
- Collect each member's own clips/chase, directly targeted live MIDI, and UI/controller pad
  presses and releases into the same stage. These bypass the **parent's effects and zone
  fan-out**, not choke/solo admission: a member-targeted on can trigger only that member's
  stored pad note. Other keys are unmatched and cannot choke. Route `MixEngine::live_note`
  audition through this stage with a UI source and matched press/release identity in this mode;
  its existing direct `Plugin::event` path is forbidden here. No pad hook calls a member
  instrument directly. Parent and direct inputs still obey their own arm/monitor rules.
- Preserve the original port/generation, channel, note id, transport epoch and destination
  slot generation. Add an origin discriminator (parent, or member id, including distinct clip
  and UI sources) so a parent's note-off cannot release a member's same-key note. The admission
  tables and rack-port bookkeeping live with the stage, even when a worker holds a member;
  never inspect or mutate that worker's instrument to decide admission. Keep the existing
  bounded note/pedal tables and refusal limits, with commit/rollback for prospective ons.
- Merge by absolute due frame. At a shared frame take member clip/chase streams in original
  pad order, then the parent's post-effect stream, then direct live/UI events in ingress
  sequence order. Preserve each stream's internal order, including clip off-before-on and
  MIDI-effect carry order; do not globally sort offs ahead of ons. Already committed deferred
  envelopes precede new work at an equal effective offset. Sample-stamped pad mute/solo
  changes apply before triggers at that frame. Eligibility uses the rack's saved solo mask
  and pad mute, consistently for every origin; an ineligible on starts nothing and chokes
  nothing. Releases of already admitted notes still pass. Last admitted same-offset trigger
  wins; its peer chokes precede any later retrigger in those peers' ordered streams.

**Atomic delivery and bounded refusal.** Prepare the capacities in the table above, including
scratch for the merge; parent effects keep their own ADR 0036 overflow policy. The merged
1024-entry budget includes parent output and all direct sources, not 1024 per source. At most
767 entries are non-releases. Overflow refuses/counts new ons and other non-releases before
they can affect a member, and retains releases in the reserve. A release that still cannot fit
sets fixed rack/member reset flags: cancel pending starts, clear the stage's held-note/pedal
state and reset every member at its next available block before any new on. Refuse starts
until that reset is acknowledged; count this safety recovery. No release is silently lost.

For each eligible trigger, reserve one delivery envelope for its destination and one for every
other member of its choke group, whether that peer currently sounds or is away. A choke
envelope is a private `choke_pad` control invoking the identical 32-frame post-shaper fade,
including one-shots; ordinary MIDI note-off is insufficient. Chokes use non-release credits
as part of the trigger, never the reserve needed for real releases. Check all destination
credits and note-table capacity before committing any envelope or source identity. If any
check fails, refuse/count the whole on with no choke or table mutation; do not auto-retry a
refused on later and create an unexpected hit. Paired releases for refused ons are unmatched.
Live ingress may report queued before this later admission decision; expose final refusals in
diagnostics and the admission/capture result, not as a successfully started voice.

Each member's 1024-envelope pool covers both ready and deferred deliveries; a worker holding
the member consumes no extra pool. Envelopes carry absolute frame, rack transaction order,
source identity and slot generation. Preserve FIFO order for that member; overdue envelopes
land at offset 0 before new work. Defer the entire envelope, including private choke controls,
never just its MIDI portion. Do not apply choke a second time on replay. Reserve downstream
Sampler event capacity under the same credits so a committed on/choke cannot be partially
refused after publication. These reservations and shadow note tables are new #272 work,
not a property of today's per-strip queue. Exhaustion backpressures subsequent whole triggers;
it never evicts a committed choke to admit an on. There is no cross-thread mutable choke state
or lock in an instrument. A missed worker block is silent under ADR 0029; this does not promise
audio equality across an overrun, only ordered, paired recovery without a missed choke.

Stop/seek/reset cancels undelivered starts and chokes, clears the stage's note/pedal state,
and ends member one-shots too, using fixed pending flags for away members. Port revocation
releases that origin's held notes and cancels its pending starts without releasing another
origin. Membership/slot-generation changes invalidate pending deliveries and reset the departing
member before reuse; require stopped transport for these topology edits in Sampler-pad mode.
Record parent and member live events to their respective tracks, preserving ADR 0039's takes;
the choke envelopes are runtime controls, not extra recorded MIDI notes.

ADR 0038 currently sends no note view to a rack's own inserts. Expansion therefore refuses a
track with any original note-reading insert, with a message explaining that it cannot retain
that insert's note input yet; it does not silently change its envelope. Members' new inserts
follow ADR 0038 normally. An expanded pad rack's member MIDI effect slots must be empty in this
first form: reject adding a member MIDI effect while rack pad/choke mode is active, and reject
an invalid loaded combination by marking that rack unavailable/silent, preserving its data
and reporting a load note. This enforces admission before any trigger transformation;
ordinary audio inserts remain available. Ordinary racks remain unchanged. Save these additive
fields with defaults; #272 updates ADR 0034/0036 alongside its implementation, not in this
documentation revision. Playback/recording and arrangement editing of the folder's existing
clips must work before #272 ships; ADR 0034 lists that as unfinished, so it is a prerequisite
owned by this slice, not assumed to exist.

### Control surfaces, help and presets

Expose a bounded pad snapshot and trigger/release hook using stable pad identity, bank, name,
note, occupied/mute/solo/choke and activity. A press/release uses the same event path as MIDI,
with the press-time mapping retained across bank edits. UI pads and future #240/#128 consume
this hook; no hardware driver or proprietary controller protocol belongs in sampler-core.

Each slice adds entries to `docs/help/sampler.toml`, registers ids and hovered controls per
`docs/help/README.md`, and updates the Sampler user guide. Cover views, bank/controller mapping,
voice/retrigger/choke, ranges/defaults, loop period under crossfade, detector limitations,
Auto/Manual ownership and copy-on-touch, Clear, zoom/scroll/readout, snap window and spacing,
pitch-versus-duration, both clip timing modes, takes placement, media collection/missing files,
stream underruns, import subset, expansion refusals and older-build warnings. Reuse existing
shaper/matrix help where semantics match. Keep brand names out of device/parameter ids.
There are no bundled sample assets in #269–#272; do not ship silent sample-dependent presets
under `assets/presets/sampler/`. Tests generate their own audio. A future starter-kit change
must place plain media under `assets/`, give every asset an `ATTRIBUTION.md` row and license,
and prove every preset loads and sounds. No detector attribution is added for reusing transit's
own implementation. If a future detector is ported, record its permissive source/revision and
full license in notices/help/About as ADR 0042 requires before shipping it.

## Gate

These are required implementation gates, not tests run by this documentation revision. Tests
inject events, transport samples, file contents, stamps and disk-pump progress. No sleeps,
elapsed-time thresholds or scheduling assertions. Run four-or-more-strip tests with
`MixEngine::force_serial(true)` unless they explicitly exercise workers. Preserve any failure
output verbatim; investigate before a rerun or test change, per the contributor rules.

1. **#269 voice and compatibility tests.** At 48 kHz use a 128-frame constant-one mono source,
   level unity, velocity 127, attack/decay/declick zero and sustain 1. One Shot on at frame 0, off at 16,
   produces 128 nonzero frames then zero. Gate with R=8 off at 16 emits .875, .75, .625, .5,
   .375, .25, .125, 0 at frames 16–23 and stays zero. Retrigger a numbered ramp at frame 20:
   frame 20 is source[0], not source[20] or a sum; the old note id's off at 21 does not release
   it. Trigger a second pad of the group at 16: isolate the first pad's contribution and check
   factors 31/32 through 0 on frames 16–47, zero at 47 onward. Include reverse, 64 pads,
   simultaneous offsets, group Off, cross-bank choke, mute/solo, sustain, All Sound Off and
   Stop. Use the old release build's stored fixtures/golden hashes for legacy defaults,
   loops, off-rate PCM, automation and steals; never regenerate the goldens from the new core.
2. **#269 analysis and MIDI tests.** Synthetic stereo loop with known pulse onsets, a quiet
   ghost hit, an opposite-phase hit and flams both above and below the 30 ms detector gap.
   Verify each accepted onset against the constructed attack within the detector's refinement
   window, absence of extras, and deterministic marker arrays; close-flam merging is explicit.
   A hand-built WAV contains unsorted/duplicate/end/out-of-range cue points and labels, plus
   valid/invalid tempo chunks; assert exact retained frame indices and labels, source priority
   and no regeneration of manual edits. Beat grids use hand-computed boundaries at two rates.
   Include Division/Bars overrides in 4/4 and 3/4, From tempo restoration and over-capacity
   refusal without changing either stored set.
   **Manual bench:** inject source frames and gestures, not clock or pointer timing. For a
   48,000-frame/48 kHz mono loop, set samples 0..11,999 to -1 and 12,000..47,999 to +1.
   Starting from S=0, add at 6000 with snap off, then drag it to requested frame 12,100 with
   snap on: the result must be exactly 12,000 (within the 240-frame search radius), with the
   same slice id. Snap off gives 12,100; a request at 12,241 has no crossing in range and
   stays 12,241. Also test the exact window edge, equal-distance crossings choosing the earlier,
   zero samples, opposite-phase stereo, missing source refusal and neighbor-constrained moves.
   A one-frame gap is admitted; duplicate/add-at-end and crossing a neighbor cannot break order.
   Remove the moved marker: it merges into its predecessor, reports references and frees only
   its binding. Drag/click S never changes S. Copy Auto's known set into Manual, edit Manual,
   switch Auto then Manual and compare both full sets/ids/labels to independent expectations.
   Change/accept Auto's detector result while Manual exists: Manual is unchanged. Test Auto
   marker-touch and empty-bench add copy/switch before editing, including an existing Manual
   set, one undo/redo and canceled drag rollback. Clear retains S and end, with Auto unchanged.
   Admit exactly 64 Manual slices; the next add leaves data unchanged and emits the specified
   status line. Test both actions with deliberately different Auto/Manual sets, stale mapping
   refusal and the separate remap; neither action may use the hidden set. Zoom/scroll input
   maps to the same native frames and millisecond readout at 1× and 8× without changing markers.
   For a 48,000-frame/48 kHz loop with markers 0, 12,000, 36,000, end 48,000, fit to four
   quarter notes gives 0, 1, 3 and end 4 beats; keep duration at 120 BPM gives 0, .5, 1.5,
   end 2 beats, at 60 BPM gives 0, .25, .75, end 1. Verify keys root..root+2, note lengths,
   tick rounding, a tempo change within the range, root overflow and a moved-pad refusal.
   Overlap a shorter generated clip with an old longer take and unrelated earlier/later clips:
   verify ADR 0039 active playlist, old whole take, preserved outside material, selection
   back/join, save/load and one undo/redo. Frozen/error paths leave the document identical.
3. **#270 zones, loops and shared primitives.** Probe key/velocity endpoints, every fade step,
   overlaps and both sides of each layer boundary using distinct numbered sample values.
   Verify selection before velocity scaling, sequential 0..N-1 order and a separately computed
   seeded random index sequence after reset/open/export. Exercise 256 leaves, 8 layers × 8 RR,
   64 voices and deterministic steals; the 257th zone/ninth layer/ninth alternative is refused.
   Compare forward/reverse/ping-pong and crossfaded loops `to_bits()` against the independent
   scalar reference above, including tiny loops, X=0/maximum, fractional steps, multiple wraps,
   source-rate differences and release transitions. Check the four-point reader's PCM-rate
   conversion against the existing arithmetic; no second converter may slip into the disk path.
   Shaper-off and empty-matrix paths null; active matrices test every source/target, one amp
   application, independent per-zone state and the held-note switch. Shared primitives require
   **the full workspace test with full oracles**, including Macro Oscillator, Resonator and
   Modal Voice, after the pure move and again after the behavior seam changes.
4. **#271 media and streaming.** Known PCM signals decode from WAV/AIFF identically; FLAC's
   lossless integer round trip matches those PCM values. SFZ fixtures cover every supported
   field, inheritance, inclusive ends, RR and loop_sustain, missing files, unsupported opcodes,
   hostile counts/paths and atomic rejection. Feed a fake `DiskFile`, explicitly prime/pump
   between controlled render segments and compare **every output sample bit** with memory and
   synchronous offline reads. Cover 64 voices of one source plus timeline clips sharing it,
   high/fractional pitch, reverse, loops and both crossfade heads across page boundaries,
   retrigger generations, long FLAC working PCM, session reopen and changed source fingerprints.
   Withhold pump progress deliberately: only missing frames of affected voices are zero,
   phase still advances, counters change by the specified voice-call count, and resuming never
   emits stale data. Through the new fallible synchronous seam inject open failure, a read
   error after a successful page, and EOF before the declared source end: each aborts export
   with the media named and no successful output. Legitimate out-of-region cubic taps remain
   zero without failure; existing timeline `with_sync` callers keep their behavior. Test
   cancellation/disk full, no partial patch, peaks mismatch with injected stamps, threshold
   boundaries, aggregate RAM admission and recovery
   with missing external media. No test waits for the real streaming thread to win a race.
5. **#272 expansion.** Compare original pads and expanded members through the real rack/mix
   paths, no member inserts, using the identical event schedule and sum order: sample-for-sample
   equality, not an unspecified floor. Test 16 pads across banks, duplicate naming, envelopes,
   reverse, solo/mute, per-note random identity, global level and automation, parent
   MIDI effects evaluated once, choke at offsets inside blocks and deferred worker events.
   In particular, expand open/closed hats in group 1 and put an on in the open member's own
   clip at frame 0 and the closed member's own clip at 16. With Gate 1's constant source and
   controls, isolate the open contribution: frames 16–47 have gains 31/32 through 0 and it
   stays zero afterward. Repeat through directly targeted `queue_event`, the pad hook and
   `live_note` audition, then mix parent and member origins in both directions. Muted or
   excluded-by-solo triggers must cause neither sound nor choke through every path.
   At equal offsets combine member clips, parent post-effect events and direct UI/live ons:
   assert the specified order and final winner, with an independently constructed reference
   schedule. A parent Transpose/Velocity fixture changes only parent-origin notes, runs once,
   and selects zones after transformation. Same key/id/channel on a parent and member stays
   separately owned; an older origin's off cannot release a replacement voice.
   Saturate merged scratch, one peer's ready/deferred pool and downstream event credits in
   turn, for each ingress: no on or choke is committed on a failed reservation, no note-table
   entry leaks, and freeing capacity admits the next distinct on exactly once. Saturate
   releases and verify counted reset recovery clears one-shots/pedals before accepting new
   starts. Use an injected worker-held state (no sleeps) for the open member, closed member
   and both; assert complete envelopes survive, replay in transaction order at offset 0,
   apply choke once and cannot choke a later replacement out of order. Stop, seek, revocation
   and member removal while work is deferred must cancel or release the correct origins,
   reject stale generations and leave no stuck voice. The no-overrun cases compare every
   output bit; held-worker cases assert silent missed blocks and the specified recovery,
   not a scheduling deadline. Assert 17 pads, unbound zones, full insert chains and
   note-reading insert refusals make no changes. All playlists/takes and folder clip playback,
   capture (parent and directly targeted members), editing, save/load, undo and redo must survive.
   Member MIDI effect additions are refused and invalid loaded combinations are retained,
   silent and reported. Defaults leave ordinary racks unchanged; additive fields generate
   `rack_choke` compatibility warnings on previous builds.
6. **Every slice: realtime, storage and UI.** Probe allocations **and frees**, including first
   process, events, parameter changes, reset, 64 voices, all 16 matrix rows, max queues,
   patch adoption/retirement backpressure and stale stream replies. Preallocate outside the
   measured span; deliberately hold retirement to test the full path, and verify destruction
   on the control side. Supply block sizes 1, 7, 64, 127, 512 and irregular partitions with
   the same absolute schedules: compare audio bits and state fingerprints from fresh reset
   and save/load. Check the model corpus/serde defaults, new session round trips, unknown
   schema retention, autosave recovery, collected media, old-reader warnings and unchanged
   old sampler ids/mappings. Round-trip distinct Auto/Manual sets, mode, snap and grid settings
   with either mode showing through Save/Save As/autosave; load absent fields and a legacy
   single-set fixture without detection, retaining old ids/bindings. Verify the additional
   `sampler_slicing_modes` warning with a reader that knows `sampler_patch` but not this extension.
   UI tests cover all views without a sound/mapping edit, pad drag,
   controller press/bank/release, capacity errors and every help id. Negative mutations
   (wrong choke length, doubled amp, bad loop wrap, stale page generation, dropped take,
   bypassed snap or an edit leaking into the hidden marker set)
   must fail the corresponding tests; green tests that do not reach those paths are no gate.

Add both new cores to ADR 0037's enforced bare-metal job in their first slice:

```sh
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-instrument-core
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-sampler-core
```

These are library checks, not `--workspace`/`--all-targets` builds or a claim of MCU memory fit.
Default-std desktop tests and the headless graph retain the built-in adapter. Update the Tier A
package/features list when that gate lands. The implementation merge also runs CONTRIBUTING.md's
fmt, clippy (including production lib targets), locked full workspace tests, release rt tests,
layers, assets, timing and file-size, plus `cargo xtask help --write` then `cargo xtask help`.
Run `rt-targets` when rt/mix changes, and the existing Linux/desktop CI legs. Set
`TRANSIT_FULL_ORACLE=1` for shared-primitive and release workspace tests. Every implementation
push/merge requires ADR 0009's fresh-context refutation review; this author's document is not
that review. The docs-only revision runs no cargo build or test.

## Slices

This is the authoritative re-cut of the filed descriptions; issue text is not silently treated
as updated. #269–#272 all belong to the composition release. Each lands with its help, save/load,
capacity, no-allocation and compatibility gate, not a later cleanup slice.

| Issue | Deliverable and scope changes | Blocking gate |
|---|---|---|
| #269 Pads and Slices | Tier B sampler core and shared-primitive extraction, compatibility path, full bounded patch/save model, pad controls/choke, four banks, Auto cue metadata/existing detector/grid with division and bars, Manual add/move/remove/copy/Clear, two persisted marker sets and copy-on-touch, zoom/scroll/ms readout/snap/fixed start, Slice to MIDI/takes placement and controller hook. Build slice 1 to the [panel mockup](../design/sampler-panel-mockup.html), with ADR text authoritative and real screenshots replacing it after landing. Basic one-zone Instrument panel remains usable. Move the shared extraction from #270 here so the dependency direction is valid from the first slice; activation of the shaper/matrix UI stays #270. No new AIFF/FLAC decoder task: preserve what loads today. Oversized new media is refused until #271. | Gates 1, 2 (including deterministic Manual edits, known-crossing snap, both-set preservation and 65th-slice refusal), 6 (two-set persistence/defaults), both bare-metal checks and full workspace/oracles for extraction. |
| #270 Instrument | Zone editor, layers/RR, loop/crossfade rules, per-zone shaper/matrix behavior and panel. Legacy migration is already in #269; this slice extends its tests. SFZ stays #271, correcting the original Decision's claim that it arrives with this view. | Gates 3 and 6, both bare-metal checks, full workspace/oracles for shared changes. |
| #271 Long media and SFZ | ADR 0035 raw-source reader extension, fallible synchronous raw reads and explicit pools, attack caches, native-rate streaming, bounded conversion of long compressed files, peaks, shared-file fix, format integration and SFZ subset. AIFF/FLAC decode already exists; test/extend the bounded path. No realtime compressed stream or fix claim for K91 take placement. | Gates 4 and 6, exact disk/memory/offline equality and injected open/read/early-EOF export failures, bare-metal and full workspace integration. |
| #272 Expand to rack | Atomic conversion to at most 16 ordinary member tracks, exact choke and solo-mask support through one bounded stage for parent and every direct member ingress, parent-only MIDI effects evaluated once, atomic on/choke reservations and deferred delivery, folder clip playback/record/edit prerequisites, mappings/global gain, compatibility fields and explicit refusals. No discarded excess pads or choke. Implement the scoped ADR 0034/0036 amendments with the code. | Gates 5 and 6 through the real rack: member-clip hats, direct live/audition, mixed origins, worker/queue saturation and release recovery, saved sessions and full workspace. |
| #273 Later ideas | Per-slice time stretching, velocity-to-start, SF2, release triggers and layered retrigger remain unscheduled. General MIDI banks and proprietary formats remain non-goals. No first-release gate depends on these. | A separate decision and deterministic tests when each idea is scheduled. |

## Consequences

One panel covers the common composition tasks, with exact finite limits and explicit errors.
The Sampler is mandatory for the composition release; finishing only the pad UI is insufficient.
Streaming and expansion require infrastructure work in their slices, not just new panel code.
Sample-rate conversion uses the existing cubic pitch reader and can alias at high playback
rates; higher-quality band-limited repitch is a later DSP decision, not time stretching.

New patches are bounded and reversible; old patches retain their exact legacy sound. External
references keep libraries light, while Copy into session and managed capture media make portable
and recoverable sessions possible. Older readers can open the file but cannot play the new
patch; the compatibility warning must say so. No bundled kits means the release needs the
documented load-folder workflow to become playable.

Expansion provides ordinary member inserts without inventing multi-output Sampler routing.
It has visible refusals where the existing rack cannot preserve behavior, and cannot grow past
16 members. The extra raw-page reader and portable primitive extraction carry review and test
costs; neither weakens the callback contract, layer rules, file-size ratchet or recording safety.

## Alternatives considered

- **Separate instruments (Sampler, Drum Sampler, Slicer).** Three picker entries and three panels would duplicate voices, envelopes and the matrix, and moving a patch between them would lose the sound. Simpler and Quick Sampler keep modes in one device; so do we.
- **Per-pad outputs inside the instrument.** It turns the Sampler into a mini mixer (multi-output routing in every host path, bus assignment, a pad strip for inserts). Drum Rack and Drum Machine Designer solve it with a rack; Expand to rack does the same on demand and keeps the common case light.
- **A rack of one-sample Samplers as the only drum path.** It is the Ableton and Logic model, but a 16-member rack for a drum loop is heavy to build and to read; the Pads and Slices views are the light path in front of it.
- **Import REX (`.rx2`) files.** Proprietary format with its own SDK license; WAV with cue points is the interchange.
- **Stretch each slice to the session tempo from the start.** It sounds smoother at big tempo changes but needs the stretching engine (#199 slice 2); pitched playback first, stretching as a later option.
- **SoundFont as the multisample format.** SF2 is an old container built around a General MIDI payload; SFZ is open text, maps to zones directly and is what sample libraries ship. SF2 may be added later.

## Judging round 1: responses

| Finding | Response and revision | Required evidence |
|---|---|---|
| C1 — Expanded members' own triggers have no decided choke path | Accepted. Expand to rack now requires every ingress (parent, member clips/chase, direct live input, pad hooks and legacy audition) to enter one rack-owned choke/solo stage. Only parent-origin events traverse the parent's MIDI effects and zone distribution. Original source/note identities remain distinct; the merged tie order, numeric storage bounds, atomic reservations, refusal/reset policy and complete deferred envelopes are specified. Member MIDI effects are refused in this first form. #272 owns all of these changes. | Gate 5 adds the two-member-clip open/closed-hat counterexample with exact 32-frame gains, direct-live/pad/audition and mixed-parent/member variants, same-offset ownership, saturated queues and injected held workers. Gate 6 covers the added storage and recovery paths with allocation/free probes. |
| M1 — Name the required fallible synchronous-reader extension | Accepted. #271 must add a fallible synchronous raw-reader/error-status seam alongside the existing `with_sync`; the present API's logged `None` cannot prove export success. Open/read errors and early EOF fail the export without publishing output; valid boundary taps read zero. Existing timeline callers retain their behavior. | Gate 4 injects each failure through the new seam, tests legitimate boundary zeros, and keeps bit-exact memory/live/offline comparisons and existing timeline behavior checks. |

These are documentation changes and required implementation evidence, not claims that the
implementation gates have run. No product question remains open for the maintainer in this round.

## Amendment 2026-11-03: Auto and Manual slicing, and the panel mockup

The panel review settled two slicing modes within the Slices view. **Auto** owns a read-only
detected marker set: file cue points first, then the existing transient detector with sensitivity,
then grid division with Division and Bars. **Manual** owns a separate user marker set, so a
breakbeat can be chopped entirely by hand: click to add, drag to move, click a marker to remove.
**Start from Auto's markers** copies Auto's accepted set into Manual; **Clear** leaves the fixed
loop-start marker and the implicit loop end. Touching an Auto marker (or adding on its bench)
copies the set and switches to Manual before the edit, as one undoable action. A plain mode
switch preserves both sets. Returning to Auto shows its detected set again; detection never
overwrites Manual. Slices to pads and Slice to MIDI use the mode currently showing, with the
existing explicit mapping and ADR 0039 clip-placement rules.

The bench has 1×–8× horizontal zoom, scrolling and a millisecond position readout. Zero-crossing
snap is a toggle, on by default: search the actual source within ±5 ms for the nearest sign
change, ties earlier, falling back to the requested frame if none exists. The body specifies
channel/zero handling and legal-neighbor bounds. Minimum spacing is one native source frame;
the first marker stays at loop start and cannot be moved or removed. The 64-slice limit applies
independently to each stored set: at most 64 starts including loop start, plus loop end, never
extra playable pads. Refuse a 65th Manual slice without changing its set and show **Cannot add
slice: 64 slices per loop** in the status line. Existing pad, zone, voice and media bounds stand.

Both sets, their ids/labels and the selected mode persist with the loop, along with snap and
Auto settings. Additive fields have defaults; a previous single marker set loads as Manual
with its identities/bindings intact, without redetection. The session-format section specifies
absent-field behavior and the older-reader warning. Gate 2 requires deterministic Manual
add/move/remove/copy/Clear and mode-switch tests, a loop with a known zero crossing proving
snap, fixed-start/spacing/capacity refusals and shown-set command behavior. Gate 6 requires
both-set save/load/autosave and compatibility evidence. These are requirements for #269,
not implementation or test-pass claims in this amendment.

The maintainer-reviewed [Sampler panel mockup](../design/sampler-panel-mockup.html) is the
design note for slice 1 (#269): the fixed 4×4 Pads column and selected-pad strip, the Auto/Manual
Slices bench, and the Instrument key map with its velocity axis, zones over their keys,
round-robin and velocity-split labels. The ADR's text wins where the mockup differs. Real
screenshots replace the mockup after slice 1 lands; the existing #270/#271 scope still governs
the full Instrument editor and SFZ. The Implementation section and #269 row carry this pointer.
All earlier decisions and judging responses remain in force except where this amendment
explicitly replaces the single-set slicing workflow.

## Amendment 2026-11-08: Slices and Instrument semantics (#269, #270)

Amended 2026-11-08, accepted: amended with the maintainer, who also decided **Load kit…** on
the Pads view bar the same day. This amendment decides twelve semantics the record left open for
the Slices and Instrument views, plus the Pads view bar's two actions. It changes no earlier
rule except where it says it replaces one; the validator changes are listed together under
"Validator changes". The matrix, the core's selection order and ADR 0034 are unchanged. Two
rules recur and are stated once:

- **Checked when set, clamped when used.** A value is checked by the edit that sets it, and an
  over-limit value is refused there, never clamped. A later edit of something else (a marker
  move, a key-range drag, a loop change) is never refused because of it: the stored value is
  kept, playback uses the clamped value defined below, and the state queries report both, so
  the UI can warn. The patch validator, which runs on load and on every store, accepts a stored
  value that is only clamped at use (the list under "Validator changes"), so neither a later
  edit nor a file load produces an invalid (silent) patch because of one.
- **The engine never guesses UI state.** Where a default depends on the selection or the edit
  cursor, the UI computes it and passes it explicitly.

`mix.sampler_state {track?}` is a new read-only query reporting the patch as stored plus, per
zone, the used values named below (`loop_used`, `crossfade_used`, `fades_used`) and the
normalized seed. `mix.sampler_slice_state` reports the same used values for slice-bound zones.

**1. Per-slice loop.** A slice-bound zone (a zone with a slice binding, normally a pad's) may loop Off,
Forward or Ping-pong, always over its whole slice. It is stored in that zone's existing
`loop_mode`; no field is added. For a slice-bound zone the plan compiler derives the region
from the bound slice in its own set (its start marker to the next boundary, the last slice to
E) and, when looping, `loop_start` = region start and `loop_end` = region end. The stored
fields are written through, so the file stays self-describing and the validator checks real
values: every edit that binds a zone to a slice, sets a slice-bound zone's loop, or changes a
bound slice's boundaries (a marker add, move or remove, Replace markers, Clear, Start from Auto's
markers) writes the derived `start`, `end`, `loop_start` and `loop_end` into each affected zone
in the same undo step. That set is bounded: at most the patch's 256 zones, and in practice at
most its 64 slice-bound pads, so one marker edit rewrites a bounded set and its undo snapshot is
bounded like every other patch edit. When a loaded file's stored fields disagree with the bound
slice, the slice is the authority: the validator checks and the compiler plays the derived
values, the stored fields are rewritten on the next save, and opening alone writes nothing. Loop points inside a slice are deferred (item 12);
the Instrument view's zone waveform shows a slice-bound zone's derived region and loop with
handles that do not drag, marked with the same lock as a pad-bound zone's key-map edges.

A loop requires Gate, for every new-patch zone and not only slices. An edit that sets a loop
mode other than Off on a One-shot zone is refused unless the same edit sets Gate; an edit that
sets One-shot on a looping zone is refused unless the same edit sets the loop Off. The text is
`A one-shot cannot loop: choose Gate, or turn the loop off.` The Slices view's Loop and
Ping-pong presses send the loop mode and Gate in one edit, one undo step restoring both; its
One-shot press on a looping slice sends nothing and shows that refusal as an error banner
(design round 2). The asymmetry is deliberate: choosing a loop implies sustain, so it carries
Gate with it as one edit, while leaving a loop is a separate intent the user states by setting
the loop Off first, as the banner says; a One-shot press that silently turned the loop off would
lose the loop without warning. On
load, a One-shot zone whose stored loop mode is not Off is accepted and plays as a one-shot
without looping (the body: a one-shot disables looping); `loop_used` reports `off`.

The body requires a loop of at least two frames. Setting a loop on a one-frame slice is refused
(`Slice 4 is one frame long, too short to loop.`). A later marker edit that shortens a looping
slice to one frame stands; the slice plays unlooped and `loop_used` reports `off`. Ping-pong
with a nonzero crossfade remains refused (body); the Ping-pong press sends crossfade 0 in the
same edit, and its undo restores the crossfade. Every per-slice loop change is one undo step.

**2. Crossfade read-back.** The zone gains `crossfade`, the body's X: an integer count of
native source frames (stored like every source position, unsigned 64-bit), default 0, written
only when nonzero. Commands take frames. The UI edits milliseconds with one decimal: an entry
of m ms becomes `X = floor(m * r / 1000 + 1/2)` with r the zone source's native rate (ties
upward), and the field shows `X * 1000 / r` rounded to one decimal with X in its tooltip. This
replaces design round 2's "converted at the session rate": X counts source frames, and at
original pitch a source frame lasts 1/r seconds. With L = `loop_end - loop_start` of the zone's
effective loop (a slice's length for a slice loop), the edit that sets X refuses
`X > floor(L/2)` and names the maximum (`Crossfade is at most 4.7 ms (226 frames) for this
loop.`). When a later edit makes `2X > L`, X is kept and playback uses
`crossfade_used = min(X, floor(L/2))` in place of X in every equation of "Loop points and
crossfade reference"; the queries report `crossfade` and `crossfade_used` in frames, and the UI
shows the used value with the warning line `Slice 7 is 9.4 ms long now: 4.7 ms of crossfade is
used.` (the Instrument view says `Zone` and its label). `crossfade_used` is 0 whenever the
effective loop mode is not Forward.

**3. Edge-fade representation.** Each zone gains `fades {key_lo, key_hi, vel_lo, vel_hi}`:
four widths in steps of the zone's own units (keys or velocities), integers 0 to 127, default
0, written only when one is nonzero. A width n at the low edge fades the n values `lo` to
`lo+n-1`; the value k steps from the edge (k = 0 at `lo`) has the factor `(k + 1)/(n + 1)`, and
`lo+n` is at full level; the high edge mirrors downward from `hi`. These are the body's ADR 0034
factors, applied as the body says: to the velocity, after selection by the original key and
velocity, never to voice gain. A fade never moves a note between layers and never extends a
zone. With `f_key` the key factor (`(k + 1)/(n + 1)` inside a key fade, 1 elsewhere) and
`f_vel` likewise, the selected zone's voice plays velocity `max(1, round(v * f_key * f_vel))`,
computed exactly as a ratio of integers and rounded once, ties upward; velocity sensitivity
applies to that value.

The edit that sets the widths refuses `lo_w + hi_w > hi - lo` on either axis, and any nonzero
width on a pad-bound zone (one key, one layer). When a later range edit leaves the widths too
wide, they are kept and playback uses `fades_used`: the low width clamped to the span, then the
high width clamped to what remains (the high one shrinks first). Conversion to ADR 0034, without
changing it: a rack zone's absolute edges are `fade_low = low + lo_w` and `fade_high = high -
hi_w`, and the same `(k + 1)/(n + 1)` factors result on both sides. A pad-bound zone's
`fades_used` are all 0 whatever it stores (binding is a different edit). Expand to rack creates
member zones without fades (body), so it never needs this conversion for a pad.

**4. Per-group random state.** One patch seed. With s the normalized seed (item 11) and g the
group id (0 to 255), the random state of group g starts at
`s ^ (g + 1).wrapping_mul(0x9E3779B9)` in 32-bit unsigned arithmetic, and at 1 when that is 0
(zero is xorshift32's fixed point). Each draw is the body's xorshift32 step (13, 17, 5) on that
state; the alternative is `word % count`. A group draws once per admitted layer trigger; when
one note admits several layers of the same group, they draw in ascending layer order. Sequential
counters are kept per group and layer, starting at 0. A group without a stored entry is
Sequential. This derivation is part of the format: files play the same everywhere, and changing
it later requires a new schema version or a new `Session::needs` entry. It does not change the
matrix's seeded note random.

**5. Beat-grid BPM override.** The loop gains `grid_bpm`, optional, finite, 5 to 1000 BPM (the
tempo map's range), written only when set. The grid's tempo is, in priority: an explicit Bars
count (the derived BPM, body), else `grid_bpm`, else the file's valid `acid` tempo, else the
session tempo at the insertion point. The insertion point is the `at` tick that
`mix.sampler_slice_detect` takes, filled by the UI by item 9's rule. `at` is required whenever
Detect reads the session's tempo map: when the session tempo is the grid's source, and when Bars
is explicit (the derived BPM uses the insertion meter, body). Without it Detect refuses (`Detect
needs a placement (at) to read the session tempo.`). With `grid_bpm` or a valid `acid` tempo and
no explicit Bars, `at` is optional and unused.
Like Bars, `grid_bpm` never changes Slice to MIDI's beat length or timing modes. In the Grid
controls a BPM field follows Bars: empty, it shows the tempo in use and its origin (`acid 96.00`,
`session 120.00`); typing sets `grid_bpm`, clearing returns to none; while Bars is explicit it
shows the derived BPM read-only; it dims with the other Grid controls. A change travels with
Detect and is stored by Replace markers (item 7). The round-2 board lacks the field; the design
README gains it in the design landing.

**6. Pad assignment for Slices to pads.** For the showing set of n slices at root r (refused
when `r + n > 128`), slice i plays trigger note r+i, and pads are chosen as follows, pads
counted in bank order (A1 to A16, then B1, as numbered above):

1. Pads bound to slices of the showing set count as free for this run: the run replaces its own
   previous binding.
2. If the pad holding note r+i (stored or default) is empty or slice-bound (either set), slice i
   takes it. If it is a loose occupied pad, the run is refused naming it (`Pad B13 already plays
   E3`), as the body requires.
3. The remaining slices, ascending, take the lowest free pads not taken in step 2, and each such
   pad's trigger note becomes r+i. No pad held r+i, so notes stay unique.
4. Pads bound to the showing set and not taken are unbound (they empty; their notes stay) and
   reported. A taken pad that was bound to the hidden set is reported as replaced.

The pads available to the run are the free pads (step 1 included) plus the pads step 2 takes.
When fewer than n are available (occupied pads would exceed 64), the run is refused before any
change with a notice naming both counts (`Slices to pads needs 9 pads; 6 are free.`); nothing
is truncated. A taken empty pad gets the new-pad defaults; a taken occupied pad
keeps its zone identity and settings, as the body's drag does. `preview: true` returns the same
plan and changes nothing. The whole run is one undo step.

**7. Detection candidate lifecycle.** At most one candidate per track. Detect produces it and a
newer Detect cancels and replaces it. It is stale after any change to that track's patch,
including undo and redo; Replace markers on a stale candidate refuses and says why (`The patch
changed after this detection: detect again.`), and on a pending one says `Still analyzing.`. It
is discarded by `mix.sampler_slice_discard` (the UI's **Keep current**), by Replace markers, by
removing or replacing the track's instrument, and by closing or opening a session. It is never
saved, autosaved or put in the undo history; Detect and discard are not undo steps. The detector
settings a change in the Detect panel makes travel with Detect and are stored by Replace (one
undo step), so a settings change never makes its own candidate stale; Keep current returns the
panel to the stored settings. The bench and every action use the current Auto set until Replace.

**8. Touching S in Auto.** In Auto, a move or remove aimed at the fixed start marker copies
Auto's markers to Manual and switches to Manual (the body's rule) as one undo step; S itself
does not change, and the result reports `fixed: true` beside `copied_auto: true`. It is a
successful edit, not a refusal. In Manual the same gesture is refused and changes nothing (`The
loop start cannot move.`). An add in Auto that would be refused (too close, outside the loop,
the 65th slice, missing source data for snap) is refused first and changes nothing: no copy,
no switch. Any accepted gesture aimed at an Auto marker id applies to that marker's copy, the
Manual marker at the same frame, because the copy creates new Manual ids (body).

**9. Slice to MIDI placement.** `mix.slice_to_midi` requires `at`, a tick at or after 0, and
refuses without it (`Slice to MIDI needs a placement.`). The UI always passes it: the start of
the arrangement selection when it includes the target track (a time-range selection's start;
with only clips selected, the earliest start among the track's selected clips), else the edit
cursor, else bar 1. The result
reports the tick and position it placed at, and the UI's placement line names which of the
three it used. An overlap becomes a new take per ADR 0039, as the body specifies. This replaces
the body's engine-side "placement defaults to the selection start, else the edit cursor" with
the same rule computed by the UI.

**10. Round-robin counters reset.** Every sequential counter and every group's random state
restart from item 4's values on `reset()`, on prepare, on open and at the start of an export
(the body), and at each transport transition between playing and stopped, in either direction,
at the first frame of the first block that reports the new state, before that block's events. A
locate while playing, a cycle wrap and a patch edit do not restart them. The reason is
reproducibility: every pass started from a stopped transport plays the same alternatives, and a
pass started where an export starts plays what the export plays. A pass started mid-song differs
from a whole-song export at that point, the same history dependence the body keeps for ADR 0033's
random LFOs. No convention is cited; the rule rests on reproducibility.

**11. Seed range.** Every command that writes the seed (`mix.sampler_seed`,
`mix.sampler_new_patch`) accepts integers 0 to 4,294,967,295, stores 1 for 0 and reports the
stored value; anything else is refused (`seed must be an integer from 0 to 4294967295 (0 is
stored as 1)`). The
field offers 1 to 4,294,967,295 and writes 1 for 0 (design round 2). A file whose stored seed is
0 is not rewritten on open; it plays as 1 and the queries report 1.

**12. Deferred by design, named.** In 0.7.0: no zone table, the key map and the Zone and Group
panels being the editor (#387); fades drawn on the key map but not dragged, the Zone panel's four
fields editing them (#388); no 64-cell slice strip, the flag strip selecting, the selection line
naming the slice and the Pads view's drag moving a slice between pads (#389); no loop points
inside a slice (item 1, #390); and no open states for the Shaper and Modulation section bars,
which stay collapsed bars as the Pads view has them (#391). Each reopens on a maintainer request
or at the next Sampler design round; the zone table also reopens when SFZ import (#271) makes
large maps common.

**Pads view bar: Load kit… and Expand to rack….** **Expand to rack…** is shown disabled at the
unused-control alpha, with a tooltip saying it is not available yet, until #272 lands; it then
follows this record's expansion rules, and its preview is a later board. **Load kit…** ships
in 0.7.0 as its own slice (decided by the maintainer, 2026-11-08): `mix.sampler_load_kit
{track?, folder, preview?}` applies the body's kit-folder rule: the folder's WAV, AIFF and
FLAC files (not recursive) in case-folded filename order with the original spelling breaking ties, assigned to free pads ascending with new-pad
defaults, referenced by path (the save model's default); an SFZ in the folder is listed and not
executed (#271). It decodes off-thread through the import transaction, refuses through the
existing admission rules (capacity previewed and refused without truncating, the in-memory and
aggregate media bounds, an unreadable file refusing the whole load and naming it), is one undo
step, and on a legacy one-sample patch is disabled behind the Notice's **New Kit**. Per-zone
color on the key map (zones stay neutral `surface.raised` until then, as design round 2 notes) is
#392.

**Validator changes.** The body says to reject an invalid new edit and, on load, to preserve an
invalid patch opaquely (silent). For exactly the following cases that rule is replaced: the
validator accepts the stored value on load and on store, the edit that sets it refuses it, and
playback clamps it as stated, reported by the state queries:

- a One-shot zone with a loop mode other than Off: plays as a one-shot, `loop_used: off` (item 1);
- a slice-bound zone's loop of one frame (a slice shortened after its loop was set): plays
  unlooped, `loop_used: off` (item 1);
- a slice-bound zone whose stored region or loop fields disagree with its bound slice: the slice
  wins and the fields are rewritten on the next save (item 1);
- `crossfade` above half the effective loop: `crossfade_used = min(X, floor(L/2))` (item 2);
- Ping-pong with a nonzero stored crossfade: `crossfade_used` is 0 (items 1, 2);
- fade widths too wide for the range, or nonzero on a pad-bound zone: `fades_used` (item 3).

Every other body validation rule stands, including the loop-inside-region and two-frame checks
for zones without a slice binding.

**Design text that changes with this amendment** (carried by the design landing, not this
branch): the Grid controls gain the BPM field (item 5); the crossfade field converts at the
zone source's rate, not the session rate (item 2); "loop points inside a slice are the
Instrument view's zone waveform" becomes "loop points inside a slice come later" (items 1, 12);
and the zone waveform of a slice-bound zone draws its derived start, end and loop handles locked,
with the 10 px lock mark the key map uses for pad-bound edges (item 1).

**Evidence each item requires** (implementation gates, not claims made here). Gate 2 adds: a
marker move that moves a looping slice's loop in one undo step; the loop/One-shot refusals both
ways and their combined edits; a one-frame slice refusing a loop and a later one-frame shrink
reporting `loop_used: off`; each bind, loop edit and marker edit writing the derived region
and loop into every affected stored zone in the same undo step, and a file with disagreeing
stored fields loading audible with the slice's values, opening unmodified and saving the
rewritten fields; each case under "Validator changes" loading audible (not opaque) and its
setting edit refusing; the crossfade ms-to-frames rounding at 44.1 and 48 kHz sources; a
slice edit leaving `crossfade_used` below `crossfade` with output equal to the reference at the
used value; the grid priority chain (Bars, `grid_bpm`, `acid`, session) and `grid_bpm`
round-tripping; Slices to pads at roots 60, 62 and 36 over default and edited notes, a
re-application at a new root, a hidden-set replacement and the shortage refusal; each stale
cause refusing Replace; S in Auto and Manual and a refused Auto add leaving the document
identical; Slice to MIDI refusing without `at`; Detect refusing without `at` when the session
tempo is the source and when Bars is explicit, and succeeding without it from `grid_bpm` or an
`acid` tempo; `mix.sampler_state` and `mix.sampler_slice_state` reporting `loop_used`,
`crossfade_used`, `fades_used` and the normalized seed beside the stored values, for clamped
and unclamped cases; `crossfade`, `fades` and `grid_bpm` round-tripping, absent from the
serialized patch at their defaults and present otherwise. Gate 3 adds: the fade table at both edges with
selection by original velocity (control: scale before selection and a layer case fails); width
refusals and `fades_used` after a range edit; item 4's per-group start states and draw order
against an independent reference; counters restarting at stop and at play but not at a locate
or a cycle wrap; seed 0 stored as 1 by the commands and a loaded 0 left unrewritten. As in
Gate 6, these negative mutations must fail the corresponding tests, and green tests that do not
reach those paths are no gate: the write-through skipped on a marker move (a stale stored loop);
the crossfade clamp removed; the per-group start state taken from the patch seed without the
group term; the counter reset at play start removed; the `at` check removed from Slice to MIDI
or from Detect; step 2 of Slices to pads skipped (a duplicate trigger note); a "Validator
changes" case loaded opaque instead of audible; and fades scaled before selection.

## Amendment 2026-11-09: the pad start note

Amended 2026-11-09, accepted: the maintainer mapped a 16-pad controller to the Pads view and
found the fixed default pad notes (60 to 123, "Controller mapping" above) at odds with the drum
machine convention. He decided that the Sampler gets a setting for the pad start note, with the
pads following it, and that new kits start at the convention's note. Pad numbering is unchanged
(bottom left, across each row, then upward; "Pad count and layout"). This amendment replaces
the fixed base and nothing else in that row: bank changes do not rewrite stored notes, a press
captures its destination and note identity until release, ordinary MIDI notes address the
stored trigger notes directly, and the per-machine controller mapping the row leaves unbuilt is a
separate thing that may coexist with this setting.

**1. A start note per Sampler patch.** The patch stores one value, `pad_start_note`, a MIDI
note 0 to 64, so that the start note plus 63 stays within 127. A pad with no stored trigger
note has the effective note `pad_start_note + index`, index 0 to 63: bank A is start to
start+15, bank B starts 16 above A, C 32 and D 48.

**2. Defaults.** Every command that creates a patch stores start note 36 (C1 where 60 is C3),
the drum convention: 36 is where the General MIDI percussion map starts (the bass drum) and
where the reference DAWs put the first pad of their drum instruments (the bottom-left pad of
Ableton's Drum Rack view, the first pad of Logic's Drum Machine Designer), so drum MIDI files
and clips made in other DAWs land on bank A, and a controller whose template starts elsewhere
is set once with the start note instead of remapping the controller. Today that command is
`mix.sampler_new_patch` (`crates/engine/src/cmd/sampler_patch.rs`, the one place that builds a
fresh `PatchV1`); the implementation lists any other. A patch saved without the field reads
60, which is what such a patch meant, so sessions and kits saved before this amendment open
unchanged. That is the field's stored default, not a migration: there is no compatibility code
and no known-issue entry. The serialization rule holds: the field is written only when it
differs from 60, so a new patch's 36 is written and an old patch round-trips byte for byte.
The field has its own serde default of 60, apart from the patch struct's default: the
empty-patch constructor (`PatchV1::default()`, `SamplerPatch::new()`) yields 36, so every
present or later patch-creating path gets 36, and only reading a saved patch without the field
yields 60. Like the 2026-11-08 fields, the field lives inside the unreleased `sampler_patch`
format and adds no new `Session::needs` entry.

**3. Stored pad notes are absolute.** A pad's note is stored when the patch holds a note for it
(`note` present), whatever its value: setting a note equal to the pad's current default stores
it, and plain-entry pruning never drops a stored note (today `set_pad` drops entries that
`is_plain`, and `is_plain` treats a note equal to the default as plain; the implementation
changes that; old files round-trip unchanged because pruning runs only on edits). A stored note
keeps its value when the start note changes; only defaulted pads move. `mix.sampler_pad_set`
with `note: null` returns a pad to its default note, refused like any note edit when that
default is another pad's note (the existing text `note 36 is pad 17's trigger note`) or fails
check (c). The start note change is refused, with the
document bit-identical, when:

- (a) `note` is not an integer 0 to 64: `note must be an integer from 0 to 64` (this is also the
  only way a pad could leave 0 to 127);
- (b) a defaulted pad would take a stored pad's note: `Pad B1 plays C1: a start note of C1 would
  put pad A1 there.`;
- (c) for some velocity 1 to 127, a moved, bound pad's new note would start more than eight
  voices. The pad counts one, and each unbound zone counts one if its key range holds the note,
  its velocity range holds that velocity, and it has no layer membership or is its layer's
  alternative 0 (one per round-robin layer, as the core selects); this is the body's "at most 8
  layer voices start for one note", counting the pad as one layer. A pad note edit
  (`mix.sampler_pad_set`, including `note: null`) makes the same check and refuses with the same
  text: `Pad A5 would be a ninth layer on E3.`;
- (d) the root write-through of item 5 would leave 0 to 127: `Pad A3's root would leave 0 to 127.`

The pad and note names in these texts are examples of the fixed wording. Note names in these
texts and in item 6's status notice come from the shared note namer (ADR 0051) and follow its
Middle C preference (the examples name 60 as C3); the command takes and `mix.sampler_state`
reports MIDI numbers. Defaulted pads move
together, so they never collide with one another; choke and retrigger match by pad, not note,
and a start change creates no zones and no layers by itself, so choke, retrigger and the
256-zone limit cannot be broken by one. This rests on the "Checked when set, clamped when used"
rule of the 2026-11-08 amendment: a later edit of something else is never refused because of an
earlier start note. The patch validator on load checks the unique-note rule on effective notes
and refuses a `pad_start_note` above 64 (opaque, as the body says).

**4. No MIDI is rewritten.** Clips, patterns, takes and automation are untouched by a start
note change, the same principle as "Dragging slices" ("no change to existing MIDI"). Notes
written against the old pad notes no longer hit those pads, and the user transposes the clips.
The command's status notice reports the distance the start note moved in semitones, so the
user knows what to transpose by.

**5. Pitch never changes with the start note.** "New pads default to ... root equal to their
trigger note" ("Playback, controls and modulation") means, today, that the root is written
into the pad's zone when the pad is bound (`ZoneEntry::pad_defaults`, called from the bind
path), so it is stored, not derived. With key tracking on, moving a defaulted pad's note
without its root would repitch it. The rule the implementation follows: when the start note changes by d, the same undo step adds
d to the stored root of every zone bound to a defaulted pad, whatever its root value and key
tracking, so each moved pad keeps the distance between its note and its root (a change that
would put such a root outside 0 to 127 is refused, item 3d); zones bound to pads with stored
notes, and unbound zones, keep their roots. A key-tracked defaulted pad with a user-set root
would otherwise repitch, so no "root equals note" test is used. A new binding after the change
takes root equal to the effective note, as now. The write-through does not interact with the
2026-11-08 write-through, which derives region and loop fields from slices and never touches the
root. No schema field is added; one rule serves old and new patches, because every existing pad
zone stores its root explicitly. Undo restores the whole patch snapshot bit-identically.

**6. Command and state.** One engine command, `mix.sampler_pad_start {track, note}`, sets the
start note, beside `mix.sampler_pad_set`: one undo step, refusals as in item 3. Its status notice
reads, for example, `Pad start note C1 (down 24 semitones): transpose pad clips by -24 to keep
them on the same pads.` `mix.sampler_state` reports `pad_start_note`; per pad `note` (the
effective note) and `note_stored` (a boolean); and the loop's `root` and `root_stored`. The
core receives effective notes: the link in `crates/mix/src/sampler_link.rs` computes
`start + index` for defaulted pads and the core has no start note (its own `60 + index` only
fills an empty spec, where no pad sounds). `mix.sampler_pad_set` setting a pad's note is
unchanged apart from `note: null` (item 3).

**7. Everything downstream uses the effective note:** the Pads view labels, the
computer-keyboard audition, Slices to pads, Slice to MIDI (each pad's effective note at the
time of the command), Load kit (the loaded pads take the patch's start note, and a kit's
stored notes stay as stored), choke matching and retrigger matching. No rule changes there.
The slice mapping root's default is replaced: a loop without a stored root uses the patch's pad
start note as its root, so a default Slices to pads run puts slice 1 on pad A1 at any start note
and `r + n <= 128` always holds (start at most 64, n at most 64). This replaces "root 60" in the
body's Slices paragraph (the defaults sentence) and in the session format's defaults, and
reconciles item 6 of the 2026-11-08 amendment: its "root r" is the stored root when there is one,
else the start note, and every other step of that item is unchanged. A root the user selects is
stored and absolute, like a stored pad note, is written only when stored (an explicit 60 is
written too), and is cleared with `null` (back to following the start note). The selectable
range stays 0 to 128 minus the count. Old patches read start 60 and so root 60, unchanged: no
migration. The error example "Pad B3 already plays E3" is corrected to "Pad B13 already plays
E3" for the default start note 36 (E3 is note 64, index 28, bank B slot 13). Expand to rack
(#272) maps pads by effective note at expansion: each member's one-note key zone is the pad's
effective note, and the member Sampler stores that note explicitly on the pad, so the member's
own start note never moves it.

**8. UI.** A **Start note** control in the Pads view's bar shows the start note as a note name
from the shared note namer (36 shows as C1 when middle C is named C3, C2 when it is named C4)
and sets it by drag or by typing a name; the stored value is the MIDI number. Its placement and look come from the design board; until that lands the
control is not built, and the command is reachable from the control channel.

**Evidence for the implementation slice** (not built by this amendment): a new patch stores 36
and an absent field reads 60; an old patch round-trips byte-identically; a start change moves
only defaulted pads; refusals (a) to (d) leave the document identical and name pad and note; a
loaded `pad_start_note` of 65 loads opaque; no clip, take or automation lane is touched; the
status notice reports the semitone distance; a key-tracked defaulted pad with a user root a
fifth below its note plays the same pitch before and after a start change, and a stored root on
a stored-note pad stays; `note: null` returns a pad to `start + index`, and is refused when another pad owns that note; a
stored note equal to its current default survives a later mute edit
and a start change; a default slice run at starts 36 and 12 puts slice 1 on A1; an explicit loop
root stays put across a start change and `null` clears it; `mix.sampler_state` reports
`pad_start_note`, per-pad `note` and `note_stored`, and the loop's `root` and `root_stored`;
Slices to pads, Slice to MIDI and Load kit follow the start note. As in the 2026-11-08 gates,
these negative mutations must fail the corresponding tests: the root write-through skipped for
a root that is not equal to the old note; the loop root read as a literal 60; `is_plain`
pruning a stored note equal to its default.

## Amendment 2026-11-11: the one-sample instrument is retired (#455)

Amended 2026-11-11, accepted: settled in one judging round with edits; the maintainer decided
the product choice (no conversion: an old one-sample Sampler opens as an empty kit). The
maintainer decided (#455), and these points are fixed: the old one-sample
Sampler panel and the legacy playback path this record kept for it are retired and deleted, and
nothing shows that panel or its New Kit notice again; an old session's one-sample Sampler opens
as an empty kit, its sample assignment and its 19 parameters dropped, with no conversion, no
migration shim and a load note; this is stated plainly as an alpha break in the release notes;
and a new Sampler is an empty kit on the Pads view (#454). This record's general rules stay
("checked when set, clamped when used", "the engine never guesses UI state", sources keep their
native rate). Where these leave a gap, the items below choose the narrowest rule, marked
**proposed**; the two gaps the maintainer settled are under "Decided by the maintainer".

**What exists today.** A Sampler is one-sample when its instrument slot names `sampler` and
`Insert::sampler_patch` is absent (`crates/model/src/mixer.rs`, `Insert`; the UI's test is
`has_patch` in `crates/ui-studio/src/sampler_panel/mod.rs`). Its state is the 19 parameters in
`Insert::params` (`SAMPLER_PARAMS` in `crates/dsp/src/plugins/sampler/legacy.rs`) and the session
source in `Insert::sample`. The plan compiler hands that source to the instance through
`Plugin::set_sample` (`crates/mix/src/compile.rs`, with the swap on a running instance in
`crates/mix/src/active.rs` and `crates/mix/src/plan.rs`), and the legacy renderer plays it
(`legacy.rs`: 32 voices, the exponential ADSR, the `hermite` reader). The window draws the
legacy body (`crates/ui-studio/src/sampler_panel/legacy.rs`: the sample line, Load Sample and
Clear from `builtin_editor::sample_picker`, the parameter rows, and the notice "This one-sample
patch stays unchanged until New Kit." with its New Kit button, `behavior::new_kit`, which runs
`mix.sampler_new_patch`). Three paths make or feed one-sample Samplers: `mix.sampler_load` and
`mix.sampler_clear` (`crates/engine/src/cmd/sampler.rs`, `attach`), `io::load_sampler_bytes`
(dropped bytes), and the browser's `Target::SamplerWindow` drop (`crates/ui-studio/src/browser.rs`,
`editor_target`). Until #454 lands, a Sampler picked from the browser is one-sample too. Sessions
are read by `Session::from_json_as_saved` (`crates/model/src/session.rs`), which already replaces
retired devices there (`legacy_synth::retire_subtractive_synth`,
`legacy_effects::retire_inherited_effects`) and reports through `Session::load_notes`.

**Clauses this amendment replaces.** Each is quoted with its line in this record as amended
through 2026-11-09:

| Line | Clause | Now |
|---|---|---|
| 48–51 | "**Old work keeps its sound.** Keep the existing `sampler` registry entry, parameter ids, automation addresses and `Insert::sample`. An old one-sample instrument is presented as one Instrument zone but retains its legacy renderer until explicitly converted by a structural patch edit. Opening, saving or switching views never performs that conversion." | The registry entry `sampler` stays. The 19 parameter ids, their automation addresses, `Insert::sample` and the renderer retire; opening drops the old instrument's state and leaves an empty kit (items 1 and 2). |
| 65 | "Already converted legacy sources keep their stored PCM." | Moot for the Sampler: the old source is no longer a Sampler source (item 3). |
| 92–94 | "The legacy ADSR, exponential release threshold, interpolation boundaries and 32-voice selection remain unchanged in legacy mode; a pure move must not replace them with these new envelope rules." | Retired with the renderer. |
| 286 | "Legacy loop arithmetic remains in the legacy path." | Retired with the renderer. |
| 316 | "... rack macros, legacy compatibility and prepared sample-bank handoff." | No legacy compatibility in the adapter. |
| 327–328 | "Preserve `Plugin::set_sample` for legacy instances; add a prepared multi-source bank seam rather than squeezing zones through its single-source API." | `Plugin::set_sample` is deleted (item 7). |
| 338 | "new Instrument patches use up to 64, legacy remains 32." | Every Sampler has 64 voices. |
| 347 | "Legacy multichannel behavior stays unchanged." | No legacy behavior remains; zones take mono or stereo. |
| 352–354 | "Existing large legacy samples are grandfathered under the old loader limit until #271 can stream them equivalently; do not refuse an old session merely because new-kit limits are smaller." | Retired: no old sample plays in a Sampler, so nothing is grandfathered; an old session still opens. |
| 372 | "Existing unprefixed ids keep their legacy meanings and mappings." | The unprefixed ids retire; what addressed them is dropped (item 4). |
| 459–460 | "Neither this migration nor these defaults create a patch for a legacy `Insert::sample` instrument." | Opening gives such an instrument an empty kit patch (item 2). |
| 462–463 | "No patch field means legacy behavior from `Insert::sample` and the existing 19 parameter ids. Do not write a new patch merely by opening an old session." | No patch field on load means an old instrument, which opens as an empty kit, and the next save writes that kit (item 2). The rest of that paragraph stands. |
| 653–654 | "Use the old release build's stored fixtures/golden hashes for legacy defaults, loops, off-rate PCM, automation and steals; never regenerate the goldens from the new core." | Those goldens are removed, not regenerated (item 8). |
| 758–759 | "... old-reader warnings and unchanged old sampler ids/mappings." | Item 8's drop evidence replaces "unchanged old sampler ids/mappings". |
| 794 | "... compatibility path ..." and "Basic one-zone Instrument panel remains usable." | No compatibility path and no one-zone panel. |
| 795 | "Legacy migration is already in #269; this slice extends its tests." | There is no migration; this amendment's slices drop the old state (item 9). |
| 808 | "old patches retain their exact legacy sound." | Old one-sample instruments do not sound; they open as empty kits (item 2). |
| 1087–1088 | "... and on a legacy one-sample patch is disabled behind the Notice's **New Kit**." | Load kit… has no legacy state to wait for. |

**1. The one-sample instrument is retired.** After this amendment's slices every Sampler has a
patch and plays through `transit-sampler-core`. The legacy renderer, the legacy panel, its notice
and New Kit, and every command that made or fed a one-sample Sampler are deleted (item 7).
`SAMPLER_INFO` keeps its id, name and category, and its parameter table becomes empty: the 19
ids were the legacy renderer's, and the zone slot ids stay outside it (body, "Capacities and
admission"). `mix.sampler_new_patch` stays, since #454's new Sampler uses it. Every command that
puts `sampler` into an instrument slot gives it `SamplerPatch::new()` in the same step: today
those are `mix.instrument` (the browser and the mixer's instrument picker) and `track.new` with
`instrument: "sampler"`, beside `mix.sampler_new_patch`. #454 covers both; if it covers only
`mix.instrument`, slice 1 adds `track.new`. A test runs each with `sampler` and finds a ready
empty patch. With that and item 2, nothing makes a Sampler without a patch once slice 2 removes
`mix.sampler_load`. **Proposed:** an instance that still meets one plays silence through the
core with nothing published, and the window draws it as it draws an opaque patch (no legacy
body, Load kit… unavailable); there is no third path. A `sampler` id that `mix.insert` puts in an
insert slot is outside this amendment: only the instrument slot's `Insert::sample` was ever
handed to an instance.

**2. Old instruments open as empty kits.** The drop happens on parse, in the model. A new
`legacy_sampler::retire_one_sample_sampler` (proposed file `crates/model/src/legacy_sampler.rs`,
beside `legacy_synth.rs`) is called in `Session::from_json_as_saved` after the two existing
retirements and before `legacy_mapping::migrate_session`. Its test (`uses_one_sample_sampler`)
joins the `changes` expression, so the caller gets the session as stored and the opened session
reads as modified, as with the retired synth. It runs once per load, on the control thread,
before the session reaches the engine (`Engine::replace_session`) or the plan compiler.
`replace_session` clears undo and redo, so the drop is not an undo step and no undo brings the
old state back. A second pass finds nothing.

- **Detection.** A slot is one-sample exactly when its `plugin` is `sampler` and its
  `sampler_patch` is `None`, with or without `Insert::sample`. A patch that is present in any
  state is never touched: a ready patch, a patch of a schema this version does not know, and a
  patch kept opaque because it did not parse or validate (`PatchState::Unknown` and
  `PatchState::Invalid` in `crates/model/src/sampler_patch.rs`, "kept as saved") are all `Some`,
  because the field's deserializer turns any present value into a `SamplerPatch`. Only an absent
  field (or an explicit `null`) is `None`.
- **What the slot becomes.** `sampler_patch` is set to `SamplerPatch::new()`, that is
  `PatchV1::default()`: schema 1, seed 1 (not written, its default), no sources, zones, pads,
  groups, loop or controls, and pad start note 36. This is a new kit made at load, so the
  2026-11-09 amendment's new-kit default applies (`PatchV1::default()` and `SamplerPatch::new()`
  yield 36 in the code; only reading a saved patch without the field yields 60). Because 36 is
  not 60, the next save writes `pad_start_note: 36`, exactly as #454's new kit does. The 19 ids
  are removed from `Insert::params`, `Insert::sample` is cleared, and the instrument's user pages
  are removed (item 4). `bypass` and `active` are kept. The `preset` label is reset to the one
  `Insert::new` gives (`<factory default>`), as `retire_subtractive_synth` does: a kept name
  would title the empty kit after a preset of the 19 retired values, which no longer applies
  and which the Sampler no longer lists (item 6).
- **What is written.** The next save (and autosave) writes the empty patch, no `sample` and none
  of the 19 ids; `Session::needs` then lists `sampler_patch` (item 6).
- **Kits that already hold the 19 ids.** A Sampler with a patch is never touched, its
  `Insert::params` included. Today `mix.instrument` writes the 19 defaults and
  `mix.sampler_new_patch` keeps them, so kits saved by current builds carry them, and lanes,
  mappings or macro targets made on them before New Kit may remain. They already drive nothing
  audible (the core ignores the legacy ids). Once the table is empty they are unknown parameters
  under the existing rule (`unknown_param_lines` in `crates/engine/src/io/session.rs`): kept in
  the document, one INFO log line per id at open, no load note, and the session opens clean. The
  plan compiler's `set_param` refuses them without logging.

**3. The old sample's file and pool entry.** Nothing deletes or rewrites it. The session source
that `Insert::sample` named stays in `Session::sources` (it shows in the Clip List), and the open
path still decodes it into the pool, because the open path decodes every entry of
`Session::sources` (`open_session_linked` in `crates/engine/src/io/session.rs`). No instrument
references it any more, so it becomes unreferenced unless a timeline clip also uses it. One
change follows: the drop runs before decoding, so it is no longer in `sampler_sources`, and it
is decoded at the session rate like timeline audio rather than at its stored rate. The file on
disk is unchanged. When the two rates differ, the pool's PCM is resampled, while the source's
stored `frames` and `sample_rate` still describe the file. Save keeps the file like every other
source (`write_session` keeps a source whose file is already in the session folder or its Audio
Files), so a saved session still carries the original file. A save to another folder (Save As)
writes each source's pool audio as a new 32-bit float WAV, as for timeline audio; for this
source that copy is now at the session rate, where it used to be at the stored rate. The user
removes the source with the Clip List's clear, as for any unused file, or loads it onto a pad
again from its file (a pad drop, Load…, or `mix.sampler_zone_source` with `path`), which imports
it at its native rate as a new source. **Proposed:** `mix.sampler_zone_source` with `source`
refuses a session source whose decoded rate differs from its stored `sample_rate` ("load it
from its file"): the zone's bounds would come from the session-rate PCM, and the next open
decodes a Sampler source at its stored rate.
`file.import_session_data` no longer brings the file along with a dropped instrument, since
the donor is parsed first and the instrument names no source (`session_data.rs`, `needed`). The
import posts the donor's item 5 line for each one-sample track it imports, under the name the
donor gave it, and the summary for those tracks. It already posts the donor's video note this
way. Without this the drop would be silent on import, because the import discards the donor's
other load notes.

**4. What addressed the old parameters.** The drop removes, on that track: automation lanes on
the instrument slot whose parameter is one of the 19 ids; session MIDI mappings to those ids on
that slot; rack macro targets (`MacroMapping` naming that track, the instrument slot and one of
the ids); and the instrument's user pages (ADR 0047), whose slots can hold only those ids. This
follows `retire_subtractive_synth`. Controller profile mappings live outside the session and are
not edited; one that names a retired id finds no parameter and does nothing. No modulation
matrix addresses the old ids.

**5. Load notes.** One line per track and one summary, through `Session::load_notes`, shown once
on open; a save leaves nothing to drop, so the next open has no note. Proposed texts:
`track "Keys": the old one-sample Sampler was removed and is now an empty kit; its sample
"Hit.wav" stays in the session's files and can be loaded onto a pad again; dropped 2 automation
lane(s), 1 MIDI mapping(s)` (the sample clause only when the instrument had a sample that the
session still lists, and the drop clause only when something was dropped), and the summary `N
track(s) had the old one-sample Sampler and now have an empty Sampler kit`.

**6. Other ways in, and older readers.** Every path that parses a session drops the same way:
open, open from bytes, crash recovery, session templates (saved sessions) and
`file.import_session_data` (import of tracks; the donor is parsed with `Session::from_json`).
Copying, duplicating or pasting a track inside a session meets only instruments that already
have a patch. **Proposed:** `track.load_preset` (`crates/engine/src/cmd/track_more.rs`), which
deserializes an `Insert` without parsing a session, applies the slot part of the drop (item 2:
the empty patch, no `sample`, none of the 19 ids, no user pages, the preset label reset) to a
one-sample instrument before it is applied, and posts the track line in its status line. It
leaves the target track's lanes and mappings as any preset load does. Built-in Sampler user
presets (`crates/ui-studio/src/presets.rs`) hold only the 19 retired values. Decided by the
maintainer: they are reported once like presets of retired devices (a
`retired_user_preset_lines` line for a preset whose `plugin` is `sampler`) and never deleted,
and the Sampler shows no preset list until a patch preset exists. No factory Sampler
preset ships (body). No new compatibility marker: the empty kit is an ordinary schema 1 patch,
so `Session::needs` emits `sampler_patch` (ADR 0023). A previous release, which lacks that
entry, warns that the session needs `sampler_patch`, has a Sampler with no sample (silent, as
the kit is), and loses the empty patch if it saves; a development build that knows
`sampler_patch` shows the empty kit.

**7. What is deleted.** Each item, with what still needs it: nothing.

- **The control-channel commands `mix.sampler_load` and `mix.sampler_clear` are removed.** They
  are documented (`docs/control-protocol.md`), so the removal is stated in the release notes and
  the protocol document. Today their callers are the legacy panel's sample picker, the
  Sampler-window drop, `builtin_editor::load_sample`, and two tests that are rewritten (item 8).
  The Slices view's file picker passes the string `"mix.sampler_load"` only as its purpose key;
  it never runs the command and takes a key of its own.
- `io::load_sampler_bytes` and `cmd::sampler::attach` and `refuse_patched`: their callers are
  each other and tests.
- `Target::SamplerWindow` in `browser.rs`: `editor_target` returns it only for a legacy body.
- `builtin_editor::sample_picker` and `load_sample`: their callers are the legacy panel and the
  window drop.
- The legacy renderer (`legacy.rs`; `Sampler::new` moves into `mod.rs`) and every legacy branch
  of `mod.rs`. The core path no longer needs to be optional.
- `Plugin::set_sample`, with its forwarder in `crates/plugin-host/src/hosted.rs`. Only the plan
  compiler calls it.
- The mix sample plumbing: the `sample_audio` and `sample_wrapper` slot fields, the sample
  action and its audio-thread swap (`plan.rs`, `active.rs`), and the sample parts of
  `compile.rs` and `compile/build.rs`. They exist only to call `set_sample`.
- The legacy panel (`sampler_panel/legacy.rs`, `LegacyLayout`, `view_bar::legacy_bar`, the legacy
  branch of `body`), its notice and New Kit (`behavior::new_kit`).
- Every other New Kit reference. The refusal "New Kit first." in `mix.sampler_load_kit`
  (`crates/engine/src/cmd/sampler_kit.rs`, `prepare`): a Sampler without a patch gets
  `patch_of`'s own error. The Pads view bar's disabled hover "Choose New Kit first" on Load kit…
  (`view_bar.rs`): an opaque patch still reaches it, and it now says the patch is kept as saved.
  Once the Instrument view is on main, its legacy body.
- The 19-parameter table. Its readers are the legacy renderer, the legacy rows, built-in Sampler
  user presets (item 6), and `fill_default_params` in `mix.sampler_new_patch`, which writes those
  19 defaults into every new patch's `Insert::params` today and writes nothing once the table
  is empty (the call is removed).
- The `Insert::sample` reads in `sampler_sources` and `session_data.rs`. In the model,
  `Insert::sample` stays only as the drop's input (it names the file in the load note). It is
  read on parse and never written.

Real time: the drop runs on the control thread during parse. Deleting `set_sample` removes the
audio-thread sample swap, so this change removes real-time work and adds none.

**8. Evidence** (implementation gates, not claims made here). Removed, not regenerated: the
legacy goldens (`crates/dsp/tests/sampler_legacy_golden.rs`, the old-session renderer hashes),
the legacy renderer's unit tests (`crates/dsp/src/plugins/sampler/tests.rs`), the legacy halves
of `a_legacy_instance_never_touches_the_core_and_a_core_instance_ignores_the_legacy_sample` and
`slot_ids_are_stable_and_unprefixed_ids_keep_their_legacy_meaning` (`core_tests.rs`; the slot id
half stays), the `set_sample` assertion in `crates/dsp/src/lib.rs`,
`a_sampler_sample_swap_allocates_and_frees_nothing` and the test wrappers' `set_sample`
forwarders (`crates/mix/tests/plan_alloc_probe.rs`, `crates/mix/src/plan_tests.rs`,
`crates/playback/src/plan_bridge/tests.rs`), the tests in `cmd/sampler.rs`, the one-sample
refusals in `tests_sampler_patch.rs`, `legacy_one_sample_patch_refused` in
`tests_sampler_kit.rs`, `sampler_panel/legacy_tests.rs`,
`the_legacy_window_loads_its_one_sample_without_an_import` in `drop_routing_tests.rs`,
`a_legacy_patch_shows_new_kit_on_the_instrument_view` in `instrument/states_tests.rs` (once the
Instrument view is on main), and the legacy cases of the builtin editor, Middle C display, view
bar (`legacy_views_keep_the_sample_until_new_kit_is_chosen`) and browser drop tests. Rewritten on the current
path: the read-root check in `crates/automation/tests/automation_roots.rs` (through
`mix.sampler_zone_source`) and the donor in `setup_more.rs`'s session data test. Added:

- an old one-sample session (a fixture with a sample and nondefault values for the 19 ids)
  opens with an empty kit (seed 1, pad start note 36, no zones), no `sample`, none of the 19
  ids, and the load note naming the track and the file;
- the source entry still in `Session::sources` and its audio in the pool, with the file
  byte-identical after open and after save;
- the lanes, session mappings, macro targets and user pages on the old ids of that track gone
  and counted in the note, and those of other tracks and slots untouched;
- `from_json_as_saved` returning the stored session (the open reads modified), the saved
  result listing `sampler_patch` under `needs`, and a second open of it with no note;
- a ready patch, an unknown-schema patch and an invalid (opaque) patch each left byte-identical
  and not reported as one-sample;
- a one-sample Sampler without a sample (the note without the sample clause), a track preset
  and `file.import_session_data` taking the same drop;
- `mix.instrument` and `track.new` with `sampler` each leaving a ready empty patch;
- the import posting the track line, and a track preset taking the slot part only (the target
  track's lanes kept);
- the preset label reset;
- a kit holding the 19 ids in `params` with a lane on one of them: it opens with no note, reads
  unmodified, and saves the ids and the lane unchanged;
- `mix.sampler_zone_source` with `source` refusing a source whose decoded rate differs from its
  stored rate, and accepting one whose rates match;
- no pinned hash anywhere in these tests.

These negative mutations must fail them: an opaque patch treated as one-sample; the lanes kept;
the pool entry removed; the drop missing from `changes` (the open reads clean); `track.new`
with `sampler` making no patch; the import's note missing; the preset label kept.

**9. Slices.** In this order, each building and passing on its own. #454 lands first.

| Slice | Content | Rough size (non-test lines) |
|---|---|---|
| 1 | The drop on parse, in `track.load_preset` and on import, with its notes; the patch on `track.new` if #454 did not add it (item 1); the `source` rate refusal (item 3); and tests. Every old instrument opens as a kit and no UI path makes a Sampler without a patch, so the legacy panel cannot be reached from the UI. Until slice 2 removes them, `mix.sampler_load` and `mix.sampler_clear` can still make or feed one through the control channel; a session saved that way is dropped again on its next open. Nothing converted needs the Instrument view, so slice 1 does not wait for it. | +150 |
| 2 | Delete the legacy renderer, `set_sample`, the mix plumbing, the panel, notice and New Kit, the commands, the dropped-bytes loader, the window drop, the sample picker and the parameter table, with item 8's test removals and rewrites. | +40, −800 |
| 3 | Docs: the user guide (including "A one-sample Sampler needs **New Kit** first"), `docs/control-protocol.md` (the two commands removed, and "require **New Kit** first"), `docs/mcp.md` (its `mix.sampler_load` lines), the design language README's Sampler rows and legacy notice, the session-format knowledge note, and the release-note text below. No help catalog entry names the legacy panel, so `cargo xtask help --check` stays green without `--write`; the slice runs it anyway. | docs only |

Release-note text (proposed): "Alpha break: the old one-sample Sampler is gone. Opening an older
session gives each one-sample Sampler an empty Sampler kit: its sample assignment and settings
are dropped, and so are the automation, MIDI mappings and rack macros that pointed at them. The
sample file stays in the session (it is in the Clip List) and can be dropped onto a pad again.
The load notes name every affected track. The control-channel commands `mix.sampler_load` and
`mix.sampler_clear` are removed: use `mix.sampler_zone_source` or `mix.sampler_load_kit`. Tracks
imported from an older session get the same empty kit, and the sample file does not come along.
Built-in Sampler user presets are retired: they are reported once and left on disk. A session
saved by this version and opened in v0.6.2 or earlier warns that it needs `sampler_patch`, has
a silent Sampler, and loses the empty kit if saved there."

**Alternatives rejected.** Keep the legacy path: two renderers and a panel the maintainer decided
has no place. Convert the old instrument into one zone (the draft first proposed for #455): the
maintainer chose the empty kit because the one-sample instrument had no users, and a conversion
would bring a mapping table and loop rules of its own. Drop the old source from the session too:
it would delete a user's file reference without being asked. Keep `Insert::sample` beside the
patch so a previous release still plays it: a shim, which the body already forbids ("New patches
leave `Insert::sample` unset").

**Reopen when** a release decides that sessions saved before this change no longer open; that
ends the drop code.

**Decided by the maintainer (2026-11-11).**

1. Built-in Sampler user presets are reported once as retired and never deleted; the Sampler
   shows no preset list until a patch preset exists (item 6).
2. The drop code stays until a release stops opening sessions saved before this change; whether
   it is removed is decided at 0.8.0, beside ADR 0049's file tags.

## Amendment 2026-11-11: kit presets (#467)

Amended 2026-11-11, accepted: settled in one judging round with edits; the maintainer decided
kit presets for v0.7.0. The maintainer decided (#467), and these points are fixed: for
v0.7.0 the Sampler's preset menu saves and recalls the whole kit (pads, zones, slices and both
marker sets, groups and round robin, every zone and pad setting, the pad start note), each sample
referenced by its file path, no audio copied; a recall replaces the instrument's kit in one undo
step, and the user is asked first when the current kit has loaded pads; a missing or moved sample
shows the views' existing missing-file state and the rest of the kit loads; a kit built by hand
is reusable across sessions. Convention: in the reference workstations a drum rack or sampler
instrument preset is the kit and points at its samples (Ableton's Drum Rack and Simpler or
Sampler presets, Logic's Drum Machine Designer and Sampler patches). This record's rules stay:
"checked when set, clamped when used"; "the engine never guesses UI state"; sources keep their
native rate; no proprietary sample-library format is parsed or vendored; no migration shims while
alpha. Where these leave a gap, the items below choose the narrowest rule, marked **proposed**,
and item 11 records how the open points were decided. "Later, not in this issue" (#467):
saving a kit together with copies of its samples.

**What exists today** (on the branch that retires the one-sample instrument, #455).

- *The patch* is `PatchV1` in `crates/model/src/sampler_patch.rs`: `schema`, `seed`, `sources`,
  `zones`, `pad_start_note`, `pads`, `groups`, `loop` (`SliceLoop` with both marker sets),
  `controls` (the nondefault `zoneNNN_*` slot values) and `extra` (unknown fields, kept). A source
  is a `SourceRef { source: SourceId, ownership, extra }`: a session source id, nothing else. The
  file path lives on the session's `Source` (`crates/model/src/source.rs`: `path` relative to the
  session folder for managed audio, absolute for referenced media; `frames`, `channels`,
  `sample_rate`, `format`, `unsaved`). `SamplerPatch::from_value` reads a patch: schema 1 parses
  and validates to `PatchState::Ready`; any other schema is `Unknown`, and anything that fails is
  `Invalid`, both kept as raw JSON and written back unchanged.
- *Built-in presets* are `Preset { plugin, version, params }` JSON files
  (`crates/ui-studio/src/presets.rs`), user files in `<config dir>/presets/<plugin-id>/<name>.json`,
  read and written by the UI directly (`save_user`, `load_user`, `delete_user`, `user_names`).
  The title bar (`crates/ui-studio/src/device_editor/title_bar.rs`) lists factory then user names
  (`preset_names`), steps with previous and next (`stepped`, wrapping; from no preset, next is the
  first and previous the last), and Save writes over a user preset or opens the list for Save As
  (`save_current`). Recall is `StudioApp::apply_preset`, which writes parameter values only. For
  the Sampler, #455 left `user_names` returning nothing for `sampler`, and
  `retired_user_preset_lines` reports every `.json` file in `presets/sampler` (by its `plugin`
  field, or else by the folder name) as a preset of the retired one-sample Sampler.
  `SAMPLER_INFO.params` is empty (`crates/dsp/src/plugins/sampler/mod.rs`), so a recall today
  changes nothing (`the_factory_default_on_a_kit_leaves_the_kit`).
- *Hosted plug-in presets* go through the engine: `plugin.preset_save` and `plugin.preset_load`
  (`crates/engine/src/cmd/plugin_presets.rs`) take an explicit `path`, and
  `crates/engine/src/io/presets.rs` lists, reads, writes (temporary file, then rename) and
  deletes through the checked file access.
- *Track presets* (`crates/engine/src/cmd/track_more.rs`): `save_preset` writes
  `{format: "transit-track-preset", version: 1, ..., instrument}` with the `Insert` serialized as
  is, so a Sampler's `sampler_patch` carries the saving session's source ids (#458); `load_preset`
  checks the tag (`file_tags`, ADR 0049), deserializes the `Insert`, applies #455's slot drop and
  assigns it with no source remap, no admission and no `store()` check.
- *Admission and the checked store* (`crates/engine/src/cmd/sampler_patch.rs`): `source_param`
  reads a `path` (resolved by `resolve_path`), checks `admit_source` (mono or stereo, 30 s,
  32 MiB, the streaming threshold) and `admit_patch` (128 MiB of distinct decoded audio; a source
  without pool audio counts nothing) from the header before decoding, and prepares native-rate
  PCM with `io::prepare_audio_bytes(.., ImportRate::Native, ..)`; `add_audio` adds it as a
  session source (`Referenced` keeps its path, `unsaved: false`). `store` runs
  `transit_mix::check_sampler_patch` (validation, region within present audio, the core's spec
  check; missing audio is allowed) and `PatchV1::layer_overload(&before)` (the eight voices per
  note rule, against the patch before the edit), then writes the patch.
- *Load kit* (`crates/engine/src/cmd/sampler_kit.rs`, `load` and `prepare`) stages the whole
  candidate on a worker thread over a clone of the session inside `file_access::confine`, and
  joins it before returning: the engine waits for the decode (#450). An admitted candidate
  replaces the session and goes through `store` as one undo step; a refusal answers data with a
  `notice` and changes nothing.
- *Stored paths* are resolved by `io::stored_path(stored, document)` (`crates/engine/src/io.rs`,
  the `craft_docpath` check: relative to the document, `/` and `\` both separators, Windows forms
  recognized on every platform, other computers, devices and URLs refused), then
  `file_access::check_read`.
- *The missing-file state* is a session source with no audio in the pool: the Instrument view's
  zone panel and the Slices bench hatch the well and show a card naming the file with Relink…
  disabled (`instrument/mod.rs` `source_of`, `instrument/zone_wave.rs`, `slices/bench.rs`
  `missing_card`); `sampler_silence_notes` (`crates/engine/src/io/session.rs`) writes one load
  note per track naming its silent zones. The Pads view has no missing state of its own.
- *A Detect candidate* lives on the engine (`crates/engine/src/slice_detect.rs`) and turns stale
  when the track's patch differs from the one it was detected from (`Candidates::observe`).

**1. One kit format: the portable patch.** A kit is the session's patch serialization with one
change: each entry of `sources` carries a `file` object in place of the session's `source` id.
`file` holds `path` (rules in item 2) and the facts the session's `Source` records about it:
`name`, `frames`, `channels`, `sample_rate` and `format` (the session's names). `ownership` is not
written (a recalled source is always referenced, item 4). Everything else (`schema`, `seed`,
`zones`, `pad_start_note`, `pads`, `groups`, `loop`, `controls`, unknown fields in every entry) is
written exactly as a session writes it, by the same serde code, so a field added to `PatchV1`
later reaches kit presets and track presets with no second format. A new model module
(**proposed** `crates/model/src/sampler_portable.rs`, pure, no file IO) converts both ways:

- `to_portable(patch, session, base)` turns each `SourceRef` into its `file` object from the
  session's `Source` and returns the JSON value. It works on a `Ready` patch and on an opaque one
  (it rewrites the raw `sources` array the way `SamplerPatch::remap_media` does), so a track preset
  of an opaque patch stays opaque and still carries no session id.
- `from_portable(value)` returns the value with the `file` objects taken out and the list of file
  references by source index. The engine resolves and admits each file (item 4), writes the new
  session id into `sources[i].source`, and then reads the value with `SamplerPatch::from_value`,
  so schema handling, migration of the single marker set, validation and kept unknown fields are
  the session's.

A source entry with neither `file` nor `source`, or with both, makes the kit invalid (refused on
recall, item 9). A `source` id alone is a pre-amendment track preset (item 7).

**2. Paths.** **Proposed** rules, one implementation for kit presets and track presets:

- *The sample's absolute path* at save: a referenced source's `path` as stored; a managed source's
  `path` joined to the session folder. The Windows extended-length prefix (`\\?\`, `\\?\UNC\`) is
  removed first, before the relative test below and before anything is written (#450 asks the
  same of stored source paths; Load kit stores `\\?\` paths today, from canonicalization).
- *Relative* when the sample is the preset file's folder or below it: compared by whole components
  after `std::path::absolute` (no link following, no file access), without regard to case on
  Windows. Written with `/` separators, never `..`, never a drive or root. So a kit saved into its
  sample folder (`Kits/808/808.tkit` beside `Kits/808/Kick.wav`) moves, copies and backs up as one
  folder.
- *Absolute* otherwise, in the platform's own form, as the session writes referenced media
  (`D:\Library\Kick\Kick 01.wav`, `/Users/me/Library/Kick 01.wav`). The session already holds
  every path as UTF-8 text (a file picked with a name that is not valid UTF-8 is stored lossily
  today, `source_param`'s `to_string_lossy`), so Save writes what the session holds; such a
  sample is missing on recall like any path that does not resolve. Fixing the lossy store is
  outside this amendment.
- *Reading.* Each `path` goes through `io::stored_path(path, preset_file)` and then
  `file_access::check_read`: a relative path is taken from the preset file's folder (both
  separators accepted; a hand-written `..` is folded lexically and accepted while it stays below
  the drive or root, since an absolute path may name the same files), an absolute one as written,
  and a path to another computer, a device or a URL is refused and treated as missing. A network
  share counts as another computer unless the preset file is on that same share
  (`craft_docpath`'s rule, the same as a session's): a kit kept in the config folder whose samples
  sit on `\\nas\samples` finds none of them; a kit saved on that share beside them does, and so do
  mapped drive letters, which the lexical check cannot see. The file is accepted when it is a
  regular file with links followed, as `io::read_file` reads a session's referenced media (not
  Load kit's no-links rule, which guards a folder listing). The read root bounds what is opened
  only under automation; in the app a shared kit can name any local file, which is opened only to
  be decoded as audio, never shown or sent anywhere. Nothing is guessed: no search by file name,
  no drive letter mapping, no fallback to the session folder.
- *Another machine or drive.* Relative paths follow the kit folder. Absolute paths resolve only
  where the same path exists; otherwise the zone is missing and the note names the path it looked
  for (item 5). A Windows path read on macOS or Linux (and the reverse) does not exist there and is
  missing. Case matters where the file system makes it matter: a kit made on Windows that names
  `kick.wav` for `Kick.wav` is missing on Linux.
- Each `path` is at most 4096 bytes (the patch's `MAX_LABEL_BYTES`).

**3. Save.** **Proposed** engine command `mix.sampler_kit_save {track?, path}`, not undoable and
not journaled (like `track.save_preset`). The caller passes the full path (the UI builds it from
the config folder, item 6, with the name cleaned as `presets::file_name` cleans preset names);
the engine appends `.tkit` unless the path already ends in `.tkit` (any case). It never replaces
another extension: `track.save_preset`'s `with_extension` would turn `808 v1.5` into `808 v1.tkit`
and let two kit names write the same file. It writes through `file_access::check_write`, to a
temporary file beside the target and then a rename, as `io::presets::write` does, so a failed
write never leaves half a kit.

- *The file:* `{"format": "transit-sampler-kit", "version": 1, "kit": <portable patch>}`, pretty
  printed JSON a user can read and diff. The tag and version follow ADR 0049 and are checked by
  `file_tags::Tags` before anything else is read. The name is the file name; the file has no name
  field.
- *What is captured:* the whole patch, nothing else. Enumerated outside the patch: the
  instrument's `Insert` has no Sampler parameters (`SAMPLER_INFO.params` is empty; any of the 19
  retired ids still in `Insert::params` drive nothing and are not saved), no user pages (the
  Sampler declares none), and `bypass`, `active` and the preset label belong to the slot, not the
  kit. There is no output level or macro on the Sampler today; the zone levels and pans are slot
  controls and are in `controls`. Track mixer settings belong to track presets.
- *Slices of a loop:* the loop's source is a source like any other; the loop region, both marker
  sets with ids and labels, the mode, detector and grid settings, root and beat length, and every
  slice-bound zone are in the patch and are saved with it.
- *A source missing at save time* (listed by the session, no audio in the pool): saved with its
  stored path and recorded facts, and the result lists it under `missing`, so a kit with a lost
  file can still be kept.
- *A source with no file of its own* (managed audio still `unsaved`: recorded, captured or dropped
  bytes before the session is saved; or a source id with no session record, such as the source 0
  an older build gives a portable entry): Save is refused before the temporary file is created,
  nothing written, with a reason that names the pad, else the zone, else the slice loop:
  `Pad A3's sample "Hit.wav" has no file yet: save the session, or load the
  sample onto the pad from a file.` (proposed text). Writing the audio beside the preset is the
  later feature #467 names, not this one.
- *A managed source with a file* (in the session's `Audio Files/`) is saved by its absolute path,
  and the result's `notes` say that the kit depends on that session folder. A sample on a network
  share other than the one the kit file is written to is saved, and `notes` say it will be missing
  on recall (item 2).
- *An opaque patch* (unknown schema or invalid) is not saved as a kit: "this version cannot save
  the track's Sampler kit (it is kept as saved)".
- *Writing over a file:* Save refuses to replace an existing file whose tag, version or kit
  schema this build does not read, naming it: "was saved by a newer transit: Save As under
  another name". Otherwise Save replaces it (the title bar's Save over a user kit).
- *Result:* `{track, path, pads, zones, sources, relative, absolute, missing: [{zone, pad?, path}],
  notes}`.

**4. Recall.** **Proposed** engine command `mix.sampler_kit_load {track?, path, replace?: false}`,
one undo step ("Load Sampler Kit"), not journaled (as `mix.sampler_load_kit` and every Sampler
command that reads a file: `cmd!(@spec false, true, false; ..)`). The track's instrument must be
a Sampler with a ready patch: a patch this build keeps opaque (unknown schema or invalid) is never
replaced, with `patch_of`'s refusal ("this version cannot edit the track's Sampler patch (it is
kept as saved)"), because it is a newer build's kit; a frozen track is refused as every edit is.

- *The user's choice is explicit.* When the current patch has any zone or a loop, the command
  without `replace: true` changes nothing and answers data `{loaded: 0, replace_needed: true,
  pads, zones}` (the loaded pads and the zones that would be replaced). The question itself is the
  UI's (item 6).
- *Staging.* As `mix.sampler_load_kit` does: the file is read and checked (size at most 8 MiB,
  proposed; tag and version; kit schema; `from_portable`) before any audio is touched, then a
  worker prepares the whole candidate over a clone of the session inside
  `file_access::confine`. For each referenced file: resolve (item 2); if it is a regular readable
  file, check `admit_source` and `admit_patch` for the running total (against the new kit, not
  the old one) from its header, or from its bytes before decoding where the header gives no
  length (as `source_param` does), decode once through the shared native-rate preparation
  (`ImportRate::Native`), check the decoded rate and frames against the kit's record (next
  bullet) and add it with `add_audio` as `Referenced`. A file the session already holds as a
  referenced Sampler source is reused rather than decoded again (proposed) only when the stored
  path equals the resolved one (prefix stripped, compared as item 2 compares) and its pool audio
  is present, at its stored rate, with the kit's recorded rate and frames; otherwise it is read
  as a new source.
- *A changed or unreadable file.* When a file is found but its decoded `sample_rate` or `frames`
  differ from what the kit recorded, the zones' native-frame regions no longer describe it; when
  it cannot be opened or decoded, it has no audio. Either way that source takes the missing state
  of item 5 (no pool audio; the kit's recorded facts; its zones and pads as saved) and the rest of
  the kit loads, as #467 asks for a sample that moved: the notice names the file and why
  (`Kick 01.wav changed since the kit was saved: 48000 Hz, 21000 frames; the kit expects
  44100 Hz, 19845 frames`, proposed text). Loading the new audio is not an option: `store`
  refuses a region past present audio, and rescaling or clamping the regions would rewrite the
  kit by guess. The user reloads the pad (Load…, drop), which keeps its settings, or restores the
  file and recalls again. Once the session is saved and reopened, the file is read under the
  session's own rules for referenced media.
- *One check, nothing partial.* The candidate patch goes through `store` with two differences
  from an edit: the eight voices per note rule is checked against an empty patch (the kit is a
  new map, so its own overloads count), and the new patch replaces the old one whole (`store`
  gains this as an explicit "replacing" argument; edits keep comparing with the patch before
  them). Any refusal (budget, layers, the core's spec check, a bad kit, an opaque or frozen
  target, a failed worker) answers data with a `notice`, as Load kit does, and leaves the
  document, the undo history and the session's sources and pool as they were: the candidate is a
  staged clone and only an admitted one replaces the session.
- *What changes.* The track's whole `sampler_patch` is the kit's: seed, pad start note, stored pad
  notes as stored (never rebased onto the old start note), pads, zones, groups, loop and controls.
  The slot's preset label becomes the kit's file stem. `bypass`, `active`, the track's clips,
  takes, automation and mappings are untouched: no MIDI is rewritten (the 2026-11-09 amendment's
  item 4), and lanes or mappings of zone controls (`zone003_level`) now drive the new kit's zone
  of that id; when the new start note differs from the old one, the notice says so as
  `mix.sampler_pad_start` does.
- *The old kit's sources.* A source that the replaced patch named as `referenced` leaves
  `Session::sources` and the pool in the same step when, in the candidate session after the new
  patch is in place, nothing else names it: no clip in any playlist of any track
  (`Session::clips_using`), no track's freeze (`Frozen::source`), no Sampler patch on any track,
  the new one included and opaque ones by their raw `sources` (`SamplerPatch::media`), and no clip
  on the engine's clipboard. It is also dropped from `edit.selected_sources`. The file on disk is
  never touched, and undo restores the whole previous document, entry and pool audio included.
  Managed sources stay, since they may be the only copy. This keeps the session (and its next
  save) to the kits in use; it does not by itself bound memory while stepping, because each undo
  step keeps the document before it, pool audio included, up to the 200-step history: see
  *previous and next* in item 6.
- *A pending Detect candidate* turns stale under the existing rule (the patch differs), so its
  Replace markers is refused as stale and the Slices view drops the preview; no new code.
- *Result:* `{track, kit, loaded, pads, zones, sources, missing: [{zone, pad?, path}], notice}`.
- *Real time:* the recall publishes one new generation through the existing handoff; decoding and
  admission run off the audio thread, as for every Sampler edit.

**5. Missing files on recall.** A file that does not resolve, is refused by the path check or the
read root, is not a regular file, or is changed or unreadable (item 4) is not an error: the recall
adds a session source with the resolved path it looked for (absolute, so a later session save
keeps it as referenced media; a path the lexical check refused is recorded as the kit wrote it
when that is absolute, and joined to the preset's folder as text otherwise, never as a bare
relative path, which a session would read against its own folder),
the kit's recorded `name`, `frames`, `channels`, `sample_rate` and `format`, `unsaved: false` and
no pool audio. Its zones and pads are created as saved. That is exactly the existing missing state:
the Instrument view and the Slices bench show the card naming the file, the zone plays silence,
it counts nothing toward the 128 MiB budget, and `check_sampler_patch` accepts it. The result's
`missing` list and one notice name them (the first 16 files, then "and N more"): `Kit "808": 2
samples were not found and play silence: Kick 01.wav (D:\Library\Kick\Kick 01.wav), ...`
(proposed text). Relink does not exist yet. Until it does, the user puts the file back where the
note says, or moves the kit folder so relative paths resolve, and recalls the kit again; or loads
a sample onto the pad (drop, Load…), which keeps the pad's settings. The Pads view gains the
smallest missing state, in slice 3: a pad whose zone's source is in the session with no pool
audio draws the shared hatch (`widgets::hatch`) over its cell and its hover text reads
`Kick 01.wav is missing` (the card's words); nothing else on the pad changes. Without it a
recalled drum kit, whose main view is Pads, would show silent pads as loaded, against #467's
"shows the views' existing missing-file state and names the file".

**6. Where kits live, and the UI.** **Proposed:**

- *Folder:* `<config dir>/presets/sampler/`, the existing user preset folder of the `sampler`
  plug-in id, with the extension `.tkit`. The extension keeps kits apart from the retired
  one-sample presets in the same folder: `retired_user_preset_lines` reads `.json` files only, so
  it keeps reporting those and never a kit, and builds before this amendment list no `.tkit` file
  at all. `user_names` for `sampler` lists `.tkit` stems (sorted case-insensitively, as now)
  through a new engine helper beside `io::presets::list` that reads with the checked access. The
  #455 rule "the Sampler shows no preset list until a patch preset exists" is met by this.
- *Factory kits:* none ship (body: no sample-dependent presets under `assets/presets/sampler/`).
  The factory default is the empty kit a new Sampler starts with (#454). `<factory default>` is
  only the field's label for a kit not recalled from or saved to a file (the header's `NO_PRESET`);
  it is not a list entry, and the list has no Factory section and no "Empty kit" entry in v0.7.0.
- *The list* (title bar field and editor dropdown) shows the user kits, Save As, Delete, and two
  file entries: **Open kit file…** (any `.tkit` through the file dialog, recalled by the same
  command; the field then shows its name, and next from a kit that is not in the list goes to the
  first, as `stepped` does) and **Save kit file…** (Save As to any folder, for a kit stored with
  its samples). Delete removes the `.tkit` file through the engine helper, which refuses any other
  extension.
- *Save* writes over the current user kit only when the window's last recall or save was that
  list file (the kit file kept above); a kit opened from elsewhere, or built by hand under a label
  that happens to match a list name, opens the list for Save As instead, so Save never replaces a
  kit the user did not open. The Sampler's list never calls the parameter preset code
  (`presets::save_user`, `load_user`, `delete_user`, `StudioApp::apply_preset`): on the #455
  branch its Save As still writes an empty `.json` parameter preset into `presets/sampler/`, which
  the next launch would report as a retired one-sample preset.
- *Ask before replace.* When the current kit has a zone or a loop, the UI asks before it sends
  `replace: true`: `Replace the kit? 12 loaded pads will be replaced by "808".` with **Replace** and
  **Cancel** (proposed text; the dialog component is the one #465 adds when it lands first). There
  is no "fill free pads" here: a kit preset is the whole kit.
- *Previous and next.* Each step is a replace. A step, a pick from the list and Open kit file…
  ask only when the kit has changed since the last recall or save (the UI keeps, per device
  window, the patch and the kit file as they stood after its last kit recall or save, and
  compares patches); with no such record (a kit built by hand, a reopened session, a track
  preset, an undo past the recall) any zone or loop counts as changed and the question is asked.
  Stepping through unchanged kits asks once, at the first step away from edited work. Nothing a
  step replaces without asking is lost: it is in a file, and undo restores it. Consecutive
  previous and next steps on one Sampler with nothing else between them are sent with
  `Engine::execute_merged` under one key per track, so a run of steps is one undo step back to the
  kit before the run, and the history keeps two kits' audio, not one per step (200 steps of a
  100 MiB kit would otherwise hold 20 GB).
  Convention: in both reference workstations, stepping or hot-swapping a drum rack or sampler
  preset replaces without a question and relies on undo; this record keeps the maintainer's
  question for unsaved work. That behavior is from the record's sources and general knowledge of
  both products; this job could not check it against their current manuals.
- *Selection after a recall* (UI state; the engine reports none): the view and the bank are kept;
  the selected pad index is kept (every kit has 64 pads); a selected zone that the new kit does
  not have is cleared; the Slices view's marker selection is cleared.

**7. Track presets (#458).** The amendment decides that a track preset carries a Sampler's kit
in the same portable form, so #458's Sampler half is fixed by the same code:
`track.save_preset` writes `instrument.sampler_patch` through `to_portable`, paths relative to the
`.tpreset` file under item 2's rules, and is refused for an unsaved managed source exactly as
item 3 says. `track.load_preset` sends a Sampler instrument's patch through item 4's staging
(resolve, admit, missing state, the checked store) once per target track; sources are shared
between targets (the same resolved file is decoded once), and any refusal refuses the whole preset
load, nothing applied. The track preset load is already one undo step and keeps its other rules
(routing kept, #455's slot drop). It does not ask before replacing: loading a track preset already
replaces the track's instrument, and that command never asked. A track preset written before this
amendment has `source` ids and no `file`: alpha, no migration. Each such entry becomes a new
session source in the missing state with an empty path and the name `source <id>` (the preset
holds nothing else about it), so it can never resolve to a sample of the loading session, and
the status line says: `track preset "Bass": its Sampler samples were saved by session id and
cannot be found: load them again.` That ends the wrong-sample bug without guessing. A track
preset whose Sampler patch this build cannot read (unknown schema or invalid) is refused whole,
naming the file, as a kit with an unknown schema is (item 9). The other half of
#458 (an instrument retired before #455 loaded from a track preset) stays in #458.

**8. Control channel and help.** `docs/control-protocol.md` gains a "Sampler kit presets" section
with two rows: `mix.sampler_kit_save` `{track?, path}` and `mix.sampler_kit_load` `{track?, path,
replace?: false}`, with the results and refusals above, the path rules of item 2 and the file
format of item 3, and its `track.save_preset`/`track.load_preset` text says that a Sampler's
samples are file references. `docs/mcp.md` adds `mix.sampler_kit_load` (and the files a kit
names) to the read root list and `mix.sampler_kit_save` to the write root list. Each command's spec
text (the `cmd!` params string) carries the same summary, which the control channel's help serves.
The preset list's new entries and the ask dialog get help entries and ids per `docs/help/README.md`
(`cargo xtask help --write`, then `cargo xtask help`). The session-format knowledge note gains a
short "kit preset file" section pointing here.

**9. Compatibility.** A kit file whose tag is not `transit-sampler-kit`, or whose `version` is above
1, is refused by `file_tags` with its standard messages before anything else is read. A kit whose
`kit.schema` this build does not know is listed (its name shows) but recall is refused: "saved by
a newer transit (Sampler kit schema 2)"; the file is never rewritten, and Save over it is refused
(item 3). Installing it as an opaque, silent patch would replace a playing kit with silence, so
the recall refuses instead. A schema 1 kit with fields this build does not know recalls; the
fields are kept in the session's patch (`extra`), and a later Save writes them back. An invalid
kit (fails `PatchV1::validate` after `from_portable`) is refused with the reason, nothing changed.
Builds before this amendment do not list `.tkit` files and do not report them. `track.load_preset`
in those builds reads a new track preset's Sampler patch as a ready patch whose sources all name
session source 0 (`SourceRef` defaults a missing `source`, and the session never allocates id 0,
`Session::alloc`), with each `file` kept as an unknown field: every zone is missing and silent,
and nothing resolves to an unrelated sample. No `Session::needs` entry is added: the session
format does not change.

**10. Decoding cost and #450.** Recall decodes up to 256 sources and 128 MiB. Before #450 it uses
Load kit's pattern: a worker stages the candidate and the command joins it, so the engine and UI
wait for the decode, with no progress and no cancel; admission from headers runs first, so an
over-budget kit is refused before anything is decoded. When #450 lands, kit recall becomes the
same staged job as Load kit and pad drops: the command starts the job and answers at once, the job
reports progress and can be canceled, and committing it applies the candidate as one undo step
(cancel leaves nothing). The staging function is written once for both commands so #450 changes
one place. The same holds for `track.load_preset` with a Sampler.

**11. Decided with the coordinator (2026-11-11).** The items marked **proposed** above are
decided as written, and these were open:

1. *The extension `.tkit`* and the tag `transit-sampler-kit`: accepted; ADR 0049 gave the other
   side files `t`-prefixed extensions, and the extension keeps kits out of the retired-preset
   report.
2. *The question* is asked only when the kit changed since its last recall or save, for previous
   and next, picks from the list and Open kit file… alike (item 6). This narrows #467's "when the
   current kit has loaded pads, ask before replacing"; the maintainer confirms it by accepting
   this record.
3. *"Loaded"* means any zone or the loop, not only pad-bound zones: an Instrument-only kit is work
   too.
4. *A changed file* (found, but its rate or length differs from the kit's record) takes the
   missing state and the rest of the kit loads (item 4), as #467 asks for a moved sample.
5. *The replaced kit's unused referenced sources leave the session* (item 4), and a run of
   previous and next steps is one undo step (item 6), which is what bounds memory while stepping.
6. *Track presets* (#458) are slice 4, right after the kit preset slices, because they are the
   same code; they ship in v0.7.0 if ready, else v0.7.1.
7. *No "Empty kit" entry* in v0.7.0; a new Sampler, or clearing the zones, covers it.

**Decided by the coordinator, to be confirmed by the maintainer (2026-11-11).**

1. *The question is asked only when the kit changed since its last recall or save* (item 6 and
   11.2), not on every replace of a kit with loaded pads: asking on every previous or next step
   would make stepping unusable, and the replaced kit is in a file and undo restores it.
2. *No modified mark* in the title bar (`808 *`) after editing a recalled kit in v0.7.0. No device
   shows one today; the window already keeps the baseline, so it is a small later addition.
3. *A run of previous or next steps is one undo step* (item 6): undo after browsing five kits
   returns to the kit before browsing, not the fourth. This keeps memory bounded, and the browsed
   kits are files.
4. *Samples on a network share load only when the kit file is on that share* (item 2), the
   session's rule: a shared kit never makes the app contact another computer.

**12. Evidence** (implementation gates, not claims made here). No pinned hash: every test
compares documents, JSON and file bytes, or renders in-process.

- A round trip of a kit covering every field (pads with stored notes, names, choke, mute and
  solo; zones with every playback field, fades, membership, crossfades, labels; groups in both
  round-robin modes; a loop with both marker sets, ids, labels, `next_id`, mode, detector, grid,
  bars, tempo override, root and beat length; slice-bound zones; controls; the seed; a nondefault
  pad start note; unknown fields at the top and in entries): save, recall into another session,
  and the recalled patch equals the original except for the source ids.
- Paths: a sample below the preset folder is written relative with `/`; one elsewhere absolute; a
  Windows `\\?\` path is written without the prefix; moving the whole kit folder keeps every
  sample found; moving the kit file alone keeps the absolute ones found and makes the relative
  ones missing; a relative path is resolved against the preset file, never the working directory
  or the session folder; a path outside the read root is missing, not read.
- A missing sample: the zone exists, its source is listed with the recorded facts and no audio,
  the Instrument view shows the card, the notice names the path looked for, and the rest of the
  kit plays.
- One undo step: undo restores the old patch, the session's source list and the preset label
  exactly; redo reapplies.
- Without `replace: true` on a kit with a zone the command answers `replace_needed` and changes
  nothing; on an empty kit it needs no `replace`.
- Refusal on budget: a kit over 128 MiB (generated audio) is refused from headers with nothing
  decoded and the document unchanged; a kit with nine voices on one note is refused naming the
  note; a changed file is refused naming it.
- Save refusals: an unsaved managed source names its pad and writes nothing; an opaque patch is
  refused; an existing newer kit file is not replaced. A missing source at save time is written
  and listed.
- Compatibility: a kit with `version: 2` and one with `kit.schema: 2` are refused, and the file
  is byte-identical afterward; a schema 1 kit with an unknown field keeps it through recall and
  save.
- Track presets through the same path: a `.tpreset` with a Sampler carries no `source` id; loaded
  into another session it finds its samples by path; a pre-amendment `.tpreset` loads its zones as
  missing with the status line; a refused kit refuses the whole preset load.
- The retired-preset report still lists the old `.json` Sampler presets and never a `.tkit`, and
  Save As on a Sampler writes no `.json`.
- Names: `808 v1.5` saves as `808 v1.5.tkit`, and `Kit.1` and `Kit.2` are two files.
- A changed file (another frame count; another rate) and an undecodable file take the missing
  state, named in the notice, and the rest of the kit plays.
- Targets: a recall onto an opaque patch and onto a frozen track is refused, nothing changed.
- Sources: after a recall, a source of the old kit that a clip, a freeze, another Sampler or the
  new kit also names stays; one nothing names is gone from the session and the pool, the file is
  untouched, and undo brings both back.
- Stepping: three merged steps are one undo step back to the first kit, and the undo history
  holds no pool audio of the middle kit.
- Paths: a sample reached through a link loads; a network path outside the kit's share is missing
  and is stored absolute, never relative.
- The Pads view hatches a pad over a missing zone (in-process render).

These negative mutations must fail them: `to_portable` writing a session id; a relative path
resolved against the session folder; the layer check run against the old kit instead of an empty
one; a refused recall leaving an added source behind; `replace` ignored; the `\\?\` prefix kept;
a newer kit file overwritten by Save; the extension replaced instead of appended; a removed source
still used by a clip; a changed file decoded into the kit.

**13. Slices.** Each builds and passes on its own; #455 lands first.

| Slice | Content | Rough size (non-test lines) |
|---|---|---|
| 1 | Model: `sampler_portable.rs` (`to_portable`, `from_portable`, the path rules of item 2 as pure functions) and its tests. | +200 |
| 2 | Engine: `mix.sampler_kit_save` and `mix.sampler_kit_load` in a new command file (staging shared with Load kit, missing and changed files, admission, the replace flag, the opaque and frozen refusals, the old kit's unused sources), `store`'s replacing argument, the `.tkit` list, read, write and delete helper, and tests. | +450 |
| 3 | UI: the Sampler's preset list on kits (list, Save, Save As, Delete, previous and next as merged steps, Open and Save kit file…), the ask dialog, selection after recall, the Pads view's missing hatch, help entries, and tests. | +350 |
| 4 | Track presets (#458): `track.save_preset` and `track.load_preset` through the portable form and the staging, the pre-amendment note, and tests. | +120 |
| 5 | Docs: control protocol, `docs/mcp.md`, the user guide's Sampler section, the session-format note, the release note. | docs only |
| 6 | With #450: kit recall (and a track preset with a Sampler) as a staged job with progress and cancel. | with #450 |

The minimum useful cut for v0.7.0 is slices 1 to 3 and 5 without Open and Save kit file… (the
control channel already takes any path): a kit built by hand is saved under a name, listed,
stepped and recalled in any session, with missing files shown. Slice 4 follows (v0.7.1 at the
latest), and slice 6 rides with #450.

**Alternatives rejected.**

- *Parameter-only presets, as today:* the Sampler has no table parameters, so such a preset holds
  nothing; the kit is the patch.
- *Copying the audio into the preset* (a folder or an embedded copy): the maintainer chose
  references for the first version, and copies duplicate a sample library per kit; #467 lists it
  as later.
- *A binary bundle* (one archive with the patch and the audio): not plain text, cannot be read or
  diffed, and needs its own reader; the same reasons as copying audio.
- *A second kit format beside the session's patch:* two serializations drift; `from_portable`
  feeding `SamplerPatch::from_value` keeps one.
- *Session source ids in the preset* (today's track presets): the bug #458 records.

**Reopen when** kit presets must carry their samples (the later feature #467 names), Relink lands
and missing files want a search rule, or the patch moves to a schema 2 that changes how sources
are named.

## Amendment 2026-11-11: click plays a slice, double-click adds a marker (#469)

Amended 2026-11-11: decided by the maintainer (#469). This replaces the bench's add gesture in
Decision's Manual paragraph ("Clicking empty bench space adds a marker") and in the amendment of
2026-11-03 ("click to add"); everything else stands.

- **A single click on the waveform** (the bench under the flag strip) selects the slice under
  the pointer and plays it the way Space does (the slice's pad note at the piano roll's
  velocity) while the button is held. Release ends it, and so do the pointer leaving the window,
  the window losing the focus and the window closing. It never starts the song, changes nothing
  in the kit, is no undo step, and in Auto copies nothing to Manual. One audition is held at a
  time: whichever of the click and Space or Enter started last owns it, and only its own release
  ends it. The click does not scroll the view, so the second click of a double-click lands
  where the first did.
- **A double-click on the waveform** adds one marker at the pointer with every existing rule:
  the `too close` refusal, the zero-crossing snap, one undo step, and in Auto the copy to Manual
  in the same step with its notice. Its first click selects and plays as above. The double-click
  is the toolkit's own detection.
- **Unchanged:** dragging a Manual handle (silent: a press on a handle plays nothing); the flag
  strip (a click in a cell selects, a click on a flag removes its marker, a double-click in a
  cell names the slice); Space and Enter, the arrow keys, the nudges and Delete. With a menu, a
  modal or a dialog open, or while a field has the keyboard, the click and the double-click do
  nothing, as the keys do nothing. The bench's hint and its hover tooltip name the double-click
  (`double-click adds a marker`); the ghost line still shows where an add would land.

## Amendment 2026-11-11: playheads on the Slices bench (#470)

Amended 2026-11-11: decided by the maintainer (#470): a playhead for every sounding voice in the
Slices view. The record was silent on a play position in the Slices view; everything else stands.

- **Every sounding voice of the slice loop** draws a thin vertical line across the bench's
  waveform at the source frame it reads, whatever started it: a click on the bench, Space or
  Enter, a pad, MIDI or the song. Release tails count while they sound. The purpose is to show
  where a transient that is heard sits in the loop, to place and nudge markers.
- **The position is the voice's own.** The instrument publishes each sounding voice's zone and
  its phase in the source's own frames (the bench's coordinate) after every block, so pitch, the
  source's rate, loops, ping-pong and reverse are already in it. The view draws what the last
  block published and never extrapolates between blocks with a clock: the line moves in audio
  block steps. Publishing is display only: it never changes the audio, and it costs nothing
  while no Sampler window is open.
- **Only the loop's source:** a voice whose zone plays another source draws nothing. A voice
  outside the visible zoom range draws nothing, and the view never scrolls to follow a voice.
  Every voice has a slot (the published places equal the voice count), so no voice is left out.
- **Nothing moves when nothing sounds:** with no voice of the loop sounding, no line is drawn
  and the view asks for no repaint. A bypassed or inactive Sampler, or an inactive track, shows
  none, as no bypassed device shows a moving meter.
- The line is 1 px in `waveform/playhead` (`text`), the token the zone waveform's playhead
  already names.
