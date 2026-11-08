<!-- transit-index
status: Open, accepted
follow-up: Record only; revisit if libjack changes how it stops the thread.
order: 75
-->

Native JACK: JACK 2's `jack_deactivate` and `jack_client_close` stop the client thread with `pthread_cancel` (asynchronous cancel type). If that ever landed while the thread is inside the Rust process callback, a forced unwind would cross `extern "C"` frames with destructors (undefined behavior). Our order (stop and quiesce, then deactivate) makes it unreachable in normal stops, and a 20x kill-mid-callback stress did not show it. Every Rust JACK client shares it (libjack internals).
