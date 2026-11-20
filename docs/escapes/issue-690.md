# A fix for saved I/O tables could not reach the tables the bug had already saved (#690, #759)

1. **Failure.** On a tester's Linux machine (native JACK on PipeWire, v0.7.3) Audio Devices > I/O
   paths read "Channels open on this device: 2 in, 2 out" with no "In 1", "In 2" or "In 1-2", only
   a path made by hand, although #690's fix had shipped. The fix (default input paths when a saved
   table goes from 0 inputs to N) never ran on that machine.
2. **Mechanism.** Before #690, adopting a saved table set its input count but made no input paths,
   and the app saved it so: v0.7.0 to v0.7.2 wrote tables that say 2 inputs and hold no input
   paths. #690's repair in `reconcile` (`crates/model/src/io_map.rs`) keyed on the saved count being
   0, the state before the bug, so a table the bug had already written (count N, no paths) read as
   "the user removed the defaults" and was left alone. The same table also served whichever input
   was open, because it belonged to the output device (#759 case 2).
3. **Why checks missed it.** #690's test built its saved table with today's constructor at 0 inputs
   (`IoMap::for_device(.., 0, 2)` in `crates/ui-studio/src/io_settings.rs`), the state before the
   bug. No test loaded a table in the form the defective builds wrote, so the repair was proved only
   on tables the defect had not yet touched.
4. **Regression test.** `crates/model/tests/io_old_tables.rs` loads `io_maps` entries written as
   v0.6.x and v0.7.0 to v0.7.3 serialized them (`crates/model/tests/io_tables/*.json`, built field
   for field from those versions' structs): 2 inputs and only output paths (#759 case 1), the
   defaults and a user path, no `live`, a JACK group that is now only the input, and a native
   WASAPI record. `a_v072_table_with_inputs_and_no_input_paths_gets_the_defaults_once` fails
   without the fix (ADR 0015, amendment for #431, rule 11: an older table's input half has no
   `defaults_for`, so the defaults are made once). Each fixture is also saved and reloaded and its
   input half compared byte for byte with what was read.
5. **Gate class.** A change to how saved per-machine settings are read or repaired is tested over
   fixtures in the serialized form every older build in support wrote, including the forms a known
   defect wrote, never only over values today's constructors make.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`
   (By area, saved settings). Owner: the maintainer. Blind spots: the fixtures are written from the
   old structs by hand, not produced by running the old builds; a field an old build wrote through
   a path no struct shows (a merge of raw `ui.json` values) is covered only by the app's own merge
   tests.
