# ADR 0029: Per-strip job slots for the multicore strip render

- **Status:** proposed 2026-10-27 (issue #124; slice 1, the slot protocol, implemented in `transit-rt` `strip_jobs`; slice 2, the worker set and the deadline policy, in `transit-rt` `worker_set`)
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

**Policies decided in #124, recorded here.** Overrun, admission and retirement are implemented by slice 2 (`worker_set`); deferred MIDI is slice 3; the default is slice 4.

| Policy | Rule |
|---|---|
| Overrun | A claimed strip is never canceled, re-run or rendered inline. The callback waits for `Done` no later than its deadline; past it the strip is silence for the block, a counted stream fault, and the slot is republished only after `Done` is observed and retired. Holding the block instead (a late block, no gap) becomes an option only if the bench shows it is ever better. |
| Admission | The callback reclaims only as many unstarted strips as fit in the time left, by each strip's measured cost; the rest are overruns. A starved pool degrades to serial with gaps, never to a late block. |
| Deferred MIDI | Events of a skipped block carry forward to the next processed block at offset 0, in order, in a fixed-capacity per-strip ring; a collapsed on/off pair gets the off one sample later; on overflow note-ons and ordinary controllers go first, the release class (note-off, sustain-off, All Notes Off, All Sound Off) stays, and past that the queue becomes All Sound Off + All Notes Off. Overflows are counted; recording is unaffected (capture at ingress and device) with an overrun marker in the take journal. |
| Retirement | Engine retirement gives each outstanding slot a bounded wait, then quarantines: the worker thread is abandoned, the instance never freed or reused, the fault logged with the plug-in's name. |
| Default | Serial stays the default unless the bench meets gate 6. |

## Preconditions for callers

The slot enforces its transitions; these rules are the caller's, and the executor's refutation review checks them.

| Rule | Detail |
|---|---|
| Single publisher | `publish`, `reclaim`, `consume`, `retire` and `JobSet::begin` only from the audio callback; `state`, `counts`, `faults` and `for_each_pending` from anywhere. Workers only `try_claim`/`try_claim_next` and complete. |
| Buffer ownership follows the slot state, by convention | Touch a strip's buffers and instance only while holding its `ClaimToken`, after a successful `reclaim`, or after `consume` returned `Ready` (until the next publish). Never on `state() == Done` alone: a `Done` result is read only through `consume`. |
| Faults are failures | `faults()` counts refused transitions in every build; tests outside `transit-rt` assert `faults() == 0` (inside it, a refusal already fails the test). |
| One `WorkerSet`, owned by the callback | Its block methods (`begin_block`, `publish`, `wake`, `finish`) take `&mut self`, which makes the single publisher a compile-time rule; `stats`, `outcomes` and `jobs` are reads. |
| Inputs only when not busy | Write a strip's inputs before `publish`, and only when `is_busy(strip)` is false: a busy strip's buffers belong to a worker until a later `begin_block` retires its result. |
| Mix only `Mixed` and `Inline` | After `finish`, a strip is mixed only when its `Outcome` is `Mixed` or `Inline`; `Overrun` is silence; `Faulted` is silence and a strip to retire. |
| Never reuse a quarantined instance | A strip named in a `Quarantine` report keeps its instance and buffers alive and unused for the life of the process. |

## Correctness gates (#124) and what slices 1 and 2 prove

| Gate | Requirement | Slice 1 (`strip_jobs`) | Slice 2 (`worker_set`) | Later slices must prove |
|---|---|---|---|---|
| 1 Exclusive execution | one executor per publish | **Proven for the slot:** threaded claim/reclaim race with injected spins and yields; free-running per-block test with a reentry guard; exhaustive operation sequences (depth 8, two actors) against the table | **Proven for the worker set:** real parked workers and the inline path under one reentry guard and a strictly growing generation check per strip; free-running, 1500 blocks in each of five configurations (1, 2, 3, 3 and 5 workers) with random spins, strips slower than a block and stalls released up to 20 blocks later; the payload hand-off checked in both directions through the workers | slice 4: the real strip processor under the same guard; the payload orderings on an ARM runner or under loom |
| 2 No incomplete or stale audio | only `Done` of this generation is mixed | **Proven for the slot:** `Ready` only for `Done` of this generation and only once; `Stale` retired unread; exhaustive per-block interleavings over three blocks | **Proven for the set:** `Mixed` only from `consume` returning `Ready`, cross-checked against the generation the executor ran; `Unstarted` and `Busy` strips never executed for the block; late results retired unread at the next `begin_block` (`late_results`) | slice 4: the mixdown reads a strip only on `Mixed` or `Inline` |
| 3 Bounded waiting and storage | no wait past the deadline; admission; event ring | groundwork only: every operation is wait-free and allocation-free (allocation-armed test) | **Proven:** `finish` never waits on a `Claimed` slot past its deadline (one stalled strip of eight: it returns after the deadline and less than 200 ms past it, the strip is `Overrun(Late)`, skipped next block as `Overrun(Busy)`, and resumes after release); admission from a per-strip cost EWMA (inline renders at most budget / cost, a strip that cannot fit never started, overshoot at most one strip's cost; with time to spare every unstarted strip is rendered inline); zero allocations on `begin_block`, `publish`, `wake` and `finish` and in the worker loop between runner calls, and no allocation retiring an idle set | slice 3: the deferred-event ring; slice 4: the starved gate and block-time percentiles on the bench |
| 4 MIDI and state discontinuities | carry-forward and overflow rules | none | none (the set reports `Overrun(Busy)`, the hook slice 3 needs) | slice 3: MIDI carry-forward at offset 0 with the overflow rule, and the continuity instrument driven through injected stalls |
| 5 Retirement | bounded wait, quarantine | none (a dropped token leaves the slot `Claimed`, which quarantine will detect) | **Proven:** `shutdown(bound)` returns an idle set at once; with a stalled worker it returns within the bound plus slack, reports the worker and its strip and leaves the slot `Claimed`, and the released worker later finishes into leaked state without harm; a runner panic is caught, the slot completed and the strip reported `Faulted` | slice 4: the engine retires through it and logs the plug-in's name; a quarantined instance is never reused |
| 6 Measured improvement | bench, continuity | none | none | slice 4: wiring into `crates/mix` behind the serial default, the #124 test matrix, and the default decision |

## Deviations from #124

| # | #124 says | Implemented | Why |
|---|---|---|---|
| 1 | `retire` is `Done -> Idle` | `consume` returning `Ready` is itself the `Done -> Idle` transition; `retire` covers an unread `Done` and `Reclaimed` | "ready once" without a field written outside the transitions |
| 2 | a stale `Done` is a counted fault | a stale `Done` is the normal overrun path (`Stale`, retired unread); the fault is a `Done` newer than the block asked for | a strip skipped at block N+1 whose worker finishes before the deadline read is `Done(N)` at N+1 by design |
| 3 | a debug assertion in tests | a refused transition asserts only in `transit-rt`'s own tests (`cfg(test)`); everywhere else, debug builds included, it is counted | no panic on the audio thread of a debug build |
| 4 | the callback waits for `Done` up to the deadline (no wait policy given) | `finish` sweeps the open slots; after a sweep without progress it spins (64 sweeps of 32 `spin_loop` hints), then calls `yield_now` between sweeps; never a lock, a park or a sleep | a spin alone starves a worker that shares the core; a park would need a wake from the worker on every completion |
| 5 | at the deadline, overrun the rest | an unstarted strip the admission limit refused stays claimable by the workers until the deadline; at the deadline it is reclaimed and retired unrendered (`Overrun(Unstarted)`) | a worker that started it after the deadline would hold the strip into the next block and turn one overrun into two |
| 6 | a worker unwinding from a plug-in panic leaves the slot `Claimed`: catch it and quarantine | the worker loop catches the panic, completes the slot and reports the strip `Faulted` (`runner_panics`); the worker lives on and the caller retires the strip | the thread is sound after a caught panic; only the instance is suspect, and quarantine is for threads that cannot be joined |
| 7 | at block start a `Claimed` slot is skipped and counted | `begin_block` marks the strip busy; the overrun is counted when the caller publishes it (`Published::Skipped`, `Overrun(Busy)`), and its result is retired only at a later `begin_block`, even if the worker finishes mid-block | a strip the plan no longer renders is not counted; results are never retired mid-block |

Additions, not deviations: `publish` checks `Idle` before it writes the generation (the single-publisher precondition makes that race-free) and refuses a generation that is not newer; `retire` from `Idle` or `Published` is refused too.

## Consequences

- No `unsafe` in the protocol; slots hold no buffers. Buffer and instance ownership follows the slot state and is enforced by the caller (the later executor), which is where its refutation review must look.
- `loom` was not added (a new dev-dependency in the unsafe crate is the owner's call). The exhaustive tests interleave whole operations, which is complete for this slot because each operation is one atomic read-modify-write on the state; Slice 2 adds parking, wake-ups and the buffer hand-off through the slots; its threaded tests run on x86 only, which cannot catch a weakened ordering on the payload.
- Slice 2 adds no `unsafe` and no dependency. Parking is `std::thread::park`/`unpark` with a per-worker woken flag: only the flag ends a park, so a spurious return or a stray unpark only loops, and a wake before the park is never lost. The shutdown join polls `JoinHandle::is_finished` against the bound.
- The WSL gate's starved re-run names `transit-playback`, `transit-plugin-host` and `transit-ui-studio`; the race tests here run starved only once `transit-rt` is added to that list.

## Related

- Issue #124 (body, gates, design notes 2026-10-26 and 2026-10-27); #1 (unified render schedule); #8.
- [ADR 0009](0009-refutation-review-before-every-push.md): refutation review before merge.
