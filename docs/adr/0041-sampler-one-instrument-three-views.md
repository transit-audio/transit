# ADR 0041: One Sampler instrument with three views (Instrument, Pads, Slices)

- **Status:** proposed 2026-11-01 (decided by the maintainer with the coordinator; slices filed)
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
| Controller mapping | A 16-pad controller addresses visible slots 1–16 of the selected bank, one to one. Default trigger notes are 60–123; bank changes do not rewrite stored notes. A press captures its destination and note identity until release, even across bank changes. Ordinary MIDI notes always address the stored trigger notes directly. |
| Choke groups | Off plus groups 1–16, labeled numerically; color is supplementary. Groups cross banks. An admitted trigger fades every other sounding pad in its group over exactly 32 output frames. This allows independent hi-hat and percussion groups without dependence on UI colors. |
| Pad voice model | Monophonic per pad; retrigger cuts the previous voice at the event frame and starts at the region entry with a fresh envelope. No layered retrigger option in #269. This keeps drums predictable and expansion reproducible. The short attack/declick can soften the new attack but does not promise a click-free hard cut. |
| Dragging slices | Allowed between pads, including banks. Move source/region/slice-id/name into an empty pad's zone; swap those contents for an occupied target. Pad identity, zone identity, trigger note, automation address and all playback/mixer settings stay at the destination. This is an explicit remap, one undo step, with no change to existing MIDI. Marker order stays chronological. No silent overwrite and no two sources on a pad. |
| Expand naming | Explicit pad name, else slice label, else sample stem, else `Pad A1` (bank and slot). Duplicate names get ` (2)`, ` (3)` in pad order. Names are labels, never event identities. |
| Choke after expansion | Add the narrowly scoped rack choke contract below in #272, an amendment to ADR 0034's member model. Never drop choke with a warning: that would change an open/closed hi-hat pattern. |
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

Source order is valid WAV cue points/labels, then **the existing**
`transit_dsp::offline::detect_transients`, then beat-grid division. Auto chooses the first
nonempty usable result; the user can explicitly choose any source or edit manually. Cue
positions are source-frame indices: bound-check RIFF chunk sizes, sort, deduplicate, discard
out-of-range positions with a load note, retain labels by cue id, add region start and end.
Use `cue ` and `LIST/adtl/labl`; end is a boundary, not a zero-length slice. Marker moves keep
stable slice ids and strict order. Removing a marker merges into the preceding slice and frees
the following binding; references to the removed slice are reported. Regeneration requires an
explicit Replace markers action and is undoable. Never overwrite manual edits on reopening.

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

One loop per patch, at most 64 slices and 65 boundaries. It may coexist with loose sample pads,
but total occupied pads cannot exceed 64. Slicing previews any shortage before applying and
refuses excess instead of truncating. Defaults map chronological slice i to root+i, root 60;
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

### Crates and integration seams

| Owner | Work |
|---|---|
| `crates/sampler-core`, package `transit-sampler-core` (L1, Tier B) | Region reader, cubic interpolation, voices, selection, envelopes, loops, choke and processing trait over borrowed audio/event spans. Descriptive name, no vendor branding. `default = ["std"]`, no-default `no_std + alloc`, no model, file IO, egui or desktop Plugin dependency. |
| `crates/instrument-core`, package `transit-instrument-core` (L1, Tier B) | Pure move of shared shaper/matrix, required SVF/LFO/envelope/curve primitives out of dsp; then add the external amp-gain seam separately. Both dsp and sampler-core depend on it, never sampler-core on dsp. Preserve old module re-exports and numeric behavior. Register layers and the primitive crate's standalone status; sampler-core has this explicit L1 dependency. |
| `crates/dsp/src/plugins/sampler/` and registry | Desktop adapter retains `SAMPLER_INFO`/`sampler`, Category Instrument, `is_instrument: true`, `audiosuite: false`; typed parameters, events, transport, rack macros, legacy compatibility and prepared sample-bank handoff. Split the existing file by concern as pure moves before adding code. |
| `crates/model` / `crates/mix` | Serialized patch and source references; prepare the L2 disk-reader implementation behind an L1 borrowed reader interface and pass it down through the adapter. Model may depend on the L1 trait; L1 must not import `DiskSource`. Compile patch/mappings off-thread and retain instance generations, fingerprints, note-view and worker-retirement rules. |
| `crates/engine` / `crates/audio-io` | Atomic load/edit commands, checked filesystem access, cue/tempo metadata, existing decoder reuse, off-thread detector, Slice to MIDI, media collection, SFZ and expansion. Extend the current `cmd/sampler.rs` path as well as `cmd/file.rs` imports. |
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
| Slices / markers | 64 / 65, one loop; duplicate or invalid boundaries rejected. |
| Shaper / matrix | 64 voice states, two LFOs per voice, 16 rows per zone; settings for 256 zones allocated in prepare. |
| Events | 1024 per instance, existing 257-entry release reserve. Preserve stable sorting and the host's retry on refusal; no fresh Vec or sort allocation in processing. |
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

Offline bounce obtains raw samples through `DiskSource::with_sync` on the offline thread and
passes them through **the same core region/interpolation/loop function** as memory and live
windows. No second resampling or `StreamMap::sample` interpolation is inserted. IO errors fail
the export with the media named, rather than claiming a successful silent render. A fully fed
stream equals memory sample for sample; an underrun intentionally does not.

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
bindings, source references, choke groups, layer/RR groups and seed, slice ids/markers/labels,
detector settings, root and beat length. Physically serialize only populated entries and
nondefault controls. Every field has `serde(default)` (optionals default None, arrays/maps
empty, seed 1, root 60, controls as above); unknown fields are accepted and retained by the new
patch codec. Unknown schema versions retain their raw object and media references, report
unavailable and play silence; never rewrite them into an empty patch. Missing/invalid source
references stay in the patch, report silence per zone and can be relinked. Validate all counts,
ids, lengths, finite values and ranges before allocating render data. Reject an invalid new
edit; on load preserve an invalid patch opaquely with a load note instead of discarding it.

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
single rack event stage described by ADR 0036 before using choke: process the parent's MIDI
effects once, then zones and choke, then deliver each member's events before any worker renders.
Apply the saved pad mute/solo eligibility before choke, as the unexpanded Sampler does.
Do not use the current per-member copy of the parent's MIDI effects for this mode. Reserve
16 choke-control entries per admitted trigger in prepared storage and integrate them with
bounded event delivery. A trigger is committed together with its choke actions, or all are
refused for host retry; a full queue cannot start the new hat while leaving the old one open.
Members receive a private, offset-stamped `choke_pad` control that invokes the identical
32-frame post-shaper fade, including one-shot voices. Ordinary MIDI note-off is insufficient.
Stable same-offset order, source/generation matching, deferred worker delivery and Stop/seek
must be tested. No extra cross-thread mutable group state or lock in the instrument.

ADR 0038 currently sends no note view to a rack's own inserts. Expansion therefore refuses a
track with any original note-reading insert, with a message explaining that it cannot retain
that insert's note input yet; it does not silently change its envelope. Members' new inserts
follow ADR 0038 normally. An expanded pad rack's member MIDI effects that remap/delay triggers
are disabled in this first form: reject adding them while rack pad/choke mode is active, rather
than letting them bypass choke ordering. Ordinary racks remain unchanged. Save these additive
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
   emits stale data. Test failed read/export, cancellation/disk full, no partial patch, peaks
   mismatch with injected stamps, threshold boundaries, aggregate RAM admission and recovery
   with missing external media. No test waits for the real streaming thread to win a race.
5. **#272 expansion.** Compare original pads and expanded members through the real rack/mix
   paths, no member inserts, using the identical event schedule and sum order: sample-for-sample
   equality, not an unspecified floor. Test 16 pads across banks, duplicate naming, envelopes,
   reverse, solo/mute, per-note random identity, global level and automation, parent
   MIDI effects evaluated once, choke at offsets inside blocks and deferred worker events.
   Exercise saturated queues with a new trigger and choke as one admission, and verify note
   release matching after replacement. Assert 17 pads, unbound zones, full insert chains and
   note-reading insert refusals make no changes. All playlists/takes and folder clip playback,
   capture, editing, save/load, undo and redo must survive. Defaults leave ordinary racks
   unchanged; additive fields generate `rack_choke` compatibility warnings on previous builds.
6. **Every slice: realtime, storage and UI.** Probe allocations **and frees**, including first
   process, events, parameter changes, reset, 64 voices, all 16 matrix rows, max queues,
   patch adoption/retirement backpressure and stale stream replies. Preallocate outside the
   measured span; deliberately hold retirement to test the full path, and verify destruction
   on the control side. Supply block sizes 1, 7, 64, 127, 512 and irregular partitions with
   the same absolute schedules: compare audio bits and state fingerprints from fresh reset
   and save/load. Check the model corpus/serde defaults, new session round trips, unknown
   schema retention, autosave recovery, collected media, old-reader warnings and unchanged
   old sampler ids/mappings. UI tests cover all views without a sound/mapping edit, pad drag,
   controller press/bank/release, capacity errors and every help id. Negative mutations
   (wrong choke length, doubled amp, bad loop wrap, stale page generation or dropped take)
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
| #269 Pads and Slices | Tier B sampler core and shared-primitive extraction, compatibility path, full bounded patch/save model, pad controls/choke, four banks, cue metadata/existing detector/grid, Slice to MIDI/takes placement and controller hook. Basic one-zone Instrument panel remains usable. Move the shared extraction from #270 here so the dependency direction is valid from the first slice; activation of the shaper/matrix UI stays #270. No new AIFF/FLAC decoder task: preserve what loads today. Oversized new media is refused until #271. | Gates 1, 2, 6, both bare-metal checks and full workspace/oracles for extraction. |
| #270 Instrument | Zone editor, layers/RR, loop/crossfade rules, per-zone shaper/matrix behavior and panel. Legacy migration is already in #269; this slice extends its tests. SFZ stays #271, correcting the original Decision's claim that it arrives with this view. | Gates 3 and 6, both bare-metal checks, full workspace/oracles for shared changes. |
| #271 Long media and SFZ | ADR 0035 raw-source reader extension and explicit pools, attack caches, native-rate streaming, bounded conversion of long compressed files, peaks, shared-file fix, format integration and SFZ subset. AIFF/FLAC decode already exists; test/extend the bounded path. No realtime compressed stream or fix claim for K91 take placement. | Gates 4 and 6, exact disk/memory/offline equality and fault injection, bare-metal and full workspace integration. |
| #272 Expand to rack | Atomic conversion to at most 16 ordinary member tracks, exact choke and solo-mask support, single parent MIDI stage, folder clip playback/record/edit prerequisites, mappings/global gain, compatibility fields and explicit refusals. No discarded excess pads or choke. Implement the scoped ADR 0034/0036 amendments with the code. | Gates 5 and 6 through the real rack, including worker/queue probes, saved sessions and full workspace. |
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
