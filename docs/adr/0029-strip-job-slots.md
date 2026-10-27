# ADR 0029: Per-strip job slots for the multicore strip render

- **Status:** proposed 2026-10-27 (issue #124; slice 1, the slot protocol, implemented in `transit-rt` `strip_jobs`; slice 2, the worker set and the deadline policy, in `transit-rt` `worker_set`; slice 3, the deferred-MIDI ring and the continuity test instrument, in `transit-rt` `midi_defer`)
- **Date:** 2026-10-27
- **Amends:** [ADR 0012](0012-midi-device-layer.md) gate G1 and [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md) (the lock-free fan-out it names as a `transit-rt` residual)
- **Reopen when:** the slot needs a field written outside its transitions, a second publisher appears, or a later slice cannot keep the callback's bound with this state machine.

## Context

The render pool (`crates/mix/src/render_pool.rs`) is rayon: its injector allocates every ~63 pushes, the callback waits on a latch, and a starved worker makes a late block. Serial is the realtime default on that evidence. #124 replaces it with a fixed worker set owned by `transit-rt`, and froze six correctness gates on 2026-10-27 (issue body, "Correctness gates", and the design notes of 2026-10-26 and 2026-10-27). This ADR records the ownership protocol those gates rest on.

## Decision

Each strip has one `JobSlot`: an `AtomicU8` state and an `AtomicU64` generation (the block it was published for), plus a fault counter written only on a refused transition. A `JobSet` is a `Box<[JobSlot]>` of fixed capacity (the most strips), built once off the audio path; nothing after construction allocates. The callback is the only publisher; workers only claim and complete.

| From | Operation | To | Who | Ordering on success | Refused (counted fault) when |
|---|---|---|---|---|---|
| `Idle` | `publish(g)` | `Published` | callback | Release (generation and inputs visible to the claimer) | not `Idle`; `g` not newer than the slot's last |
| `Published` | `try_claim()` | `Claimed` + `ClaimToken` | worker | CAS, Acquire | never; a lost race is `None` |
| `Published` | `reclaim()` | `Reclaimed` | callback | CAS, Acquire | never; a lost race is `false` |
| `Claimed` | `complete(token)` | `Done` | the claiming worker | CAS, Release (the worker's writes) | not `Claimed`; a token passed to another slot (its own slot still completes) |
| `Done`, generation `g` | `consume(g)` returns `Ready` | `Idle` | callback | Acquire | `Done` with a generation newer than `g` |
| `Done`, older generation | `consume(g)` returns `Stale` | unchanged | callback | Acquire | |
| `Done`, `Reclaimed` | `retire()` | `Idle` | callback | Acquire (a late worker's writes before buffer reuse) | from `Idle`, `Published` or `Claimed` |

**Gate 1 invariant.** Per publish at most one of claim and reclaim succeeds: both are a CAS from `Published`, and `Published` is re-entered only by a publish, which needs `Idle`, which only the callback reaches, from `Done` or `Reclaimed`. The token is the only way to complete, so a worker that lost the claim cannot.

**Per-block rule** (#124, follow-up 2). At each block the callback reads every slot first: `Idle`, publish; `Done`, retire unread (a late result) and publish; `Claimed`, skip the strip and count an overrun. At the deadline it reclaims the unstarted (within the admission limit) and renders them inline, consumes what is `Done` for this block, and counts what is still `Claimed` as an overrun.

**Policies decided in #124, recorded here.** Overrun, admission and retirement are implemented by slice 2 (`worker_set`); deferred MIDI by slice 3 (`midi_defer`, rules below); the default is slice 4.

| Policy | Rule |
|---|---|
| Overrun | A claimed strip is never canceled, re-run or rendered inline. The callback waits for `Done` no later than its deadline; past it the strip is silence for the block, a counted stream fault, and the slot is republished only after `Done` is observed and retired. Holding the block instead (a late block, no gap) becomes an option only if the bench shows it is ever better. |
| Admission | The callback reclaims only as many unstarted strips as fit in the time left, by each strip's measured cost; the rest are overruns. A starved pool degrades to serial with gaps, never to a late block. |
| Deferred MIDI | Events of a skipped block carry forward to the next processed block at offset 0, in order, in a fixed-capacity per-strip ring; a collapsed on/off pair gets the off one sample later; on overflow note-ons and ordinary controllers go first, the release class (note-off, sustain-off, All Notes Off, All Sound Off) stays, and past that the queue becomes All Sound Off + All Notes Off. Overflows are counted; recording is unaffected (capture at ingress and device) with an overrun marker in the take journal. |
| Retirement | Engine retirement gives each outstanding slot a bounded wait, then quarantines: the worker thread is abandoned, the instance never freed or reused, the fault logged with the plug-in's name. |
| Default | Serial stays the default unless the bench meets gate 6. |

## Deferred MIDI (slice 3, `midi_defer`)

A `DeferSet` holds one `DeferRing` per strip, each a fixed buffer of the caller's capacity (at least 2), allocated in `DeferSet::new`; nothing after it allocates. The event is a 16-byte `Copy` struct: `status`, `data1`, `data2`, `offset` (the sample offset within its block) and `block` (the generation it belonged to, stamped on defer and kept on delivery).

**One producer, one consumer, one thread.** The audio callback defers after a block and drains at a later block; `defer_block`, `deliver` and `drain_into` take `&mut self`, so the ring needs no atomics and no lock. Only the fault counters (`DeferStats`, shared through an `Arc` cloned off the audio path) are atomics, readable from any thread.

| Step | Rule |
|---|---|
| Who defers | `should_defer(outcome)` is true only for `Overrun(Busy)` and `Overrun(Unstarted)`; `defer_for` checks it. A `Late` strip's worker consumed its block's events; `Faulted` is replaced; `Mixed`, `Inline`, `NotPublished`, `Pending` have nothing to carry. |
| Landing | An event of block `g` at offset `o` lands in block `now` at `o - (now - g) * block_len`, clamped to `0..=block_len - 1` (`landing`). An event of a skipped block is in the past, so it lands at offset 0. |
| Order | Delivery keeps the original order, across several skipped blocks too; offsets never decrease (an event lands no earlier than the one delivered before it). `deliver` delivers the deferred events and then the block's live events under the same rule. |
| Collapsed pair | A note-off (or note-on with velocity 0) that would land on the sample of the note-on of the same channel and key delivered in the same call moves one sample later. |
| End of block | If that would pass `block_len - 1`, the note-off and every event behind it (live events included, deferred with generation `now`) stay in the ring for the next processed block, where they land at offset 0. No zero-length note, no reordering; counted as `carried`. |
| Overflow | When a block's events do not fit, the oldest events outside the release class are dropped (note-ons, ordinary controllers, pitch bend, pressure, program changes), the new block's own only after every older one; the release class stays: note-off, note-on velocity 0, CC 64 below 64 (sustain off), CC 66 below 64 (sostenuto off), CC 120, CC 123. One pass over the ring and the new events. |
| Silence | If the release class alone does not fit, the ring's content is discarded and the strip is silenced on every channel the discarded events used plus the strip's own (`set_channel`, channel 1 by default): a 16-bit channel mask, no stored events. Events deferred while silenced are dropped and their channels join the mask. The next drain emits, per channel in the mask (lowest first), CC 120 All Sound Off, CC 123 All Notes Off and CC 64 = 0 at offset 0 (CC 120 does not lift a held sustain pedal on a real instrument), then the block's live events, and clears the mask. Nothing allocates. |
| Counters | Per strip and for the set: `deferred`, `dropped` (every event the overflow rule discarded, the silence's included), `silences`, `carried`; `overflow_faults` = dropped + silences; `refused` for a strip out of range. |

The continuity test instrument (`midi_defer::testing::ContinuityInstrument`, feature `testing`) processes blocks given their transport position, records holes (transport time never processed), overlaps, every event with the sample it landed on, out-of-block and unsorted offsets, orphan note-offs, zero-length notes and restrikes, and reports stuck notes and pedals. Its trace and hole lists are preallocated, so it can sit on an allocation-armed path in slice 4.

## Preconditions for callers

The slot enforces its transitions; these rules are the caller's, and the executor's refutation review checks them.

| Rule | Detail |
|---|---|
| Single publisher | `publish`, `reclaim`, `consume`, `retire` and `JobSet::begin` only from the audio callback; `state`, `counts`, `faults` and `for_each_pending` from anywhere. Workers only `try_claim`/`try_claim_next` and complete. |
| Buffer ownership follows the slot state, by convention | Touch a strip's buffers and instance only while holding its `ClaimToken`, after a successful `reclaim`, or after `consume` returned `Ready` (until the next publish). Never on `state() == Done` alone: a `Done` result is read only through `consume`. |
| Faults are failures | `faults()` counts refused transitions in every build; tests outside `transit-rt` assert `faults() == 0` (inside it, a refusal already fails the test). |
| One `WorkerSet`, owned by the callback | Its block methods (`begin_block`, `publish`, `wake`, `finish`) take `&mut self`, which makes the single publisher a compile-time rule; `stats`, `outcomes` and `jobs` are reads. |
| Inputs only when not busy, after `begin_block` | Write a strip's inputs only after this block's `begin_block`, before its `publish`, and only when `is_busy(strip)` is false: a busy strip's buffers belong to a worker until a later `begin_block` retires its result. |
| Mix only `Mixed` and `Inline`, in the window | After `finish`, a strip is mixed only when its `Outcome` is `Mixed` or `Inline`; `Overrun` is silence; `Faulted` is silence. Read a strip's outputs only between `finish` and that strip's next `publish`, and read the outcomes before the next `begin_block` (which resets them). |
| Never republish a faulted instance | A strip whose runner panicked (a `Faulted` outcome, or a late result that panicked, seen at a later `begin_block`) is suspect: `publish` refuses it (`PublishRefused::Suspect`) until the caller has replaced the instance and called `clear_suspect`. |
| The inline closure must not panic | An unwind through `finish` leaves the block unresolved; the caller catches inside the closure (as `guarded` does in `crates/mix`). |
| Never drop a `WorkerSet` on the audio thread | `Drop` runs `shutdown`, which sleeps while it waits (up to 1 s); retire the set through the garbage path. |
| Defer only what the strip missed; deliver before its next render | Defer with `defer_for` (or `defer_block` after `should_defer`) after `finish`, with the events the block would have delivered; at the strip's next processed block call `deliver` with that block's live events before its `publish` (or `drain_into`, then live events no earlier than `Drained::floor`, and none while events are carried). |
| Never reuse a quarantined instance | A strip named in a `Quarantine` report keeps its instance and buffers alive and unused for the life of the process. |

## Correctness gates (#124) and what slices 1, 2 and 3 prove

| Gate | Requirement | Slice 1 (`strip_jobs`) | Slice 2 (`worker_set`) | Slice 3 (`midi_defer`) | Later slices must prove |
|---|---|---|---|---|---|
| 1 Exclusive execution | one executor per publish | **Proven for the slot:** threaded claim/reclaim race with injected spins and yields; free-running per-block test with a reentry guard; exhaustive operation sequences (depth 8, two actors) against the table | **Proven for the worker set:** real parked workers and the inline path under one reentry guard and a strictly growing generation check per strip; free-running, 1500 blocks in each of five configurations (1, 2, 3, 3 and 5 workers) with random spins, strips slower than a block and stalls released up to 20 blocks later; the payload hand-off checked in both directions through the workers | not affected (the ring is the callback's own state) | slice 4: the real strip processor under the same guard; the payload orderings on an ARM runner or under loom |
| 2 No incomplete or stale audio | only `Done` of this generation is mixed | **Proven for the slot:** `Ready` only for `Done` of this generation and only once; `Stale` retired unread; exhaustive per-block interleavings over three blocks | **Proven for the set:** `Mixed` only from `consume` returning `Ready`, cross-checked against the generation the executor ran; `Unstarted` and `Busy` strips never executed for the block; late results retired unread at the next `begin_block` (`late_results`) | not affected | slice 4: the mixdown reads a strip only on `Mixed` or `Inline` |
| 3 Bounded waiting and storage | no wait past the deadline; admission; event ring | groundwork only: every operation is wait-free and allocation-free (allocation-armed test) | **Proven:** `finish` never waits on a `Claimed` slot past its deadline (one stalled strip of eight: it returns after the deadline and less than 200 ms past it, the strip is `Overrun(Late)`, skipped next block as `Overrun(Busy)`, and resumes after release); admission from a per-strip cost EWMA (inline renders at most budget / cost, a strip that cannot fit never started, overshoot at most one strip's cost; with time to spare every unstarted strip is rendered inline); zero allocations on `begin_block`, `publish`, `wake` and `finish` and in the worker loop between runner calls, and no allocation retiring an idle set | **Proven for the ring:** fixed capacity per strip from construction; every defer and drain bounded by capacity plus the events passed; zero allocations on defer (empty, past capacity with the drop pass, silence, while silenced) and on drain and deliver (empty, full, silenced, with the end-of-block carry) under the allocation-armed helper | slice 4: the starved gate and block-time percentiles on the bench |
| 4 MIDI and state discontinuities | carry-forward and overflow rules | none | none (the set reports `Overrun(Busy)`, the hook slice 3 needs) | **Proven for the ring, driven directly:** order and offsets from one and two skipped blocks (offset 0, original order across blocks); the clamp at `block_len - 1` and non-decreasing offsets; the collapsed pair split by one sample (per channel and key, across blocks, across deferred and live); the on-at-last-sample off carried to the next block with the events behind it; overflow drops the non-release class oldest first and keeps the release class; past that the silence covers every channel the discarded and the dropped-while-silenced events used plus the strip's own, each with All Sound Off, All Notes Off and sustain off at offset 0, the flag set and cleared by the drain, counters exact (`carried` equal to what the ring kept); the release class, CC 66 off included; `should_defer` true only for `Busy` and `Unstarted`; 300 random runs through the continuity instrument with skipped runs of 1 to 4 blocks: zero stuck notes and pedals, every off strictly after its on, every event exactly once in played order, one hole per skipped block; 300 overflowing runs (capacity 2 to 8, three channels, sustain pedal presses): zero stuck notes and pedals, some silences covering several channels, delivered a subsequence of played, delivered + dropped = played + the events the silences emitted; both property tests also pass on seeds 5000 to 6500 (`TRANSIT_MIDI_DEFER_SEEDS`) | slice 4: the wiring in `crates/mix` (defer on `Busy` and `Unstarted`, `deliver` before the strip's next publish, the conversion to and from `PluginEvent` keeping voice ids), the overflow counters as stream faults next to overruns, the take-journal overrun marker (recording unaffected), and the continuity instrument driven through a real worker set with injected stalls; on the bench, a sustained synth note through a forced overrun resumes without a stuck note |
| 5 Retirement | bounded wait, quarantine | none (a dropped token leaves the slot `Claimed`, which quarantine will detect) | **Proven:** `shutdown(bound)` returns an idle set at once; with a stalled worker it returns within the bound plus slack, reports the worker and its strip and leaves the slot `Claimed`, and the released worker later finishes into leaked state without harm; a runner panic is caught, the slot completed and the strip reported `Faulted` | not affected | slice 4: the engine retires through it and logs the plug-in's name; a quarantined instance is never reused |
| 6 Measured improvement | bench, continuity | none | none | the continuity instrument (holes, stuck notes, event trace), driven directly | slice 4: wiring into `crates/mix` behind the serial default, the #124 test matrix, the measurement, the continuity instrument through a real worker set with injected stalls, and the default decision |

## Notes for slices 3 and 4 (from the slice 2 review)

- Slice 3 (done): defer MIDI only for `Overrun(Busy)` and `Overrun(Unstarted)`; a `Late` strip's worker consumed its events (`should_defer`).
- Slice 4: the mix crate delivers `PluginEvent` (channel, voice id, message), not raw bytes; the conversion at the ring's boundary must keep the voice id (a side table per strip, or an id field in the event) so a carried note-off still ends the voice it started. A silence covers only the channels the ring saw; an instrument holding notes from events that never passed through the ring on other channels is the mix crate's to release (its reset path).
- Slice 4: pass `wake(count)` the number of strips published; read outcomes before the next `begin_block`; retire the set (and quarantined instances) through the garbage path, never on the audio thread; on Linux `SCHED_FIFO` a `yield_now` in `finish` can hand the core to a same-priority worker, so keep the workers at or below cores minus one, or below the callback's priority, and measure it.
- The busy skip in `wake` is ordered against the worker's `current = 0` by a `SeqCst` fence on each side (store-buffering pattern); a slice that changes either side keeps the pair.

## Deviations from #124

| # | #124 says | Implemented | Why |
|---|---|---|---|
| 1 | `retire` is `Done -> Idle` | `consume` returning `Ready` is itself the `Done -> Idle` transition; `retire` covers an unread `Done` and `Reclaimed` | "ready once" without a field written outside the transitions |
| 2 | a stale `Done` is a counted fault | a stale `Done` is the normal overrun path (`Stale`, retired unread); the fault is a `Done` newer than the block asked for | a strip skipped at block N+1 whose worker finishes before the deadline read is `Done(N)` at N+1 by design |
| 3 | a debug assertion in tests | a refused transition asserts only in `transit-rt`'s own tests (`cfg(test)`); everywhere else, debug builds included, it is counted | no panic on the audio thread of a debug build |
| 4 | the callback waits for `Done` up to the deadline (no wait policy given) | `finish` sweeps the open slots; after a sweep without progress it spins (64 sweeps of 32 `spin_loop` hints), then calls `yield_now` between sweeps; never a lock, a park or a sleep | a spin alone starves a worker that shares the core; a park would need a wake from the worker on every completion |
| 5 | at the deadline, overrun the rest | an unstarted strip the admission limit refused stays claimable by the workers until the deadline; at the deadline it is reclaimed and retired unrendered (`Overrun(Unstarted)`) | a worker that started it after the deadline would hold the strip into the next block and turn one overrun into two |
| 6 | a worker unwinding from a plug-in panic leaves the slot `Claimed`: catch it and quarantine | the worker loop catches the panic, completes the slot and reports the strip `Faulted` (`runner_panics`); the worker lives on; the strip is suspect (also for a panic in a late result) and is not published again until the caller replaces the instance (`clear_suspect`) | the thread is sound after a caught panic; only the instance is suspect, and quarantine is for threads that cannot be joined |
| 7 | at block start a `Claimed` slot is skipped and counted | `begin_block` marks the strip busy; the overrun is counted when the caller publishes it (`Published::Skipped`, `Overrun(Busy)`), and its result is retired only at a later `begin_block`, even if the worker finishes mid-block | a strip the plan no longer renders is not counted; results are never retired mid-block |

| 8 | carried events land "at offset 0" | at offset 0, then non-decreasing in original order: a collapsed off one sample later, everything after it no earlier | a block's events are delivered sorted, and a later event may not overtake an earlier one |
| 9 | (not stated) | an off that cannot be split from its on before the block ends waits for the next processed block with every event behind it, live ones included | no zero-length note and no reordering; reachable only when about `block_len` collapsed pairs pile up in one block |
| 10 | "the queue becomes All Sound Off + All Notes Off" | All Sound Off, All Notes Off and sustain off (CC 64 = 0), generated at the drain for every channel the discarded events used (and those dropped while silenced) plus the strip's own; events deferred while silenced are dropped until the drain | the discarded events may hold note-offs and pedal releases on any channel, so one channel left notes stuck on the others (the slice 3 review reproduced 228 stuck notes over 300 seeds); CC 120 does not lift a held pedal, so the sustain off is sent explicitly |
| 11 | release class: note-off, sustain-off, All Notes Off, All Sound Off | those, plus note-on velocity 0 and sostenuto off (CC 66 below 64); the other pedals (soft, hold 2) are droppable | a dropped sostenuto-off leaves its held notes sounding like a dropped sustain-off; soft and hold 2 release no note |

Additions, not deviations: `publish` checks `Idle` before it writes the generation (the single-publisher precondition makes that race-free) and refuses a generation that is not newer; `retire` from `Idle` or `Published` is refused too.

## Consequences

- No `unsafe` in the protocol; slots hold no buffers. Buffer and instance ownership follows the slot state and is enforced by the caller (the later executor), which is where its refutation review must look.
- `loom` was not added (a new dev-dependency in the unsafe crate is the owner's call). The exhaustive tests interleave whole operations, which is complete for this slot because each operation is one atomic read-modify-write on the state; Slice 2 adds parking, wake-ups and the buffer hand-off through the slots; its threaded tests run on x86 only, which cannot catch a weakened ordering on the payload.
- Slice 2 adds no `unsafe` and no dependency. Parking is `std::thread::park`/`unpark` with a per-worker woken flag: only the flag ends a park, so a spurious return or a stray unpark only loops, and a wake before the park is never lost. The shutdown join polls `JoinHandle::is_finished` against the bound.
- The WSL gate's starved re-run names `transit-playback`, `transit-plugin-host` and `transit-ui-studio`; the race tests here run starved only once `transit-rt` is added to that list.

## Related

- Issue #124 (body, gates, design notes 2026-10-26 and 2026-10-27); #1 (unified render schedule); #8.
- [ADR 0009](0009-refutation-review-before-every-push.md): refutation review before merge.
