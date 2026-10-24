# E52: the Measure Cancel test assumed the MIDI output thread had kept up with the pump

1. **Failure.** CI run 40133669975 (macOS, main 4664073) failed
   `cancel_releases_the_held_note_and_measures_nothing` at its `(2..=3).contains(&ons)` assertion:
   the port held `[92 3c 64] [82 3c 00] [b2 40 00] [b2 7b 00]`, one note-on (the warm-up) instead
   of two or three. Windows, Linux and both local gates passed.
2. **Mechanism.** The test pumps the fake duplex device far faster than real time (about 800
   blocks, 3.1 s of audio, in tens of milliseconds) and cancels at a fixed block. The Measure's
   notes are stamped with each block's host time and the MIDI output thread sends them on its own
   schedule (a 1 ms poll). On a busy runner that thread was descheduled after sending the warm-up
   note-on, so the warm-up note-off and both measured note-ons were still pending at the Cancel.
   Cancel's `release_track` (`player/midi_out.rs`, `release_track_now`) correctly drops a track's
   unsent scheduled playback and releases only what reached the synth: note-off for the warm-up,
   Sustain off, All Notes Off. The product was right; the test counted sends that depended on the
   output thread's timing, not on the Measure.
3. **Why checks missed it.** Locally the output thread keeps pace with the pump. The test's
   comment allowed the second measured note to be dropped but assumed the first had gone out, an
   implicit bound of "the output thread is no more than a few hundred blocks behind".
4. **Regression test.** The test now waits (`until`) for the warm-up and both measured note-ons to
   reach the port before it cancels, checks nothing is released yet, then requires exactly three
   note-ons, each released. Mutation: a 300 ms sleep after each note-on delivery in the output
   thread (`Worker::send_due`) reproduces the macOS vector exactly with the old test and passes the
   new one.
5. **Gate class.** A test that pumps a fake device faster than real time observes another thread's
   output only through a wait on that output, never at a fixed block count
   (`docs/contributing/evidence/gates.md`).
6. **Protocol change, owner, blind spot.** The rule is in `gates.md`. Owner: the coordinator.
   Blind spot: other pump-driven tests that read the fake MIDI port right after a pump were checked
   in `external_measure_tests.rs` only; they either wait on the port or read after All Notes Off,
   which the output thread sends last.
