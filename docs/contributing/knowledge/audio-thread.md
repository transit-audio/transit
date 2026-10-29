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

## Live-input ring alignment (K22, #88)

The live input reaches the plan through `InputRing` (`crates/playback/src/record.rs`). The input
callback pushes, and the plan pops in `plan_bridge::Audio::render`. The ring is a FIFO, so
anything that queues while nothing reads would stay in front of the reader as a fixed delay. That
happens between `open_input` and the plan's adoption, and during a stall. The contract:

- **When it aligns.** On the first read after a plan adopts a ring that the displaced plan did not
  read (`Audio::service` calls `InputRing::align_on_next_read`), and on the first read after any
  `clear_monitor`. The aligning read must find input. A read before the device's input runs
  leaves the request pending. A new plan with the same ring does not realign it, so an edit never
  moves the input.
- **What it keeps.** The read's own input frames plus a slack. After that the reader runs in
  lockstep, and the backlog B after the read equals the slack. The slack covers the capture
  period only while it is at most `read + slack`; the producer records its last pushed block size
  in one Relaxed atomic, and the unpaired rule uses it:
  - `ALIGNED_BACKLOG_BLOCKS_PAIRED` (0) when one device callback delivers the capture and then
    asks for the render (`Backend::capture_precedes_render`: native ASIO, native JACK and the duplex fake);
  - `ALIGNED_BACKLOG_BLOCKS_UNPAIRED` (`max(read, last_push)` frames: the larger of one read
    block and the last block the capture callback pushed) when capture and render run on separate
    callbacks (WASAPI's two loops, cpal), whose order can swap within a period. A 480-frame
    capture against a 128-frame render keeps 128 + 480 frames, so the aligned start does not
    underrun. The no-underrun claim holds while the capture period is at most `read + max(read,
    last_push)`; a capture period that changes after alignment (a stall, #83) is not covered.
- **Real-time rules.** The discard is one queue commit (a pointer move), made under the reader's
  `try_lock` that the read already holds, and it is counted in `discarded_frames`. The request is
  one atomic store, which is safe from the audio thread at adoption.
- **What it does not do.** Outside alignment the reader never trims the FIFO. A stall after
  alignment leaves its backlog until the next clear, and an External Instrument Measure refuses a
  run whose B changed. `buffered_input_frames` read after the plan's pop is exact (the slack)
  only when paired; unpaired, the capture callback can push between the read and the snapshot, so
  it swings by up to a capture period. A consumer compares means over many reads (#98, E54). Recording does not read this ring (it captures by device frame), and the
  loopback calibration counts device positions.
- **Proof.** The tests pre-fill the ring before the plan reads it:
  - `record::ring_tests`: `the_aligning_read_drops_*`, `a_clear_mid_session_*` and
    `alignment_waits_*`;
  - `plan_bridge::tests::an_adopted_input_reads_the_newest_capture_*`;
  - the fake-device Player tests `input_monitoring_latency_excludes_*` and
    `a_hardware_insert_round_trip_excludes_*` (`player/device/tests.rs`).

## Plug-in host thread

Hosted plug-ins (CLAP, VST3) live on the plug-in host thread ([ADR 0018](../../adr/0018-plugin-host-thread.md),
`crates/plugin-host`), not on the UI thread:

- **Creation:** `playback/src/player/handles.rs` announces an instance for a frame ("Loading…"),
  submits a job that runs `transit_mix::create_instance` (load, `initialize`, state restore,
  `prepare`) on the host thread, and polls the result once per frame. The compile takes the
  finished `NewInstance`; the plan is published and adopted exactly as before. Nothing on the
  audio side changes: no new lock, no allocation, no wait. An instance the compile asks for that
  staging did not predict (a slot whose layout changed) is submitted too: that plan leaves the
  slot vacant ("Loading…"), and when the instance is ready the same document is compiled again
  with it (`PlanCompiler::retry_vacant`, `Handles::retry`).
- **Load and open timing (issue #67):** a load passes four phases, published while it runs
  (`LoadPhase`, `PhaseLog` in `crates/plugin-host/src/phases.rs`): waiting for the host thread,
  creating the instance (module load, parameter table, instance), restoring state (the factory
  marks it with `report_phase`; `transit_mix::create_instance_with`), binding to the plan
  (made, waiting for the compile). Marks take their time from an injected clock (`Config::clock`
  for editors, the handles' clock for loads), so tests drive them with a manual clock and never
  sleep. The slot tooltip reads "Loading X… 12.3 s (creating instance)"; when the compile takes
  the instance `Handles::take_loaded` hands the phases and total to the UI, which logs one INFO
  line per phase plus a total (control thread, never the audio thread), keeps the total per plug-in
  id in `UiState::plugin_load_ms` (shown as "last time 14 s" beside "Loading…") and says "X loaded
  in 14.1 s" in the status line when it took over 2 s. An editor open is timed the same way
  (`PluginEditor::opening_for`, `take_open_report`): the button reads "Opening… 3 s" and the
  status line "X editor opened in 3.2 s" over 2 s. The format cannot split the parameter-table
  read from instance creation, so they are one phase, and the editor's window creation and plug-in
  attach happen inside one `open` call.
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

- **`cached_info(id)`** (`transit_clap_host`, `transit_vst3_host`; used by `device_info` in
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
lock once at construction on the control thread (see `InputRing::with_pairing`), use `const`-initialized
thread-locals of types without `Drop`, and keep destructor thread-locals off the audio path.

The VST3 host pre-locks both of its process-path mutexes this way: `EventList::new` (in
`crates/vst3-host/src/ffi/events.rs`) and `HostContext.edits` (in `crates/vst3-host/src/ffi/host_context.rs`;
plug-ins call `performEdit` from the audio thread despite the spec).

## Worker threads

Render workers are promoted once at thread start (MMCSS, SCHED_FIFO or rtkit, time-constraint
policy) inside `crates/rt` (`worker_set`). Never register with the OS inside a timed callback.

## Render workers (#124, ADR 0029)

The default since 2026-10-29: `FORCE_SERIAL` starts clear, and playback's `plan_bridge::pair` calls
`MixEngine::start_render_workers`. `TRANSIT_RENDER_POOL=0` (the opt-out, parsed by
`render_pool::render_pool_setting`, read once in `init_render_switch`) sets `FORCE_SERIAL`; then no
workers start and `render_active` renders phase 1 serially exactly as before; offline renders never use the workers (they keep Rayon's global pool). The rayon render pool
is gone.

- **Path.** `strip_pool::StripPool::render`: `begin_block`, bring home strips whose worker is done,
  deliver each strip's deferred MIDI, send it into its cell and `publish`, `wake`, render the
  callback's own strips (live input, hardware), `finish` by the deadline (three quarters of the
  period from `render_active`'s entry), bring home and mix only `Mixed` and `Inline`, defer the MIDI
  of `Busy` strips. Taken only on the audio thread, without sidechain keys, with at least
  `PARALLEL_STRIPS` phase-1 strips, or while a strip is still away.
- **Cells, not `unsafe`.** A strip's state moves into a `Mutex<Job>` cell for the render. Every
  access is `try_lock`: the slot protocol gives the strip to one thread, so a refused borrow is a
  counted fault (`StripFaults::conflicts`), never a wait. The table keeps a spare strip with the
  routing state (delay lines, edges, sink routes, meter, latency) so the delay walk and the meters
  work while the strip is away.
- **Away strips.** A late strip stays in its cell across blocks. Everything that touches an
  instrument between blocks must handle that: `queue_event` parks the event (tagged with its
  identity) for the strip's next block; seek, reset, All Notes Off, transport and revocation set a
  pending flag applied when it is back; Stop drops parked events. A plan is adopted only when every
  strip is home (`render_workers_settled`, `RejectReason::StripsInFlight`). A new engine method that
  walks `tables.strips` and touches an instrument must do the same.
- **Retirement.** `MixEngine::into_retired` (control thread) shuts the workers down with a bound and
  brings every strip home; a worker that does not return is quarantined and its strip leaked. Never
  drop an engine with workers on the audio thread.
- **Proof.** `crates/mix/tests/render_workers.rs` (continuity through the worker set with stalls and
  live MIDI, zero allocations on the callback, adoption while away, stop while away, ring overflow,
  quarantine) and `render_pool.rs`'s bit-identity test. The measurement is
  `crates/mix/tests/render_headroom.rs` (ignored, release).

## Proving it

Zero-allocation tests: `crates/mix/tests/plan_alloc_probe.rs`, `alloc_probe.rs`,
`event_alloc_probe.rs`, using `measure` from `crates/rt/src/testing.rs`. A change that touches the
callback path extends these; see `../evidence/gates.md`.
