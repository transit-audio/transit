# E82: Sampler admission converted native PCM despite the playback contract (#449)

1. **Failure.** New Sampler samples were converted to the session rate on import. Native cue,
   slice and loop coordinates no longer named the original PCM, and pitching converted audio
   added another interpolation pass.
2. **Mechanism.** `crates/engine/src/io.rs` offered only session-rate preparation and stamped
   registered sources with that rate. `cmd/sampler_patch.rs` and `cmd/sampler_kit.rs` admitted
   converted lengths. `io/session.rs` decoded every reopened source at the session rate and
   cached only by path. The source-rate ratio already existed in `sampler-core/src/sampler.rs`
   and `voice.rs`; admission prevented it from receiving native PCM.
3. **Why checks missed it.** The import transaction test explicitly expected conversion, contrary
   to ADR 0041. Direct core rate tests bypassed the import and persistence paths.
4. **Regression test.** `a_zone_source_comes_through_the_import_transaction_and_dropped_bytes_are_managed`
   now asserts 8,820 native frames at 44.1 kHz instead of 9,600 converted frames. The native
   tests in `crates/engine/src/cmd/tests_sampler_native.rs` cover two rendering rates, retained
   PCM across kit/drop/slice inputs, persistence, old converted sources, loops and MIDI timing.
   Forced import conversion, a unity source-rate ratio, wrong reload rates and admission of a
   converted staged job are separate negative controls. The cue test also enters through import.
5. **Gate class.** Rate and frame-coordinate contracts must be tested through admission,
   publication and persistence, not only by handing a DSP core an already-correct buffer.
6. **Protocol change, owner, blind spot.** The engine gate owns the import-to-render comparisons;
   shared import preparation has an explicit native policy, and staged Sampler inputs request it.
   Timeline policy and old PCM remain intact. The full workspace gate retains the old-session
   goldens. This does not add band-limited repitch or disk-backed Sampler sources (ADR 0041).
