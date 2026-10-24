# Audio-thread contract

The audio callback (and anything it calls) must never:

- allocate or free (including growing a `Vec`, formatting a string, or dropping a `Box`);
- take a lock, wait on a channel, or block in any other way;
- drop a plug-in instance, a plan, or anything whose destructor may run foreign code;
- panic.

Panics are contained per strip (`crates/mix/tests/strip_panics.rs`); containment is a last resort,
not permission.

## How data reaches the callback

- **Primitives:** `crates/rt` (`handoff`): `Anchor<T>` lives on the control side and owns the value;
  `AudioAccess<T>` is the callback's handle. Retired values are returned to the control side to be
  dropped there. See `crates/rt/src/handoff.rs`.
- **Plans:** the control side compiles a `PreparedPlan` (`crates/mix/src/plan.rs`), all storage
  preallocated; it is published through the queue in `crates/playback/src/publication.rs` and
  adopted by the callback at a block boundary (`crates/playback/src/plan_bridge.rs`). The old plan
  goes back through the same channel for disposal off-thread.
- **Actions** (transport, MIDI, record start) go through the action lane
  (`crates/playback/src/action_lane.rs`), a bounded preallocated queue.
- **Live Cycle** (#81): the host sends `Player::set_loop` whenever the range Play would loop
  (`StudioApp::wanted_loop`) changes while playing or a Play is pending, and the callback adopts it
  at the next block without moving the playhead: before the new end it wraps there, past it it plays
  on, `None` plays through. The wrap is the same one a loop set at Play renders (new transport epoch,
  external MIDI release and chase). It is not tied to a session snapshot, since the Cycle edit's own
  snapshot supersedes the one in force when it was sent.
- **Plug-ins** are created, scanned and destroyed off the audio thread; only `process` runs on it.
  Hosted ones are created and destroyed on the plug-in host thread (see below).
- Logging from the audio thread uses `on_audio_thread()` and defers the write to the UI thread.

## Plug-in host thread

Hosted plug-ins (CLAP, VST3) live on the plug-in host thread ([ADR 0018](../../adr/0018-plugin-host-thread.md),
`crates/plugin-host`), not on the UI thread:

- **Creation:** `playback/src/player/handles.rs` announces an instance for a frame ("Loading…"),
  submits a job that runs `soundcraft_mix::create_instance` (load, `initialize`, state restore,
  `prepare`) on the host thread, and polls the result once per frame. The compile takes the
  finished `NewInstance`; the plan is published and adopted exactly as before. Nothing on the
  audio side changes: no new lock, no allocation, no wait. An instance the compile asks for that
  staging did not predict (a slot whose layout changed) is submitted too: that plan leaves the
  slot vacant ("Loading…"), and when the instance is ready the same document is compiled again
  with it (`PlanCompiler::retry_vacant`, `Handles::retry`).
- **The forwarder:** the host thread wraps each instance (`Ctx::adopt`) in a `Hosted` forwarder.
  The audio thread calls `process` and the rest through it (one extra virtual call, no
  allocation: `crates/plugin-host/tests/audio_alloc.rs`).
- **Destruction:** dropping the forwarder (on the control thread, when a plan retires it) sends
  the instance to its host thread, which destroys it there; if that thread is gone it is leaked.
- **Editors and states:** the editor and state reader stay on the host thread. The UI holds
  proxies: open, close and parameter echoes are sent without waiting; edits and the window state
  come back through shared state the host thread's 15 ms tick fills. A save (Save, autosave,
  bounce, device reopen) waits for each plug-in that is answering, however slowly (up to the
  10 s timeout). A wedged thread, or one inside a window message for more than 10 s, answers at
  once; one held by a modal dialog (a window message past 100 ms, `MODAL_AFTER`) is waited for
  at most 500 ms per save in all (`DEFAULT_SAVE_DEADLINE`), except for a plug-in the session has
  no state for yet, which is waited for as usual. A plug-in left out keeps its stored state and
  the status line names it.
- **Stuck plug-ins:** a job or tick that runs past 10 s, or a window message (a modal dialog) past
  120 s, wedges its thread and a fresh thread takes new creations. Only a creation that was stuck
  marks its slot "Not responding" (and compiles it empty). Instances that already lived on the
  wedged thread keep playing and their slots look normal; their editors and state reads answer
  "not responding" until the thread returns, and then work again. The status line says when a
  plug-in is not responding and when it has returned.
- **Main-thread rules:** the host thread is the instances' main thread (VST3 controller and
  views, CLAP `[main-thread]` calls, `request_callback`, `restartComponent` servicing) and pumps
  window messages (`transit_rt::gui_thread`); VST3 modules are entered and exited on it.
- **Shutdown:** the host thread drops its job queue before its message queue (the COM apartment)
  ends; a job sent later is leaked by its sender, never run.
- **The stuck check** names the busy section it found overdue (a sequence number), so a call that
  returns while the check marks it never leaves an idle thread marked wedged.
- **Descriptions:** a hosted type's parameter table is read when its first instance is made and
  built outside the registry lock. A command that needs the table of a type never made
  (`mix.insert_param`) reads it on the host thread and waits at most 500 ms; past that it is
  refused as still loading and the read carries on. Adding a plug-in with parameter values
  reads it there too, waiting up to the 10 s request timeout.
- **VST3 modules** are entered outside the loaded-module lock: a hung module entry blocks later
  loads of that binary only (K38, closed).
- **macOS** runs these jobs inline on the UI thread (AppKit), as before ADR 0018.

Offline renders (bounce, stems, freeze) make, process and destroy their instances on the thread
that builds the render, not on the host thread; ADR 0018 says why.

## Parameter tables read on the UI thread

The automation picker and the plug-in parameter lists name a hosted plug-in's parameters from the
host registry's cache, not from an instance (#58, #65).

- **`cached_info(id)`** (`soundcraft_clap_host`, `soundcraft_vst3_host`; used by `device_info` in
  `crates/ui-studio/src/automation.rs`) returns the description of a type already loaded, without
  instantiating anything and without loading a binary, so it is safe on the UI thread. It returns
  `None` for a type not loaded yet; the picker then shows the stored parameters as `param <id>`
  until a first instance has loaded. A project's plug-ins load on open, so the table is normally
  there. The table is built once per type, outside the registry lock, and the first insert wins.
- **A never-loaded type** is read on the plug-in host thread by commands that need it
  (`hosted_info` in `crates/engine/src/cmd/clap.rs`), which waits at most `INFO_WAIT` (500 ms) and
  then refuses as still loading while the read carries on; the UI never waits on it (#56).
- **`ParamInfo.group`** (`crates/dsp/src/lib.rs`) is the module (CLAP) or unit (VST3) path the
  parameter sits under, parts joined by `/`; empty for built-in devices and plug-ins without
  groups. The picker groups by it (#65). Parameter ids keep their case.
- **The picker's model cache** (`PickerState::model`, `crates/ui-studio/src/automation_picker.rs`)
  builds the grouped model once per cached table and keys it by the table's address. The host
  caches a table once per plug-in type, so a new table is the only invalidation.
- Not covered: AU plug-ins on macOS.

## First use allocates on macOS

On macOS, `std::sync::Mutex` is a lazily boxed `pthread_mutex_t`: the first `lock` or `try_lock`
allocates 64 bytes. A `thread_local!` whose type has a destructor allocates on first access per
thread (Apple registers it through `_tlv_atexit`), and Apple's libunwind allocates on the first
unwind. Windows and Linux do none of these, so a test can pass there and fail on macOS. Take each
lock once at construction on the control thread (see `InputRing::new`), use `const`-initialized
thread-locals of types without `Drop`, and keep destructor thread-locals off the audio path.

The VST3 host pre-locks both of its process-path mutexes this way: `EventList::new` and
`HostContext.edits` (plug-ins call `performEdit` from the audio thread despite the spec) in
`crates/vst3-host/src/ffi.rs`.

## Worker threads

Pool workers are promoted at thread start (MMCSS, SCHED_FIFO or rtkit, time-constraint policy) by
`crates/rt`. Never register with the OS inside a timed callback.

## Proving it

Zero-allocation tests: `crates/mix/tests/plan_alloc_probe.rs`, `alloc_probe.rs`,
`event_alloc_probe.rs`, using `measure` from `crates/rt/src/testing.rs`. A change that touches the
callback path extends these; see `../evidence/gates.md`.
