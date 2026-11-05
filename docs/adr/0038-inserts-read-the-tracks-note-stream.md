# ADR 0038: Inserts may read the track's note stream; the Filter insert plays envelopes from it

- **Status:** proposed 2026-10-31 (#242); amended 2026-11-05 (#332, offline prints start from a settled chain)
- **Date:** 2026-10-31
- **Reopen when:** a hosted effect with a note input (a CLAP effect with a note port, a VST3 effect with an event bus) should get the stream too; an insert needs per-voice information the summed audio cannot give it (the per-voice filter of the Macro Oscillator, #243, is that case and stays inside the instrument); a rack strip's own inserts want the notes of the rack's members; or a block's note view overflows in practice (`EventStatus::midi_fx.note_view_dropped` above zero in a field report).

## Stored parameters at preparation (#280)

Parameter values already held by the session when a fresh built-in is prepared are its initial
state, not a parameter move. Following the Ableton and Logic convention of bouncing from the
stored state without a startup sweep, the shared plan compiler applies those values and snaps
their smoothers before the first block, for both offline and live instances. The final snap uses
the plug-in's reset behavior by default; a restored-state instrument must preserve its cloud
while snapping parameter targets. Retained running instances are never snapped by a session
edit. Automation, including a point or ramp starting at time zero, is applied after preparation
and continues to use normal smoothing, on a chain an offline print has already run in (see the
#332 amendment below).

## Context

The Macro Oscillator ([ADR 0033](0033-built-in-instruments-mutable-instruments-port.md)) is true to its source: it has no filter, amplifier or ADSR beyond the firmware's low-pass gate. Users who want a classic subtractive sweep on it, or on any instrument, need a filter whose cutoff follows the notes. An insert effect sees only audio: until now nothing gave an insert the notes its track plays.

Convention (transit follows what Ableton and Logic agree on): Ableton's Auto Filter and Logic's AutoFilter both offer an envelope follower and an LFO on the cutoff, and neither offers a note-triggered ADSR (Logic's AutoFilter does have an ADSR, but it is triggered by an input threshold, not by notes; transit's is note-triggered): an effect in either DAW cannot see the track's MIDI (Ableton's sidechain is audio; Logic's AutoFilter has no MIDI input). Here transit does better on purpose, because the note stream is already in hand at the strip (ADR 0036) and the missing envelope is the most requested piece of a subtractive patch built from a plain oscillator.

## Decision

- **The plumbing.** `Plugin::reads_track_notes` (default false). Every event the mix gives an instrument already leaves through one call: the note table's `send`, or the MIDI effect host's deliveries and its edit releases when the track has MIDI effects ([ADR 0036](0036-midi-effect-slot.md)). Both now go through `MidiFxHost::give`, which, when the plug-in accepts the event, records it in the host's `NoteView` (`crates/mix/src/note_view.rs`): note-ons, note-offs, the sustain pedal and the all-notes-off and all-sound-off controllers, at the offsets the instrument got them. After the instrument processes, the strip gives the view, through `Plugin::event`, to each running insert that reads notes, then clears it. It is a read-only view: the same events, aligned to each reader's input audio, after the MIDI effect slot, and nothing an insert does reaches the instrument.
  - **Upstream latency (#247).** Each reading insert delays the accepted note view by the instrument's reported latency plus the latency of every preceding running insert and any alignment delay before its own input, using the strip's running PDC sum each block (`NoteDelay::deliver`, `crates/mix/src/note_delay.rs`). Its fixed carry travels with the slot: 16,384 events (`MAX_LATENCY / 64`, one event per 64 frames of the plan's 2^20-sample maximum latency), with 64 release slots and the final open slot reserved for an all-notes-off stand-in on overflow. Pending events keep their source frames and follow latency changes on the next block; overdue events arrive at frame 0 in source order, with matched note-offs kept at least one sample after their note-ons (#257) and later offsets never decreasing; a release that cannot fit waits with everything behind it for the next block. At most 512 events leave per block, with 64 release slots, so at most 447 non-releases: the same 447 plus the all-notes-off stand-in that fills the 448th open slot of the note view (#246), not an off-by-one; excess due events remain in the carry and arrive late. Larger reported delays also fit the timestamp representation and are never shortened. Starts are refused (and counted) from an overflow until its stand-in has been delivered; after that the carry accepts starts again, since the stand-in covered every dropped one. Only events lost when the carry fills count in `note_view_dropped_total` and the engine's `note_view_dropped`, once per reader: an event two readers both lose counts twice; a reset or a bypassed, inactive or failed reader clears its carry along with releasing the reader, while whole-strip render-pool skips retain undelivered events to arrive late. Zero upstream latency with no carry forwards the original view unchanged. Offline and live use this same rule.
  - **Instrument generation (#275).** Each reader's carry remembers the instrument slot's `SlotGen`, the compiler's instance identity that survives plan moves and changes for a replacement, including a fresh instance of the same kind (instrument specs carry their `InsertUid` so the compiler distinguishes that replacement); no instrument is `None`. Before a retained reader processes its first block from a changed or removed source, it receives `all_notes_off` to release every held or sustained note without cutting its envelope tail, followed by sustain-up on all channels through the note view's fixed storage, before any new-source events. The old carry is cleared, including pending starts; these intentional discards are not overflow losses and are not counted, like reset and bypass. Skipped readers keep the existing release/clear rule and reconcile their source before processing again. Keeping the same instrument preserves the reader and carry unchanged. A MIDI effect chain edit does not reset readers: notes and the chain's edit releases keep flowing from the same instrument source. Readers on auxes or buses without an instrument are unaffected. No allocation, lock, or unbounded held-note traversal is added.
  - Events given between blocks (a Stop's or a seek's releases, at offset 0) wait in the view for the next block, then follow the reader's upstream delay.
  - An insert skipped because it is bypassed, inactive or failed receives the shared reader release once per consecutive run of skipped blocks (`all_notes_off`, then sustain-up on all 16 channels from the note view's fixed storage), preserving its release tail and clearing its carry because replaying an old pedal-down would undo that release and a delayed start could have missed its note-off (#276); whole-strip render-pool skips keep their undelivered stream to arrive late.
  - A track without an instrument gives its inserts nothing. A rack member's insert reads that member's own stream (what its instrument accepted after the rack's and its own MIDI effects); the rack strip's own inserts read nothing.
  - Fixed storage made with the instrument's event state: 512 events a block (`NOTE_VIEW_CAP`, the most a block takes into the MIDI effect chain), the last 64 (`RELEASE_RESERVE`) for note-offs and releases only. When the other slots are full the view puts an all-notes-off in the next one and counts what it dropped (`MidiFxCounts::note_view_dropped`, also summed per process into Copy diagnostics), so an overflow never holds a note forever, and releases that follow still reach the insert. Nothing allocates, locks or logs on the audio thread.
  - Only built-ins read notes for now. A hosted effect keeps the default.
- **The Filter (`filter`).** One insert effect, paraphonic: one filter and one envelope pair after the instrument's voice sum. A new note retriggers the envelopes for every held note (Every note mode; Legato retriggers only when nothing was held), the gate stays open while any note is held or sustained by the pedal, and the envelopes release when the last note lets go. This is the classic paraphonic behavior and the limit of an insert: it cannot give each voice its own filter. The per-voice filter belongs in the instrument (#243).
  - A multimode trapezoidal state-variable filter (low pass, high pass, band pass with unity gain at the cutoff, notch), 12 dB per octave or 24 dB as two sections in series (the second a Butterworth section, so the resonance peak keeps its height), resonance from Q 0.707 to 25 (no self-oscillation), stereo-linked coefficients, and drive into a soft clip before it that is exactly linear up to full scale.
  - The cutoff is `cutoff · 2^(6·(env·env_amount·velocity + follower·follow_amount + lfo·lfo_amount) + key_track·(key − 60)/12)`: each source has a bipolar amount, 100% is six octaves at full swing, key tracking at 100% is an octave per octave from middle C (the last note played). Sources: the ADSR (velocity amount, retrigger mode), an envelope follower on the input (attack, release), an LFO (sine, triangle, saw, square, random; a free rate or a tempo-synced division; phase retrigger on the envelope trigger).
  - An amp stage: a second ADSR from the same notes, with velocity, multiplies the output when on (off by default, so the Filter passes audio with no notes).
  - **Timing.** Every source and the filter run per sample on the plug-in's own sample clock, which a host block boundary does not interrupt; a note starts its envelopes at its own frame. The coefficients are worked out every 16 samples of that clock (a 3 kHz control rate at 48 kHz) and at every note-on: `tan` per sample costs far more than the zipper it would remove, and a note-on update means a pluck's first cycle already sees its envelope. The output for the same input and the same events at the same absolute frames is bit-identical whatever the host block size, except with a synced LFO while the transport plays: there the anchor is set at each host block start, so block sizes agree within 1e-6, not to the bit (tested from 1 to 1024 frames, and in the engine at 64 and 512, including across tempo and meter changes under the host split rule below).
  - **A synced LFO follows the song position (#245).** While the transport plays, the LFO is synced and its retrigger is off, the phase is set at each host block start to the song position over the division's length (mod 1): absolute quarter notes for beat divisions, and bars plus the fraction of the current bar for bar divisions (#253), counted in whole frames from an anchor (the transport's sample and quarter position, taken when playback starts, jumps or the tempo changes) so that every block size gives the same phase for the same frame. This is the rule the Macro Oscillator's matrix LFOs use (#243: `macro_osc.rs` takes the same anchor, `macro_osc_shaper.rs` sets a synced, non-retriggering LFO's phase from it), so a Filter LFO and a matrix LFO at the same rate stay in phase. The update grids differ and each device keeps its own: the Macro Oscillator anchors every 12-frame voice block, the Filter once per host block and then advances per sample. With the sync off, the retrigger on or the transport stopped the LFO free-runs from its last phase. The host splits at tempo and meter changes (#254), so every block size supplies the new transport on the same frame. The engine regressions at 64, 512 and 4096 frames are bit-identical through the changes; the device-level 1e-6 floor remains for other anchor positions.
  - Presets Pluck, Wah (envelope follower on a resonant band pass) and Paraphonic Brass (Legato). Category EQ, as hosted filters are classified. Not offered as an offline process: it plays from notes an offline process does not have.

## Consequences

- Any built-in effect can now follow the notes (a gate, a tremolo, a vocoder later) with one trait method and no new plan cell; the view travels with the instrument instance like its queue.
- Hosted effects with note inputs are not served yet; their hosts would need to forward `Plugin::event` for effects, which the CLAP and VST3 hosts do not do for effect slots today.
- The Filter is not polyphonic, and says so in its help and the user guide. Playing chords with retrigger on restarts the sweep for the whole chord.
- A bypassed Filter forgets held notes; un-bypassing mid-note leaves the envelopes released until the next note.
- Bar divisions count from bar 1: `(zero-based bar + quarters into the bar / quarters per bar) / bars per cycle`, mod 1 (#253). Both devices use the shared rule in `crates/dsp/src/plugins/lfo.rs`. A meter change updates the bar anchor at the changed bar start, even in the middle of a multi-bar cycle; it does not restart the cycle mid-bar or reset its bar count. The tempo map places meter changes on bar lines. As with tempo changes, the new snapshot reaches each device at the next host block start; its phase includes the elapsed part of that bar. Beat divisions retain their absolute-quarter anchor and arithmetic. Bar lengths cover the full tempo-map range (1/64 through 64/1); the old 0.25-to-64-quarter clamp would disagree with the supplied bar position at those extremes.
- Tempo-map split rule (#254): `MixEngine::render_active` cuts every live, offline and headless host block at tempo and meter changes, on the first integer frame at or after each exact change. Each sub-block has its own transport and rebased MIDI and parameter offsets. Changes sharing a frame coalesce; at most one sub-block per host frame (the contract is `TempoMap::render_span`). A boundary on the first frame needs no split; one on the last included frame leaves a one-frame sub-block. The synced LFO anchor rule above is unchanged.

## Alternatives considered

- **A MIDI sidechain from another track.** More general, but needs routing UI and a key-like source selection for notes; the track's own notes are what this use asks for. Can be added later on the same view.
- **Feeding inserts from the clips and live input directly.** Would bypass the MIDI effect slot and the note table's matching, so a transposed or velocity-scaled note would not be what the insert hears.
- **Putting the envelope and filter inside each built-in instrument.** That is #243 for the Macro Oscillator; an insert also serves hosted instruments.

## Amendment: offline latency changes (#251)

The offline range renderer tracks the chain latency per block exactly as the live path's
compensation does: Main and output paths use the one device latency from ADR 0015 section 6;
a bus render uses that bus's arrival latency. This replaces the Texture review's #251 known
limit in ADR 0033. Device latency declarations and the live renderer are unchanged (option A).
A fixed maximum for all settings (option B) would charge every live Digital Texture the
Dirty Tape converter latency to solve an offline placement problem.

For a rendered block starting at relative frame R with reported latency L, its first sample
belongs at output frame R - L. The output cursor only advances: discard the part overlapping
already delivered frames when L grows, and insert zero frames for the missing interval when
L shrinks. A decrease does not keep the larger skip, which would leave subsequent audio early.
Both adjustments can span several blocks; later changes replace the current mapping rather
than accumulating stale skip debt. Render extra input as needed and deliver exactly the requested
range length. Freeze, commit, mix/bus bounce and path export all use this same `RangeRender`.
Freeze metadata retains the initial chain latency as a constant playback delay; the stored
audio has already been aligned.

**Equality and transition limits.** With constant latency, bounce is bit-identical to the live
samples after the initial trim, including the pre-change goldens. A latency change is the case
where a single trim no longer makes them equal: bounce splices the live samples using the
mapping above. Every retained sample is still bit-identical, and inserted gap samples are zero;
the dense-audio live oracle in `crates/mix/tests/offline_latency.rs` asserts that distinction over
growth, shrinkage and repeated changes. This keeps subsequent audio on the timeline rather than
preserving a permanent offset merely to match a live capture with one fixed trim.

An exact reconstruction through the transition is not available from the existing block-latency
interface. Live PDC blends two different delay positions for 256 samples, and a Texture restart
can occur within a block whose latency readout describes its ending state. The summed output
cannot be unmixed into its two delay histories. Keep that crossfade and the device's restart fade;
do not claim the splice removes their audible transition or recovers discarded audio. Built-in
parameter automation still takes effect at block starts, so a point inside a block is observed
at the next start, just as live. Offline does not change automation timing or plug-in DSP.

At 48 kHz, Digital to Dirty Tape changes the reported latency from 11 to 149 frames; Digital to
Vintage Digital changes it from 11 to 115. The dry peer's impulses now remain at their exact
timeline positions in either direction. Dirty Tape itself applies intentional wow/flutter after
the dry/wet mix, even at 0% wet: those effect offsets remain, as the DSP impulse tests already
require. The regression asserts its measured peak positions separately; it does not confuse
that modulation with converter or compensation latency. Compressor lookahead growth and
shrinkage preserve both tracks' exact impulse positions.

## Amendment: offline prints start from a settled chain (#332, 2026-11-05)

An offline print (Bounce in Place and Commit, Freeze, File > Bounce, stems and Bounce Paths, all
through `RangeRender`) runs its freshly prepared chain for `RUN_IN_SECONDS` (four seconds) of
audio with the transport stopped at the range start before the first printed frame, exactly as a
live chain has run between its creation and play: no clip plays, every lane's value at the range
start is delivered, every insert processes (none is suspended) and the output is discarded.
Stored values stay snapped before the first block, as the rule above says. Automation at time
zero is applied to that settled chain: a point at the range start prints its value from frame
zero, and a ramp starting there continues from it with normal smoothing from a settled smoother.
A hosted plug-in's activation behavior (an activation fade, parameter smoothing after its state
is restored) is over before frame zero. This is the Ableton Live and Logic Pro convention: both
print through the instances already running, so a print starts from a settled chain at the
automation's value for its position (escape E71).

For users, every print starts as live play from a stop at the range start does, so a plug-in's
wake-up never lands in a print. The cost is four seconds of processing per print, bounded by the
chain's own cost: an empty chain runs in for almost nothing, a heavy one costs what four seconds
of it costs offline. A chain with no inserts and no instrument runs in too, so every print takes
the same path. A plug-in whose warm-up outlasts the run-in, or that finishes loading on its own
thread by wall-clock time, can still reach a print (E71's blind spots). The gate:
`an_automated_gain_insert_prints_its_lane_value_from_frame_zero` and
`a_hosted_plug_in_warm_up_settles_before_the_first_printed_frame`
(`crates/mix/src/offline_prime_tests.rs`) run with every workspace test, and
`docs/contributing/evidence/gates.md` requires an offline print path test to render a settled chain.
