# E57: an autosave folder's lock outlived its session in a spawned child

1. **Failure.** Linux gate on 61439439 (WSL Ubuntu 22.04, normal-priority run):
   `midi_record_tests::a_crash_after_the_autosave_landed_discards_the_journal_without_a_duplicate_clip`
   (`crates/ui-studio`) panicked "index out of bounds: the len is 0 but the index is 0": the
   leftover scan found nothing right after the app was dropped. 739 passed, 1 failed; it passed alone.
2. **Mechanism.** An open session's autosave folder is held by an exclusive lock on its `lock`
   file; `leftovers` skips any folder whose lock it cannot take. The lock was released only by
   closing the file. On Linux and macOS the lock is a `flock`, which belongs to the open file, and a
   child process spawned by any thread gets a copy of every descriptor until its exec closes them.
   Other tests in the binary spawn processes all the time, so the writer thread's close left the
   lock held by a child mid-spawn, and the scan that followed skipped the folder. The writer was
   joined and the autosave was on disk: no ordering bug in the autosave or journal path. A spawn
   storm beside the drop-then-scan sequence missed the folder in 358 of 400 tries. The product was
   wrong too: a scan in a process that spawns (plug-in scanning) could skip a folder its own
   session had just let go. The launch scan runs before anything is held or spawned, so no user hit it.
3. **Why checks missed it.** The window is the few microseconds between a spawn's fork and exec,
   so only a run with many spawning tests in flight lines it up. On Windows a child gets no copy
   of the handle, so the Windows runs never showed it.
4. **Regression test.** `a_folder_let_go_is_offered_while_a_spawned_child_still_holds_a_copy_of_its_lock`
   (`crates/engine/src/autosave.rs`) duplicates the lock descriptor with `try_clone`, which shares
   the open file as a spawned child does, drops the folder and requires the scan to find it. The
   fix: `Autosave` unlocks explicitly when dropped or discarded, and `leftovers` unlocks its probe
   before closing (`release`). Mutation: closing without the unlock fails it at
   `leftovers(&root).len()` (0 against 1) on Linux, deterministically.
5. **Gate class.** A file lock is released with an explicit unlock, never by closing the file
   alone (`docs/contributing/evidence/gates.md`).
6. **Protocol change, owner, blind spot.** The rule is in `gates.md`. Owner: the coordinator.
   Blind spot: `Autosave::create` takes its lock right after creating the file, and a scan of the
   same root in the same instant could take it first and fail that autosave; the launch scan runs
   before the autosaver starts, and tests use their own roots, so this stays a note.
