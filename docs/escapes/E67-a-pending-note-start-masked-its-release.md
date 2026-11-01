# E67: a pending note start masked its release (#290)

1. **Failure.** With the shaper enabled, the Macro Oscillator could hold a note after its key
   was released. Turning the shaper off did not end that voice until the key restarted. All
   Sound Off cleared it. The report used a Random Sequencer ahead of the instrument.
2. **Mechanism.** In `crates/dsp/src/plugins/macro_osc_voice.rs`, a note-on set `started` for the
   next 12-frame render. If a note-off arrived before that render, the `if started` branch
   gated the envelopes and skipped the `else if !sounding && was_sounding` release. It then
   stored `was_sounding = false`, losing the release edge permanently. Both envelopes stayed
   gated without a held key. With the shaper on, the firmware low-pass gate stays open and
   the amp envelope determines silence, so this was audible indefinitely. With it off, the
   low-pass gate concealed the envelope error. The switch preserves a released voice's path;
   a new phrase takes the new switch setting, explaining the reported clearing case.
   This is an **IMPLEMENTATION-SLIP**, present before the held-note switch rule of #262.
3. **Why checks missed it.** The existing release tests rendered between a note-on and its
   note-off. The same-key restart tests covered off-then-on and six-op pitch, not a pending
   start followed by an off before the next render. Audio-only bypass tests could miss the
   envelope error because the low-pass gate still released.
4. **Regression test.** `crates/dsp/src/plugins/macro_osc_release_tests.rs` drives 384 scripted
   sixteenths with varying keys, velocity, gate lengths and both collision orders. The first
   failure on `e6afca71` was `mode=off step=3 sample=21084 voice=3 key=60 has no held key but
   amp=Sustain filter=Attack`. The suite also covers phrases wholly between renders, host
   block partitions, shaper switches at and one sample around the off, and the reported
   clearing sequence. Assertions inspect each voice's envelope stages and bounded release,
   not just output amplitude. The separate fresh-target base control preserves the failure.
5. **Gate class.** Coalesced events must reconcile a deferred consumer's final gate state:
   test both event orders within one processing quantum, including a start and end that no
   intermediate render observes. In this instrument the render starts the phrase, then
   releases its envelopes whenever no key remains. The shaper's existing idempotent release
   preserves an ongoing tail. No shared envelope or allocator changes are needed.
6. **Protocol change, owner, blind spot.** Instrument maintainers keep the event-order and
   toggle cases in the DSP test gate, plus the allocation probe in
   `crates/dsp/tests/macro_osc_release_alloc.rs`. All time is injected sample counts. The
   hunt stopped at the instrument-alone failure as requested: it does not prove that the
   exact reported sequencer settings produce this collision through the MIDI effect slot.
   The sequencer and slot were not changed, and their delivery guarantees are separate gates.
