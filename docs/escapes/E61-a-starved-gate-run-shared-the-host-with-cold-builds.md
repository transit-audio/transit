# E61: a starved gate run shared the host with cold builds

1. **Failure.** The #172 chain's Linux gate (WSL2, cf68ce1f, chain AZ) failed only in its
   starved run: `the_bounce_is_the_software_render_plus_the_return_on_the_grid` and
   `the_bounce_leaves_the_metronome_out_with_click_on`
   (`crates/playback/src/player/device/realtime_render_tests.rs`) hit the `until` helper's 10 s
   stall bound, `did not reach: the render completes while the device runs`. The same tree passed
   the gate's normal run and the Windows gate. While the starved run ran, two cold cargo builds
   that were not part of the gate were compiling on the Windows host.
2. **Mechanism.** The environment, not #172. The starved run nices the tests and a workspace
   `cargo check` inside the VM, so the kernel shares the VM's CPUs between them. Load on the host
   takes the VM's virtual CPUs away underneath that kernel; nice cannot arbitrate it, and every
   thread in the VM (the fake device's pump, the MIDI output thread, the render's file writer)
   waits alike. The render ends when the writer thread drains the ring and finishes the file
   (5 ms polls), so its wall time scales with how often the writer is scheduled.
   #172 is inert in this test. Its per-callback `set_callback_deadline` reads a `Copy`
   thread-local, and `MixEngine::set_device_block` does nothing without render workers. The
   playback tests never start workers: `FORCE_SERIAL` stays on, and no playback test clears it.
   The deadline it sets is read only on the worker path (`render_active`). No lock, allocation or
   wait was added on the path these threads share. Measured on the same WSL host, render time of
   the two tests (`RTTIME`, a temporary probe, not committed):

   | Run (5 or 10 each) | 89a28e51 (before #172) | cf68ce1f (#172) |
   |---|---|---|
   | Quiet, single-threaded | 0.08-0.20 s | 0.08-0.20 s |
   | Gate starvation, single-threaded (10 runs) | 0.14-0.66 s, 10/10 pass | 0.15-0.66 s, 10/10 pass |
   | Gate starvation, whole lib in parallel (as the gate runs it) | 0.25-1.14 s, lib 6.8-7.6 s, 5/5 pass | 0.26-1.26 s, lib 6.7-7.4 s, 5/5 pass |
   | Same, 24 busy CPU threads on the host | not run | 0.68-2.72 s, lib 13-17 s |
   | Same, 48 busy CPU threads on the host | not run | 1.33-4.73 s, lib 15-18 s |

   Busy threads on the host roughly double to quadruple the render time. The failing gate's lib
   took 22.2 s against about 7 s for a starved run on a quiet host. Two cold cargo builds add disk
   and memory pressure on top of CPU.
3. **Why checks missed it.** The starved run (E49 class) assumes the gate owns the host. Nothing
   checks that. Agents run gates and builds concurrently on one machine, and a Windows-side build
   is invisible from inside WSL.
4. **Regression test.** No product or test change. The 10 s bound has about 8x headroom over
   the worst render under the gate's own starvation (1.26 s). Even 48 busy host threads stayed
   under half of it. Widening it would only hide a real stall. The evidence is the table above.
   The two tests also passed under gate starvation 15 of 15 times at cf68ce1f and 15 of 15 at
   89a28e51.
5. **Gate class.** A gate's starved run is valid only on a host that runs nothing else heavy.
   Check before it starts: no `cargo`/`rustc` process on the Windows host
   (`tasklist.exe | grep -c "rustc.exe\|cargo.exe"` is 0). If one appears, the run is void: run
   it again once the host is quiet. A void run is not a pass, and a run on a busy host is not a
   failure of the tree. A failure on a quiet host stays a failure.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`,
   and the chain brief carries it for every Linux gate. Owner: the coordinator. Blind spot:
   other host load (a game, a video call, an indexer) is not counted. A starved-run failure is
   judged against the quiet-host table above before it is called a defect. Side finding from
   this hunt: under 24 busy host threads,
   `player::midi_out::tests::a_locate_with_a_lower_a_resets_the_deadline_clamp_so_the_next_lap_is_on_time`
   failed once on its 100 ms lateness bound. That is the E49 class, and it is tracked separately.
