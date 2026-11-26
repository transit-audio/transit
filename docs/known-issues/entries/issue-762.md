<!-- transit-index
status: Open (#762), fix checked through a fake failure only
follow-up: Check on the machine that showed the failure. A panic inside the graphics library is not covered; the maintainer decides whether it needs its own recovery.
-->

When the graphics library cannot create a window surface or find an adapter (a driver update can leave a driver registered where it no longer exists), transit retries once with the platform's default backend when `WGPU_BACKEND` asked for another, and otherwise shows what failed and where the log is instead of exiting silently (#762). The retry and the message act only on an error the graphics library returns. A panic inside the graphics library is not an error return: it unwinds past the fallback, so there is no retry and no graphics message, and the run ends through the panic shutdown (`apps/transit/src/graphics_fallback.rs` handles `Failure` values only; `main` catches the unwinding panic after it). The fix has been checked through a fake failure only (tests that stand in for the window with a closure that fails), not on the machine that showed the problem, so whether it recovers there is unconfirmed.
