<!-- transit-index
title: Plug-in state jobs (Save As, Copy, preset load, restore after undo) wait for the rendered document (`Player::plays`) with a 10 s give-up and a slot-generation check (E47 gate class); user presets are the formats' own containers (`.vstpreset`, `.clappreset`) under `<settings>/presets/<plug-in id>/`, kept in step with the session by `plugin.preset_*` and `plugin.state_*` commands
-->
# ADR 0025: Plug-in state jobs wait for the rendered document; presets are the formats' own containers

- **Status:** accepted 2026-10-24 (retroactive; decided 2026-10-23 in #68 and #73, escape E47)
- **Date:** 2026-10-24
- **Reopen when:** processed delivery can be confirmed without waiting for the current document; continuous edits (automation writes) starve header jobs to the give-up often enough that waiting for an unchanged document is the wrong rule; factory presets (VST3 program lists, CLAP preset discovery) need a container of their own; or a user preset needs to carry more than the plug-in's state.

## Context

#68 gave a hosted VST3 or CLAP plug-in's editor window a header strip with user presets (save, load, previous, next), bypass, copy and paste of state, and loads that are undoable. A user preset is the plug-in's full state, the same state a session stores (`docs/contributing/knowledge/plugin-presets.md`). Reading that state, and loading one into the live instance, are jobs on the plug-in host thread ([ADR 0018](0018-plugin-host-thread.md)) that the UI starts and polls once per frame.

The jobs raced the audio side (#73, escape E47). Parameter values reach a plug-in only when the audio side adopts and renders the next plan, while the jobs read live state immediately. A Save As right after a parameter change saved the old value, and a value in flight could overwrite a just-loaded preset. The regression test failed on Linux and macOS CI for three pushes, deterministically with the inline host, about 1 in 7 on the host-thread path, and once on Windows under load, where it was dismissed as a flake (`docs/escapes/E47-header-state-jobs-raced-the-rendered-document.md`).

## Decision

1. **Any UI job that reads or loads live plug-in state after a document change waits for its own instance's delivery (`Player::plays_instance_processed(&session, track, slot)`, #219).** `plays` remains the rendered-document confirmation. The confirming form requires only the job's slot to have visited the current plan's parameter pass and consumed its queued values (`Plugin::params_pending`, #78). The callback publishes a per-slot acknowledgment in a bank owned by the exact plan generation, with no wait or allocation. A strip still owned by a worker cannot acknowledge its own slots; other strips can confirm independently.
   - Header jobs (Save As, Copy, preset load) and live restores after undo/redo start in a *waiting* step until their own instance confirms. An unrelated deactivated insert, inactive track, failed slot or bypassed Texture never gates them.
   - **Own slot:** a bypassed slot with undelivered edits waits for reactivation and uses the existing "still updating" give-up. An inactive insert or track confirms its stored values once the document is rendered: no new parameter pass is owed while inactive. Reads still go to the live instance, which survives deactivation and holds every edit made while active; with no live instance (not made yet, failed, or a moved insert before its plan is adopted) they refuse, because a stored state that cannot be proven current is never saved; live loads/restores use the host. A failed slot refuses the job with the existing invalid-audio failure status, even if inactive.
   - **Idle parameter rule:** `Plugin::idle_applies_all_parameters` defaults to true. Bypassed active inserts receive all block-start parameters, coalesced for their first active block. A plug-in that must preserve captured state overrides it to false; its idle service applies only Freeze. Texture does so because buffer/quality edits destroy captured audio. Its normal active pass applies the deferred values before rendering. The rule is a trait capability, not a plug-in name check.
   - **Give-up:** a job still waiting after 10 s (`PLAYS_WITHIN`) is dropped with a status line and stores nothing. A restore says "Plug-in state not restored: the audio session did not apply in time; undo again or reload the preset"; other jobs say "The audio session is still updating; try again after it applies". This is the known limit under continuous document edits, because `plays` needs the document to stay unchanged for a rendered block.
   - **Slot-generation check:** every job carries the `SlotGen` (`crates/mix/src/plan.rs`) of the instance it started on, the identity of a plug-in slot across plans. Before the job begins and when it finishes, the UI checks that the slot still holds that instance (`still_holds`). If the plug-in in that slot changed in between, the result is dropped and the status reads "The plug-in in that slot changed while its preset loaded; nothing was stored". A waiting restore whose slot changed is dropped quietly (logged at debug).
   - **Tests** that change a live state directly also store the values the plug-in reports (`plugin.state_values`), or the session sends its older values over the state. The helpers use real readiness: `plugin.state_values` plus a wait for the rendered document. `a_save_as_right_after_a_parameter_change_saves_the_changed_value` fails with the wait removed, on both host paths.
2. **This is a gate class (E47).** A failing test is never passed as a flake without a tracked issue and a green re-run on the failing platform, and the CI conclusion is checked after every push. The rule is stated in `docs/contributing/evidence/gates.md`.
3. **A user preset is the format's own container, so other hosts can read it.**
   - **VST3:** `.vstpreset`, Steinberg's container with component and controller state (`soundcraft_vst3_host::preset`).
   - **CLAP:** `.clappreset`, the project's documented container (`soundcraft_clap_host::preset`).
   - **Where:** `<settings>/presets/<plug-in id made file-safe>/`, one folder per plug-in type, beside the built-in devices' presets (`soundcraft_engine::presets`). File access goes through `soundcraft_engine::io::presets`, which checks its roots (`crates/engine/src/cmd/plugin_presets.rs`).
   - **In the session:** the stored state and the preset name live on the insert. Presets are per machine settings, not session data; a session keeps the state it was saved with.
4. **Commands (`crates/engine/src/cmd/plugin_presets.rs`)** keep the session in step with what the UI did live:
   - `plugin.preset_save`: writes the state the caller read live as the preset file, stores it on the insert and names the preset. Not undoable, not journaled (states can be large).
   - `plugin.preset_load` and `plugin.state_load`: one undo step each, recorded after the plug-in took the state (the stored state, the preset name and the stored parameter values the plug-in reported after loading). `state_load` takes a state in hand (a pasted state, the plug-in's defaults).
   - `plugin.state_values`: stores the values a plug-in reports after its stored state was loaded into it again, so the session never pushes older values over it. Not undoable.
   - `plugin.preset_list`, `plugin.preset_delete` (`.vstpreset` and `.clappreset` only; not undoable).
   - Undoing or redoing a load asks the UI to load the stored state live (`Engine::take_state_restores`), through the same gated job.

## Consequences

- A preset saved from the header holds the value the user just set, and a load cannot be overwritten by an older value in flight.
- A header action can be slow right after a document change, and can give up under continuous automation writes. The user is told and nothing is stored, rather than a stale state being saved.
- `.vstpreset` files load in other VST3 hosts.
- A bypassed insert's queued edits wait for reactivation; a state job waits with them instead of saving stale state (#219).
- Known gaps recorded in #73: a preset load that finishes after the UI gave up on a not-responding host thread leaves the live plug-in and the session disagreeing until the next save; factory presets are not listed; macOS gets no header strip.

## Alternatives rejected

- **Read live state immediately.** What shipped first, and what failed (E47). The local Windows and Linux gates passed it (timing).
- **Make the helper change plug-in state behind the session's back in tests.** It hid the product race (E47 item 2); the helpers now use the same readiness as the app.

## Related

[ADR 0018](0018-plugin-host-thread.md) (host thread), `docs/contributing/knowledge/plugin-presets.md`, `docs/escapes/E47-header-state-jobs-raced-the-rendered-document.md`, issues #68, #73, #78, #219.
