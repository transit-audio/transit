//! The device-free step (ADR 0062 section 2, slice S2b): the transport part of `logic()` with no
//! window argument, in `logic`'s order (record section 10: receipts, the recording clock and lap
//! limit, device trouble, the render poll, the transport, then the write pass, session sync,
//! autosave and input). `logic` runs the same blocks in the same order, with its window work
//! between them, so a step here is what a frame does to the transport.

use crate::StudioApp;
use std::time::Duration;

impl StudioApp {
    /// The transport part of `logic()`. `frame_now` is the frame time `logic` reads from the
    /// window (seconds), `dt` the frame interval in seconds, `mono_now` a monotonic time that no
    /// block reads yet (the owed-Stop deadline arrives in slice S2d). No pointer is down.
    pub fn step_host(&mut self, frame_now: f64, dt: f64, mono_now: Duration) {
        let _ = mono_now;
        self.step_host_receipts();
        self.step_host_clock();
        self.step_host_devices();
        self.step_host_render();
        self.handle_transport(dt as f32);
        self.step_host_after(false, frame_now);
        self.player_requests.show_error(&mut self.status);
    }

    /// H1: the Player's idle work (editor edits; it services the runtime) and its notices.
    pub(crate) fn step_host_receipts(&mut self) {
        self.apply_editor_edits();
        self.poll_player();
    }

    /// H2: the recording clock follows the transport, then the lap limit.
    pub(crate) fn step_host_clock(&mut self) {
        self.update_midi_live();
        // After the pass's clock followed the transport (in `update_midi_live`).
        self.end_recording_at_lap_limit();
    }

    /// H3: output trouble, input trouble and loss, a pending reopen.
    pub(crate) fn step_host_devices(&mut self) {
        self.watch_output();
        self.watch_input();
        self.step_reopen();
    }

    /// H3r: the real-time render's poll (`logic` runs it inside the bounce step).
    pub(crate) fn step_host_render(&mut self) {
        if self.finish.realtime.running.is_some() {
            self.step_realtime();
        }
    }

    /// After the transport: the automation write pass (`held`: a pointer button is down; `now`:
    /// the frame time), the I/O and session sync once no play, locate or record admission is
    /// outstanding, autosave, and the live input a session needs.
    pub(crate) fn step_host_after(&mut self, held: bool, now: f64) {
        crate::automation::write_pass(self, held, now);
        if self.player_requests.play.is_none() && self.player_requests.locate.is_none() && self.player_requests.recording.is_none() {
            self.sync_io();
            self.sync_player_session();
            self.poll_low_latency();
        }
        self.autosave_changes();
        self.state_guard.pointer_down = held;
        self.autosave();
        if self.recorder.is_none() && self.input_failure.is_none() && self.engine.session().needs_live_input() {
            self.ensure_input();
        }
    }
}
