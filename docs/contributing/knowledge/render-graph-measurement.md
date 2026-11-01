# Render graph measurement: counters, snapshots and the tail acceptance rule

Normative for ADR 0029 section 11 (#174 slice 2) and for the slice 4 decision. The counters are informational. No scheduling or ownership decision reads them.

## Attempts

A **published attempt** is one successful Idle-to-Published transition of a worker slot. Refused publications, Busy skips, `forgo` and direct callback renders are not attempts.

Each published attempt ends in exactly one of three ways:
- **claimed**: a worker won `Published -> Claimed`;
- **pre-deadline reclaimed** (`inline_pre`): the callback won `Published -> Reclaimed` through the inline path before the deadline, whenever that render completes;
- **deadline reclaimed** (`unclaimed_at_deadline`): the callback won `Published -> Reclaimed` during deadline resolution, whether the attempt was retired unrendered or rendered as a tail task. `deadline_reclaimed_tail` counts the tail-rendered subset; it is reported and never added.

## Stamps

The publisher writes three per-slot values with Relaxed stores, immediately before each Idle-to-Published transition, ordered by the Release publish:
- `pub_ns`: one clock read per publish;
- `deadline_ns`;
- `period_ns`: the period of the original host block, `frames * 1e9 / rate`, never a sub-block's.

## Worker counters

- Right after a successful claim, the worker increments `claimed` and reads the stamps after its Acquire claim.
- At completion, before its Release complete, it increments:
  - `queue_over` if `4 * (claim_ns - pub_ns) > period_ns`;
  - `late_w` if `end_ns > deadline_ns`;
  - `late_after_queue` if both hold;
  - then `classified`.
- A runner fault (caught by the worker loop) still completes and is classified.
- `claim_ns` and `end_ns` are the two clock reads that already time the render.

## Callback counters

- `published`;
- `inline_pre`;
- `unclaimed_at_deadline`;
- `deadline_reclaimed_tail` (a subset of `unclaimed_at_deadline`);
- `forgone` (Closed-window Forgo);
- `tail_callback` (direct callback renders of auxes).

All counters are preallocated atomic `u64`s with bounded operations. The callback's `Late` outcome (unfinished at the deadline sweep) remains the overrun figure. `late_w` is the slice 4 figure only.

## The snapshot mailbox

- **Participants.** The mailbox has one control-thread consumer and at most one outstanding request.
- **Payload.** Preallocated: the request identity, a completion status (complete, incomplete) and every counter, all as atomic fields.
- **Request.** The consumer posts a request (its identity, with a Release store) only after acknowledging the previous response.
- **Capture.** At the entry of a host block (`render_active`, after the seam overrides are installed and before any publish), the callback sees the request and calls `render_workers_settled()`. Settlement runs the panic checkpoint and can unwind into playback's `catch_unwind`.
  - If settlement succeeds, the callback copies every counter into the payload, sets status complete, and publishes the response with Release.
  - If settlement fails, the request stays pending to the next host block. After 1,000 host blocks the callback publishes the response with status incomplete.
- **Handoff.** The consumer reads the response after Acquire, copies the entire payload, then acknowledges with Release. The callback may not modify or reuse the payload until it observes the acknowledgment with Acquire. A new request is accepted only after the previous response is acknowledged. Other readers receive copies from the control-thread consumer.
- **Guarantees.** The callback never waits or retries for reader progress: an unacknowledged response just keeps the next request from being posted. Complete and incomplete responses use the same protocol. Payload storage uses safe, preallocated atomic fields. No new `unsafe`, allocation or blocking.
- **Not a boundary.** Transport stop is not a measurement boundary: rendering continues while stopped.

## Rows and validity

- A row is the difference between a complete start snapshot and a complete end snapshot.
- Every complete snapshot must satisfy conservation: `published == claimed + inline_pre + unclaimed_at_deadline` and `claimed == classified`.
- A row with an incomplete or non-conserving snapshot is invalid. Report it and make no decision from it.

## Slice 4 threshold

Integer arithmetic, computed per tail-graph row at 64 frames (idle, and under compile). Slice 4 gets a brief only if one row meets all three conditions:
- (a) `unclaimed_at_deadline * 10000 > published`;
- (b) `late_w > 0 && 2 * late_after_queue >= late_w`;
- (c) `queue_over * 100 > published`.

Report every term per row.

## Tail acceptance rule (the default flip of `TRANSIT_RENDER_TAIL`)

**Session.** `trancy-keyed-tail.transit`: the keyed trancy session plus
- Return A (a hosted reverb);
- Return B (a delay);
- sends from all six instruments, one of them pre-fader;
- a Drums group aux fed by Battery's output, with Solid Bus Comp keyed from the kick;
- a Music group aux nested before Main.

**Rows.** Serial tail against tail graph, paired, ten minutes each, Scarlett at 48 kHz:
- 64 frames idle;
- 128 frames idle;
- 64 frames under compile.

**Columns.**
- callbacks;
- over budget (`OB`);
- longest callback;
- strip overrun blocks by cause, with aux tasks listed apart;
- aux hole task-blocks (`H`): one per host block per aux task that was Late or Busy in any of the block's sub-blocks;
- upstream-missed;
- tail callback renders;
- deadline reclaims;
- overshoots;
- driver overloads and stream errors;
- every counter, the conservation checks and the slice 4 terms.

**Accept** when all of these hold:
- every paired row satisfies `OB_graph + H_graph <= OB_serial` (a tie with `H_graph == 0` passes);
- every graph row has 0 driver overloads and 0 stream errors;
- every row's snapshots are complete and conserving;
- the maintainer's listening pass at 64 frames.

The flip is a separate reviewed commit. Record the run in docs/benchmarks/<date>-scarlett-tail-graph-vs-serial.md with a row in the decisions table.
