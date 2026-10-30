//! Device facade for the prepared runtime. All factories, editors and reclamation
//! belong to this creating-thread owner; the device borrows its anchored audio state.
use super::audio::{ActionResult, Audio, Command, CountIn};
use super::native;
use super::{Notice, PlayerError, RuntimeControl, ServiceReport, SessionRequestId, sources};
use crate::backend::native::NativeKind;
use crate::record::start::Status;
use crate::{AudioConfig, EditorEdit, Fault, HeldInput, MeterSnapshot, OutputTrouble, PluginStates, Shared};
use crate::{action_lane, device_clock, devices, plan_bridge, record, scheduling, telemetry};
use std::cell::{Cell, RefCell, RefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use transit_audio_backend as ab;
use transit_mix::{Arrival, DeviceGeneration, Due, EngineEpoch, InstrumentEvent, Message, NoteId, PlanLimits, RenderTarget, Source};
use transit_model::{IoMap, Session, TrackId};
use transit_rt::handoff::{Anchor, AudioAccess};
use transit_time::{Range, Samples};

type Owner = RuntimeControl<Command, ActionResult>;
pub type PlayerNotice = Notice<Command, ActionResult>;

/// A prepared MIDI recording pass.
struct MidiPass {
    reader: record::start::Reader,
    tracks: Vec<TrackId>,
}

/// What a MIDI recording pass captured.
#[derive(Debug, Clone)]
pub struct MidiTake {
    /// The armed instrument tracks the pass recorded.
    pub tracks: Vec<TrackId>,
    /// The record-start latch the audio thread applied; None if it never started.
    pub start: Option<record::start::Point>,
    pub events: Vec<crate::midi_capture::CapturedEvent>,
    /// Events lost to a full queue.
    pub dropped: u64,
    pub device_rate: u32,
}

impl MidiTake {
    /// The events of the pass's tracks at or after the latch, each with the session samples
    /// since the latch: the device frames since the latch frame, converted at the session rate
    /// (the same latch the audio take is placed from). Events before the latch are not part of
    /// the take.
    pub fn timed(&self, session_rate: u32) -> Vec<(i64, crate::midi_capture::CapturedEvent)> {
        let Some(point) = self.start else { return Vec::new() };
        let rate = u128::from(self.device_rate.max(1));
        self.events
            .iter()
            .filter(|e| self.tracks.contains(&e.track))
            .filter_map(|e| {
                let frames = e.frame.checked_sub(point.output_frame)?;
                let samples = (u128::from(frames) * u128::from(session_rate) + rate / 2) / rate;
                Some((i64::try_from(samples).ok()?, *e))
            })
            .collect()
    }
}

/// Main-thread facade. RuntimeControl deliberately makes this type !Send/!Sync.
/// The remaining native lifecycle migration moves stream/writer joins off main;
/// it does not move this owner or its plug-ins to that worker.
pub struct Player {
    shared: Arc<Shared>,
    fault: Arc<Fault>,
    failure_signal: Option<plan_bridge::RuntimeIdentity>,
    owner: RefCell<Option<Owner>>,
    midi: RefCell<Option<super::native::Native>>,
    midi_error: Option<String>,
    /// The External Instrument tracks the last plan was told have no MIDI port (ADR 0020).
    external_off: RefCell<Vec<TrackId>>,
    record_start: RefCell<Option<record::start::Control>>,
    /// MIDI recording capture (control side) and the pass it is collecting.
    capture: RefCell<Option<crate::midi_capture::Capture>>,
    midi_pass: RefCell<Option<MidiPass>>,
    state: Option<Anchor<Audio>>,
    #[cfg(feature = "test-hooks")]
    test_output: RefCell<Option<DeviceOutput>>,
    stream: Option<Box<dyn ab::Stream>>,
    input: RefCell<HeldInput>,
    clock: RefCell<Option<device_clock::MaintainedClock>>,
    origin: device_clock::HostOrigin,
    epoch: EngineEpoch,
    audition: Option<sources::Token>,
    arrival: Cell<u64>,
    initial_error: Option<String>,
    watch: RefCell<Option<(u64, std::time::Instant)>>,
    silent_worker: Option<std::thread::JoinHandle<()>>,
    silent_stop: Arc<AtomicBool>,
    pub device_name: String,
    /// The name of the capture endpoint the backend paired with the open output (the user's
    /// pairing, else the shared container); `None` when the backend does not pair.
    pub paired_input: Option<String>,
    /// The audio system (cpal host) of the open device: "ASIO", "WASAPI", ...; empty for the silent clock.
    pub device_system: String,
    /// The open device is served by a native backend (WASAPI or ASIO, ADR 0016 and its draft ASIO
    /// amendment), not cpal; `device_system` says which.
    pub native_backend: bool,
    /// The native backend that serves the device, kept for the input when it opens on demand.
    native_factory: Option<Box<dyn ab::BackendFactory>>,
    /// The output stream's latency as the backend reported it at open (ASIO: `getLatencies`).
    pub output_latency: Option<std::time::Duration>,
    /// The device sample format the output stream converts to.
    pub output_sample: Option<ab::SampleFormat>,
    /// The names of the open device's output and input channels, in channel order, where the
    /// backend reports them (ASIO `getChannelInfo`); empty otherwise.
    pub output_channel_names: Vec<String>,
    pub input_channel_names: Vec<String>,
    /// The open device was opened in WASAPI exclusive mode (native backend only; false when the
    /// user asked to share the device or the open fell back to shared).
    pub device_exclusive: bool,
    /// Said when the chosen backend did not open and another did: "Native WASAPI failed: ...".
    pub backend_note: Option<String>,
    pub device_rate: u32,
    pub device_channels: u16,
    /// This machine's paths on the open device (ADR 0015).
    io: RefCell<Arc<IoMap>>,
    /// The hardware-insert round-trip probe (ADR 0015 section 5); the audio thread plays and records it.
    probe: Option<Arc<crate::hardware_probe::HwProbe>>,
    /// The External Instrument Measure (ADR 0020 decision 3); see `external_measure`.
    ext_probe: Option<Arc<crate::external_probe::ExtProbe>>,
    /// The real-time render's capture (ADR 0027); see `realtime_render`.
    render_tap: Option<Arc<crate::realtime_render::RenderTap>>,
    /// The browser's preview (#211); `None` when its feeder thread did not start.
    preview: Option<crate::preview::Preview>,
    /// The real-time render running, with its writer thread.
    render_job: RefCell<Option<realtime_render::Job>>,
    /// The loopback calibration probe (native ASIO, #20): the device callbacks play and record it
    /// by sample position ([`crate::loop_probe`]).
    loop_probe: Option<Arc<crate::loop_probe::LoopProbe>>,
    /// The key a loopback calibration of the open pair is stored under (a duplex open whose mode
    /// a calibration names); see [`Self::start_loop_calibration`].
    pub calibration_key: Option<devices::CalibrationKey>,
    /// The calibration the open streams use: the one saved for the key at the open, or one handed
    /// to them since ([`Self::apply_calibration`]).
    pub calibration: Option<ab::Calibration>,
    /// A live input ring feeds the plan (`set_input`): the probe has something to read.
    plan_input: Cell<bool>,
    pub device_buffer: Option<u32>,
    pub silent: bool,
}

impl Player {
    fn owner(&self) -> Result<RefMut<'_, Owner>, PlayerError> {
        let owner = self.owner.try_borrow_mut().map_err(|_| PlayerError::Busy)?;
        let mut owner = RefMut::filter_map(owner, Option::as_mut).map_err(|_| PlayerError::Faulted)?;
        if self.fault.failed.load(Ordering::Acquire) || self.fault.gone.load(Ordering::Acquire) {
            owner.fault();
        }
        Ok(owner)
    }
    pub fn initial_error(&self) -> Option<&str> {
        self.initial_error.as_deref()
    }
    pub fn update_session(&self, session: Arc<Session>) -> Result<SessionRequestId, PlayerError> {
        self.owner()?.update_session(session)
    }
    /// Restore an explicitly replaced document without reopening a same-rate device.
    pub fn replace_session(&self, session: Arc<Session>) -> Result<SessionRequestId, PlayerError> {
        self.owner()?.replace_session(session)
    }
    /// Includes view-only copy-on-write changes that do not advance Engine::revision.
    pub fn has_requested_session(&self, session: &Arc<Session>) -> bool {
        self.owner().is_ok_and(|owner| Arc::ptr_eq(owner.requested_session(), session))
    }
    /// The path table in use: this machine's, once the app has set it, else the defaults for the
    /// opened device.
    pub fn io_map(&self) -> Arc<IoMap> {
        Arc::clone(&self.io.borrow())
    }
    /// Use `io` as the path table. The opened output count stays the stream's (a different count
    /// is a reopen), so paths that do not fit are dropped; the table reaches the audio thread as
    /// a plan, with no device reopen.
    pub fn set_io(&self, mut io: IoMap) -> Result<SessionRequestId, PlayerError> {
        let opened = if self.silent { io.outputs } else { self.device_channels };
        io.reconcile(io.inputs, opened);
        // The buffer the device runs: part of an External Instrument measurement's key (ADR 0020
        // decision 5), so the plan picks the measurement made at this buffer.
        io.buffer = if self.silent { None } else { self.device_buffer };
        let io = Arc::new(io);
        let id = self.owner()?.set_io(Arc::clone(&io))?;
        if let Ok(mut held) = self.io.try_borrow_mut() {
            *held = io;
        }
        Ok(id)
    }
    pub fn set_input(&self, input: Arc<record::InputRing>) -> Result<SessionRequestId, PlayerError> {
        let id = self.owner()?.set_input(Some(input))?;
        self.plan_input.set(true);
        Ok(id)
    }

    /// Whether a live input feeds the plan: the device is duplex and its input is open, so a
    /// hardware insert's return has something to read.
    pub fn duplex(&self) -> bool {
        !self.silent && (self.plan_input.get() || self.holds_input()) && self.io_map().inputs > 0
    }

    /// Whether the player holds the input it opened with the output (native ASIO and native
    /// WASAPI duplex): it is open and running although the recorder has not taken it yet.
    pub fn holds_input(&self) -> bool {
        self.input.try_borrow().is_ok_and(|held| matches!(*held, HeldInput::Held(Some(_))))
    }

    /// Reads the held input (rate, channels, buffer, latency, stats) while the player has it;
    /// `None` once the recorder has taken it, or when there is none.
    pub fn with_held_input<R>(&self, read: impl FnOnce(&record::Recorder) -> R) -> Option<R> {
        match &*self.input.try_borrow().ok()? {
            HeldInput::Held(Some(recorder)) => Some(read(recorder)),
            _ => None,
        }
    }

    /// Start measuring the round trip of a hardware insert: a click train plays on the device
    /// channels of the send path (0-based `send`) and the probe records those of the return path
    /// (`ret`) for about two seconds and a third. Poll [`Self::hardware_probe_result`]. Refuses
    /// when the transport is playing, there is no live input, the speed is not 1x, or a
    /// measurement is already running. Control side only; the audio thread allocates nothing.
    pub fn start_hardware_probe(&self, send: &[u16], ret: &[u16]) -> Result<(), String> {
        let probe = self.probe.as_ref().filter(|_| !self.silent).ok_or("there is no audio device to measure through")?;
        let io = self.io_map();
        if !self.plan_input.get()
            && let Some(ring) = self.with_held_input(|recorder| Arc::clone(&recorder.monitor))
        {
            // The probe reads the live input ring: the held input feeds the plan for it.
            self.set_input(ring).map_err(|e| format!("the held input could not feed the probe ({e:?})"))?;
        }
        if !self.plan_input.get() || io.inputs == 0 {
            return Err("this device has no live input open (hardware inserts need a duplex device)".into());
        }
        if self.is_playing() {
            return Err("stop the transport before measuring".into());
        }
        if self.speed() != 1.0 {
            return Err("set the playback speed to 1x before measuring".into());
        }
        if send.iter().any(|c| *c >= self.device_channels) || ret.iter().any(|c| *c >= io.inputs) {
            return Err("a path names a channel the device did not open".into());
        }
        if self.loop_calibration_running() || self.external_measure_running() {
            return Err("a measurement is already running".into());
        }
        if !probe.arm(send, ret) {
            return Err("a measurement is already running".into());
        }
        Ok(())
    }

    /// The result of the measurement started by [`Self::start_hardware_probe`] once it is done
    /// (`None` while it runs, and after it has been taken).
    pub fn hardware_probe_result(&self) -> Option<Result<crate::hardware_probe::Measured, crate::hardware_probe::ProbeError>> {
        self.probe.as_ref()?.take_result()
    }

    /// Whether a measurement is running.
    pub fn hardware_probe_running(&self) -> bool {
        self.probe.as_ref().is_some_and(|p| p.armed())
    }

    /// Clicks sent so far and in all, for a running measurement; (0, total) otherwise. Reads
    /// atomics only.
    pub fn hardware_probe_progress(&self) -> (u32, u32) {
        self.probe.as_ref().map_or((0, 0), |p| p.progress())
    }

    /// Stop a measurement in progress.
    pub fn cancel_hardware_probe(&self) {
        if let Some(p) = &self.probe {
            p.cancel();
        }
    }
    pub fn play(&self, from: Samples, end: Option<Samples>, looped: Option<Range>) -> Result<action_lane::Sequence, PlayerError> {
        self.owner()?.send_action(Command::Play { from, end, looped }, true)
    }
    /// Retarget (or clear) the loop of a running transport without moving the playhead (#81). The
    /// callback adopts it at its next block boundary: a playhead before the new end wraps there,
    /// one past it plays on, and `None` plays through. It is transport state, not tied to a
    /// document: Cycle and region edits are document edits, so the snapshot that was latest when
    /// this was sent is routinely superseded by the edit's own, which must not cancel it. It still
    /// lands after any Play or Locate sent before it.
    pub fn set_loop(&self, looped: Option<Range>) -> Result<action_lane::Sequence, PlayerError> {
        self.owner()?.send_action(Command::SetLoop(looped), false)
    }
    pub fn locate(&self, at: Samples) -> Result<action_lane::Sequence, PlayerError> {
        self.cancel_record_start()?;
        self.owner()?.send_action(Command::Locate(at), true)
    }
    /// Prepare input history and files on Main. The device applies the start in
    /// the same ordered transaction as Play, without waiting for a UI receipt.
    pub fn prepare_recording(&self, recorder: &mut record::Recorder, takes: Vec<(TrackId, Vec<usize>, std::path::PathBuf)>) -> Result<(), String> {
        self.owner().map_err(|error| error.to_string())?;
        if recorder.has_pending_take() {
            return Err("already recording".into());
        }
        let mut control = self.record_start.try_borrow_mut().map_err(|_| PlayerError::Busy.to_string())?;
        let control = control.as_mut().ok_or_else(|| PlayerError::Faulted.to_string())?;
        let reader = control.prepare().ok_or_else(|| PlayerError::SequenceExhausted.to_string())?;
        if let Err(error) = recorder.prepare(takes, reader) {
            control.cancel();
            return Err(error);
        }
        Ok(())
    }
    /// `from=None` punches in at the consumed playhead without flushing DSP.
    pub fn record(
        &self,
        recorder: &record::Recorder,
        from: Option<Samples>,
        end: Option<Samples>,
        looped: Option<Range>,
    ) -> Result<action_lane::Sequence, PlayerError> {
        self.record_with_count_in(recorder, from, end, looped, None)
    }
    /// As [`Player::record`], but with `count_in` the click plays first while the transport waits at
    /// `from`, and Play with the recording latch starts at the sample the count-in ends.
    pub fn record_with_count_in(
        &self,
        recorder: &record::Recorder,
        from: Option<Samples>,
        end: Option<Samples>,
        looped: Option<Range>,
        count_in: Option<CountIn>,
    ) -> Result<action_lane::Sequence, PlayerError> {
        let ticket = recorder.prepared_start().ok_or(PlayerError::NoPreparedRecording)?;
        self.start_recording(ticket, from, end, looped, count_in)
    }
    /// As [`Player::record_with_count_in`] for a pass that records MIDI only (no audio input).
    pub fn record_midi_with_count_in(
        &self,
        from: Option<Samples>,
        end: Option<Samples>,
        looped: Option<Range>,
        count_in: Option<CountIn>,
    ) -> Result<action_lane::Sequence, PlayerError> {
        let ticket = self.midi_pass.try_borrow().map_err(|_| PlayerError::Busy)?.as_ref().map(|pass| pass.reader.ticket());
        self.start_recording(ticket.ok_or(PlayerError::NoPreparedRecording)?, from, end, looped, count_in)
    }
    fn start_recording(
        &self,
        ticket: record::start::Ticket,
        from: Option<Samples>,
        end: Option<Samples>,
        looped: Option<Range>,
        count_in: Option<CountIn>,
    ) -> Result<action_lane::Sequence, PlayerError> {
        let result = self.owner()?.send_action(Command::Record { ticket, from, end, looped, count_in }, true);
        if result.is_err() {
            self.cancel_record_start()?;
        }
        result
    }
    /// Prepare a MIDI recording pass of `tracks` (armed instrument tracks): clear the capture and
    /// share the record-start latch of the audio take prepared for the same pass (`audio`), else
    /// issue a latch of its own. The latch is applied by the audio thread with Play, so the pass
    /// starts at the same sample as any audio take.
    pub fn prepare_midi_recording(&self, tracks: Vec<TrackId>, audio: Option<record::start::Reader>) -> Result<(), String> {
        self.owner().map_err(|error| error.to_string())?;
        let reader = match audio {
            Some(reader) => reader,
            None => {
                let mut control = self.record_start.try_borrow_mut().map_err(|_| PlayerError::Busy.to_string())?;
                let control = control.as_mut().ok_or_else(|| PlayerError::Faulted.to_string())?;
                control.prepare().ok_or_else(|| PlayerError::SequenceExhausted.to_string())?
            }
        };
        let mut capture = self.capture.try_borrow_mut().map_err(|_| PlayerError::Busy.to_string())?;
        capture.as_mut().ok_or_else(|| PlayerError::Faulted.to_string())?.clear();
        *self.midi_pass.try_borrow_mut().map_err(|_| PlayerError::Busy.to_string())? = Some(MidiPass { reader, tracks });
        Ok(())
    }
    /// Is a MIDI pass prepared and not yet finished?
    pub fn midi_recording_prepared(&self) -> bool {
        self.midi_pass.try_borrow().is_ok_and(|pass| pass.is_some())
    }
    /// The events captured for the pass so far that belong to it (after the latch, on its tracks),
    /// as `(samples since the latch, event)` at `session_rate`. Drains the queue into the take
    /// list without consuming it, so Stop still finds every event.
    pub fn midi_recording_progress(&self, session_rate: u32) -> Vec<(i64, crate::midi_capture::CapturedEvent)> {
        let (Ok(mut capture), Ok(pass)) = (self.capture.try_borrow_mut(), self.midi_pass.try_borrow()) else { return Vec::new() };
        let (Some(capture), Some(pass)) = (capture.as_mut(), pass.as_ref()) else { return Vec::new() };
        capture.poll();
        let Status::Started(point) = pass.reader.status() else { return Vec::new() };
        MidiTake { tracks: pass.tracks.clone(), start: Some(point), events: capture.events().to_vec(), dropped: 0, device_rate: self.device_rate }
            .timed(session_rate)
    }
    /// The pass so far as a take (latch if applied, events captured to now), for drawing the
    /// recording while it runs. Drains the queue into the take list; nothing is consumed.
    pub fn midi_recording_snapshot(&self) -> Option<MidiTake> {
        let (mut capture, pass) = (self.capture.try_borrow_mut().ok()?, self.midi_pass.try_borrow().ok()?);
        let (capture, pass) = (capture.as_mut()?, pass.as_ref()?);
        capture.poll();
        let start = match pass.reader.status() {
            Status::Started(point) => Some(point),
            _ => None,
        };
        Some(MidiTake { tracks: pass.tracks.clone(), start, events: capture.events().to_vec(), dropped: 0, device_rate: self.device_rate })
    }
    /// End the MIDI pass: everything captured, the latch and the loss count. None when no pass
    /// was prepared.
    pub fn finish_midi_recording(&self) -> Option<MidiTake> {
        let pass = self.midi_pass.try_borrow_mut().ok()?.take()?;
        let mut capture = self.capture.try_borrow_mut().ok()?;
        let (events, dropped) = capture.as_mut()?.finish();
        let start = match pass.reader.status() {
            Status::Started(point) => Some(point),
            _ => None,
        };
        Some(MidiTake { tracks: pass.tracks, start, events, dropped, device_rate: self.device_rate })
    }
    /// Stand in for the audio thread: add an event to the capture, as a device-free test.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn inject_midi_capture(&self, event: crate::midi_capture::CapturedEvent) {
        if let Ok(mut capture) = self.capture.try_borrow_mut()
            && let Some(capture) = capture.as_mut()
        {
            capture.inject(event);
        }
    }
    fn cancel_record_start(&self) -> Result<(), PlayerError> {
        if let Some(control) = self.record_start.try_borrow().map_err(|_| PlayerError::Busy)?.as_ref() {
            control.cancel();
        }
        Ok(())
    }
    pub fn set_recording(&self, on: bool) -> Result<action_lane::Sequence, PlayerError> {
        if !on {
            self.cancel_record_start()?;
        }
        self.owner()?.send_action(Command::Recording(on), false)
    }
    pub fn stop(&self) -> Result<action_lane::Sequence, PlayerError> {
        self.cancel_record_start()?;
        let mut owner = self.owner()?;
        if let Some(midi) = &*self.midi.try_borrow().map_err(|_| PlayerError::Busy)? {
            midi.cutoff(&mut owner)?;
            midi.panic_outputs();
        }
        owner.stop()
    }
    pub fn all_notes_off(&self) -> Result<action_lane::Sequence, PlayerError> {
        let mut owner = self.owner()?;
        if let Some(midi) = &*self.midi.try_borrow().map_err(|_| PlayerError::Busy)? {
            midi.cutoff(&mut owner)?;
            midi.panic_outputs();
        }
        owner.all_notes_off()
    }
    pub fn note(&self, track: TrackId, pitch: u8, velocity: u8) -> Result<action_lane::Sequence, PlayerError> {
        if pitch > 127 || velocity > 127 {
            return Err(PlayerError::InvalidNote);
        }
        let mut owner = self.owner()?;
        let token = self.audition.ok_or(PlayerError::Faulted)?;
        if !owner.source_admitted(token) {
            return Err(PlayerError::Faulted);
        }
        let target = owner.binding(track, transit_mix::INSTRUMENT_SLOT).ok_or(PlayerError::NoInstrument)?;
        let sequence = self.arrival.get().checked_add(1).ok_or(PlayerError::SequenceExhausted)?;
        self.arrival.set(sequence);
        let message = if velocity == 0 {
            Message::NoteOff { key: pitch, velocity: 0, id: NoteId::NONE }
        } else {
            Message::NoteOn { key: pitch, velocity, id: NoteId::NONE }
        };
        owner.send_note(
            Command::Note(InstrumentEvent {
                engine: self.epoch,
                target,
                due: Due::Next,
                transport: None,
                source: Source { port: token.port(), channel: 0, generation: token.generation() },
                message,
                arrival: Arrival { host_ns: self.origin.now_ns().unwrap_or(0), sequence },
            }),
            if velocity == 0 { action_lane::NoteKind::Off } else { action_lane::NoteKind::On },
        )
    }
    /// Stop / All Notes Off acknowledgments are monotonic frontiers. An admitted
    /// urgent request is applied only once its corresponding frontier reaches it.
    pub fn recovery_progress(&self) -> Result<(action_lane::Sequence, action_lane::Sequence), PlayerError> {
        Ok(self.owner()?.recovery_progress())
    }
    pub fn poll_notices(&self, out: &mut [Option<PlayerNotice>]) -> Result<usize, PlayerError> {
        Ok(self.owner()?.poll_notices(out))
    }
    pub fn service(&self) -> Result<ServiceReport, PlayerError> {
        let mut owner = self.owner()?;
        if self.fault.failed.load(Ordering::Acquire) || self.fault.gone.load(Ordering::Acquire) {
            owner.fault();
        }
        let report = owner.service();
        {
            let mut midi = self.midi.try_borrow_mut().map_err(|_| PlayerError::Busy)?;
            let status = midi.as_mut().map(|midi| {
                midi.service(&mut owner, self.epoch);
                midi.output_status()
            });
            drop(midi);
            self.follow_external_ports(&mut owner, status.as_ref());
        }
        Ok(report)
    }

    /// An External Instrument whose MIDI port is missing (unplugged), will not open (held by
    /// another program), or cannot be reached because MIDI itself is unavailable (`status` None)
    /// plays silent with no latency, so delay compensation does not wait for a synth that cannot
    /// play (ADR 0020 decision 6). When the set of such tracks changes, the plan is prepared again.
    fn follow_external_ports(&self, owner: &mut Owner, status: Option<&super::midi_out::OutputStatus>) {
        let session = Arc::clone(owner.requested_session());
        let Ok(mut held) = self.external_off.try_borrow_mut() else { return };
        if held.is_empty() && !session.tracks.iter().any(|t| t.external_instrument().is_some()) {
            return;
        }
        let off = external_unavailable(&session, status);
        if off != *held && owner.set_external_unavailable(Arc::from(off.clone())).is_ok() {
            *held = off;
        }
    }
    /// Notes played on armed MIDI inputs since the last call (the UI's step entry); empty without MIDI.
    pub fn take_played_notes(&self) -> Vec<super::PlayedNote> {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            return m.take_notes();
        }
        Vec::new()
    }
    /// Ordered note/control input for step entry. Shares its drain with `take_played_notes`.
    pub fn take_played_midi(&self) -> Vec<super::PlayedMidi> {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            return m.take_midi();
        }
        Vec::new()
    }
    /// The same, each stamped with the instant the ingress worker saw it (#139).
    pub fn take_played_midi_stamped(&self) -> Vec<(super::PlayedMidi, std::time::Instant)> {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            return m.take_midi_stamped();
        }
        Vec::new()
    }
    /// Switches the MIDI Monitor taps (#91). Off by default: the ingress and output threads copy
    /// nothing while it is off.
    pub fn set_midi_monitor(&self, enabled: bool, show_clock: bool) {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            m.set_monitor(enabled, show_clock);
        }
    }
    /// Whether the MIDI Monitor taps are on in this Player (a new Player after an audio
    /// reconnect starts with them off).
    pub fn midi_monitor_enabled(&self) -> bool {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.monitor_enabled())).unwrap_or(false)
    }
    /// Input activity for the header LED (Clock and Active Sensing excluded): the counter, and
    /// the port name and bytes of the last message. Works with the monitor window closed, for
    /// the ports open for routing.
    pub fn midi_activity(&self) -> (super::monitor::Activity, String) {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            return m.monitor_activity();
        }
        Default::default()
    }
    /// Appends the MIDI Monitor's pending events (inputs and outputs, ordered by time, at most
    /// `max` per direction) to `events`; the report lists the ports and the dropped counts.
    pub fn take_midi_monitor(&self, events: &mut Vec<super::monitor::MonitorEvent>, max: usize) -> super::monitor::Report {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            return m.take_monitor(events, max);
        }
        super::monitor::Report::default()
    }
    pub fn midi_status(&self) -> Result<super::MidiStatus, PlayerError> {
        let midi = self.midi.try_borrow().map_err(|_| PlayerError::Busy)?;
        let mut status =
            midi.as_ref().map_or_else(|| super::MidiStatus { error: self.midi_error.clone(), ..Default::default() }, super::native::Native::status);
        let map = self.clock_report().and_then(|report| report.map);
        status.timing_quality = map.map(|map| map.quality);
        status.timing_uncertainty_ns = map.and_then(|map| map.offset_uncertainty_ns);
        Ok(status)
    }
    pub fn idle(&self) -> Result<Vec<EditorEdit>, PlayerError> {
        self.service()?;
        let mut owner = self.owner()?;
        let edits = owner.idle_editors();
        self.shared.editors_open.store(owner.any_editor_open(), Ordering::Relaxed);
        Ok(edits)
    }
    /// Refuse a save until the exact caller snapshot has a confirmed active plan.
    pub fn capture_states(&self, expected: &Arc<Session>) -> Result<PluginStates, PlayerError> {
        self.owner()?.capture_states(expected)
    }
    /// Whether the audio side plays exactly `expected` (its plan confirmed and rendered). Live
    /// state reads and loads wait for it. It guarantees that the values the document stores were
    /// sent to each plug-in's parameter queue after that document was rendered, not that the
    /// plug-in processed them, and a bypassed (or inactive) insert's values are not applied at
    /// all. See issue #219.
    pub fn plays(&self, expected: &Arc<Session>) -> bool {
        self.owner().is_ok_and(|owner| owner.plays(expected))
    }
    /// Preserve confirmed live state before a device reopen, including after a
    /// fault. None requires an explicit document-only recovery notice to the user.
    pub fn capture_states_for_reopen(&self, expected: &Arc<Session>) -> Result<Option<PluginStates>, PlayerError> {
        self.owner()?.capture_states_for_reopen(expected)
    }
    /// The plug-ins the last [`Self::capture_states`] (or reopen capture) left out, issue #56:
    /// their thread was stuck, or held by a modal dialog past the save deadline, so their stored
    /// states were kept. Each capture replaces the list; this takes it.
    pub fn take_unrefreshed_states(&self) -> Vec<String> {
        self.owner().map(|mut owner| owner.take_unrefreshed_states()).unwrap_or_default()
    }
    /// How long one state capture waits, in all, for plug-ins held by a modal dialog (default
    /// 500 ms). A plug-in that is answering is waited for, however slowly.
    pub fn set_save_deadline(&self, deadline: std::time::Duration) {
        if let Ok(mut owner) = self.owner() {
            owner.set_save_deadline(deadline);
        }
    }
    /// Whether this slot's plug-in is still being made (the slot plays silence until it is in a
    /// plan). The UI shows "Loading…" for it.
    pub fn loading(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|owner| owner.loading(track, slot))
    }
    /// Whether any plug-in is still being made; the frame loop comes back soon while true.
    pub fn any_loading(&self) -> bool {
        self.owner().is_ok_and(|owner| owner.any_loading())
    }
    /// Whether this slot's plug-in did not load within the plug-in host's creation budget (ADR
    /// 0018): the slot is empty and the UI says the plug-in is not responding, with Retry.
    pub fn stuck(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|owner| owner.stuck(track, slot))
    }
    /// Whether any slot is stuck (see [`Player::stuck`]).
    pub fn any_stuck(&self) -> bool {
        self.owner().is_ok_and(|owner| owner.any_stuck())
    }
    /// What this slot shows while its plug-in is on its way: "Loading…", "Loading… (slow)" with
    /// the time so far, or "Not responding" (issue #82). `None`: nothing pending for it.
    pub fn slot_load(&self, track: TrackId, slot: usize) -> Option<crate::SlotLoad> {
        self.owner().ok().and_then(|owner| owner.slot_load(track, slot))
    }
    /// Whether this slot's live plug-in is unresponsive: its host thread is stuck past the
    /// watchdog budget or was given up on (#205, #213).
    pub fn slot_unresponsive(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|owner| owner.slot_unresponsive(track, slot))
    }
    /// Whether a render worker has held `track`'s strip past [`transit_mix::HELD_SECS`] (#213, K89).
    pub fn track_held(&self, track: TrackId) -> bool {
        self.owner().is_ok_and(|owner| owner.track_held(track))
    }
    /// The phase this slot's plug-in is in while it loads (issue #67).
    pub fn slot_phase(&self, track: TrackId, slot: usize) -> Option<transit_plugin_host::LoadPhase> {
        self.owner().ok().and_then(|owner| owner.slot_phase(track, slot))
    }
    /// The plug-ins taken into a plan since the last call, with their load times (issue #67).
    pub fn take_loaded(&self) -> Vec<crate::LoadedIn> {
        self.owner().map(|mut owner| owner.take_loaded()).unwrap_or_default()
    }
    /// How long this slot's editor open has been on its way (issue #67).
    pub fn editor_opening(&self, track: TrackId, slot: usize) -> Option<std::time::Duration> {
        self.owner().ok().and_then(|owner| owner.editor_opening(track, slot))
    }
    /// The editor opens that finished since the last call, with their times (issue #67).
    pub fn take_editor_opened(&self) -> Vec<crate::EditorOpened> {
        self.owner().map(|mut owner| owner.take_editor_opened()).unwrap_or_default()
    }
    /// The slots that are not responding: (track, slot, plug-in id).
    pub fn stuck_slots(&self) -> Vec<(TrackId, usize, String)> {
        self.owner().map(|owner| owner.stuck_slots()).unwrap_or_default()
    }
    /// Makes this not-responding slot's plug-in again, on a responsive host thread, with the state
    /// the session stores (issue #82). Not a document change. False when the slot is not stuck.
    pub fn retry_plugin(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|mut owner| owner.retry_plugin(track, slot))
    }
    /// [`Self::retry_plugin`] for every slot that is not responding; how many.
    pub fn retry_all_plugins(&self) -> usize {
        self.owner().map_or(0, |mut owner| owner.retry_all_plugins())
    }
    /// The loading display's budget ("Loading… (slow)" after it; the request stops waiting for
    /// the instance then), with the real plug-in hosts (the heavy-instrument fixture test).
    #[cfg(feature = "test-hooks")]
    pub fn set_slow_after(&self, slow_after: std::time::Duration) {
        if let Ok(mut owner) = self.owner() {
            owner.set_slow_after(slow_after);
        }
    }
    /// Test seams for the hosted-instance path (the UI tests of the slot text and Retry).
    #[cfg(feature = "test-hooks")]
    pub fn set_plugin_test_hooks(&self, hooks: crate::PluginTestHooks) {
        if let Ok(mut owner) = self.owner() {
            owner.set_plugin_test_hooks(hooks);
        }
    }
    /// How many plug-in host threads are not responding now (a plug-in call past its timeout
    /// that has not returned yet); 0 normally, and back to 0 when the call returns.
    pub fn plugin_host_wedged(&self) -> usize {
        self.owner().map_or(0, |owner| owner.plugin_host_wedged())
    }
    /// The calls the plug-in host threads that are not responding are stuck in, each naming its
    /// plug-in and the call (issue #205); empty normally.
    pub fn plugin_host_stuck(&self) -> Vec<transit_plugin_host::StuckCall> {
        self.owner().map_or_else(|_| Vec::new(), |owner| owner.plugin_host_stuck())
    }
    /// Gives up on the plug-in host threads that are stuck (issue #205): their plug-ins are
    /// quarantined until transit exits and keep the state the session stores, their windows are
    /// asked to hide, and new plug-ins load on a fresh thread. Never waits for a plug-in.
    pub fn give_up_on_plugin_host(&self) -> transit_plugin_host::GaveUp {
        self.owner().map_or_else(|_| transit_plugin_host::GaveUp::default(), |owner| owner.give_up_on_plugin_host())
    }
    /// The plug-in host threads given up on whose call returned: parked, with the plug-ins they
    /// keep quarantined until transit exits (the diagnostics report lists them).
    pub fn plugin_host_quarantined(&self) -> Vec<transit_plugin_host::Quarantined> {
        self.owner().map_or_else(|_| Vec::new(), |owner| owner.plugin_host_quarantined())
    }
    /// Editor windows that were accepted for opening and then failed: (track, slot, why), once.
    pub fn take_editor_errors(&self) -> Vec<(TrackId, usize, String)> {
        self.owner().map(|mut owner| owner.take_editor_errors()).unwrap_or_default()
    }
    pub fn has_editor(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|owner| owner.has_editor(track, slot))
    }
    /// Whether the live plug-in in this insert slot has a sidechain input (the device card's key
    /// control is offered for it).
    pub fn has_key_input(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|owner| owner.has_key_input(track, slot))
    }
    pub fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String> {
        self.owner().map_err(|e| e.to_string())?.open_editor(track, slot)?;
        self.shared.editors_open.store(true, Ordering::Relaxed);
        Ok(())
    }
    pub fn close_editor(&self, track: TrackId, slot: usize) -> Result<(), PlayerError> {
        self.owner()?.close_editor(track, slot);
        Ok(())
    }
    pub fn close_editors(&self) -> Result<(), PlayerError> {
        self.owner()?.close_editors();
        self.shared.editors_open.store(false, Ordering::Relaxed);
        Ok(())
    }
    pub fn editor_open(&self, track: TrackId, slot: usize) -> bool {
        self.owner().is_ok_and(|owner| owner.editor_open(track, slot))
    }
    pub fn any_editor_open(&self) -> bool {
        self.shared.editors_open.load(Ordering::Relaxed)
    }
    /// What the header strip of this slot's editor window shows (issue #68).
    pub fn set_editor_header(&self, track: TrackId, slot: usize, header: &transit_dsp::EditorHeader) -> Result<(), PlayerError> {
        self.owner()?.set_editor_header(track, slot, header);
        Ok(())
    }
    /// What the user did in the editor windows' header strips: (track, slot, action).
    pub fn take_header_actions(&self) -> Vec<(TrackId, usize, transit_dsp::HeaderAction)> {
        self.owner().map(|mut owner| owner.take_header_actions()).unwrap_or_default()
    }
    /// The identity of the live instance in this slot (`None`: none). A job started for one
    /// instance must not land on another that took the slot meanwhile (issue #68).
    pub fn instance(&self, track: TrackId, slot: usize) -> Option<transit_mix::SlotGen> {
        self.owner().ok().and_then(|owner| owner.instance(track, slot))
    }
    /// The live plug-in's own name in this slot (`None`: no live instance).
    pub fn live_name(&self, track: TrackId, slot: usize) -> Option<&'static str> {
        self.owner().ok().and_then(|owner| owner.live_name(track, slot))
    }
    /// `Err(reason)` while this slot's editor cannot be driven (its plug-in is not responding).
    pub fn editor_available(&self, track: TrackId, slot: usize) -> Result<(), String> {
        self.owner().map_err(|e| e.to_string())?.editor_available(track, slot)
    }
    /// Starts reading this slot's live state (a preset save, a copy); poll it once per frame.
    pub fn begin_save_state(&self, track: TrackId, slot: usize) -> Result<transit_dsp::StateJob<Option<Vec<u8>>>, String> {
        self.owner().map_err(|e| e.to_string())?.begin_save_state(track, slot)
    }
    /// Starts loading a state into this slot's live instance (a preset); poll it once per frame.
    pub fn begin_load_state(
        &self,
        track: TrackId,
        slot: usize,
        data: Vec<u8>,
    ) -> Result<transit_dsp::StateJob<Option<transit_dsp::LoadedValues>>, String> {
        self.owner().map_err(|e| e.to_string())?.begin_load_state(track, slot, data)
    }
    /// Starts reading plug-in `id`'s default state (a fresh instance on the plug-in host thread).
    pub fn begin_default_state(&self, id: &str, ch: usize) -> Result<transit_dsp::StateJob<Option<Vec<u8>>>, String> {
        Ok(self.owner().map_err(|e| e.to_string())?.begin_default_state(id, ch))
    }
    pub fn editor_set_param(&self, track: TrackId, slot: usize, id: &str, value: f32) -> Result<(), PlayerError> {
        self.owner()?.editor_set_param(track, slot, id, value);
        Ok(())
    }
    pub fn set_speed(&self, speed: f32) {
        let value = if speed.is_finite() { (speed.clamp(0.1, 4.0) * 1000.0) as u32 } else { 1000 };
        self.shared.speed.store(value, Ordering::Relaxed);
    }
    pub fn speed(&self) -> f32 {
        self.shared.speed.load(Ordering::Relaxed) as f32 / 1000.0
    }
    /// The click's level in dB, on top of Setup > Click/Countoff. Reaches the audio thread with
    /// one atomic store; no session change, no undo.
    pub fn set_metronome_db(&self, db: f32) {
        let gain = if db.is_finite() { 10f32.powf(db.clamp(-60.0, 12.0) / 20.0) } else { 1.0 };
        self.shared.metro.gain.store(gain.to_bits(), Ordering::Relaxed);
    }
    /// A record count-in under way: (samples left, whole length) in project samples.
    pub fn count_in(&self) -> Option<(Samples, Samples)> {
        let left = self.shared.metro.count_in_left.load(Ordering::Relaxed);
        (left > 0).then(|| (left, self.shared.metro.count_in_total.load(Ordering::Relaxed)))
    }
    /// Why the recording start at the end of the last count-in failed, once; `None` when it did
    /// not. The transport stays parked and nothing records.
    pub fn take_start_failure(&self) -> Option<super::audio::StartFailure> {
        super::audio::StartFailure::from_code(self.shared.metro.start_failure.swap(0, Ordering::Relaxed))
    }
    /// Report a failed count-in start as the audio thread would, for device-free tests.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn report_start_failure(&self, failure: super::audio::StartFailure) {
        self.shared.metro.start_failure.store(failure as u8, Ordering::Relaxed);
    }
    pub fn position(&self) -> Samples {
        self.shared.position.load(Ordering::Relaxed)
    }
    /// The browser's preview player (#211): outside the plan and the session.
    pub fn preview(&self) -> Option<&crate::preview::Preview> {
        self.preview.as_ref()
    }
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }
    pub fn copy_meters(&self, out: &mut MeterSnapshot) -> Result<(), PlayerError> {
        self.owner()?.copy_meters(out);
        Ok(())
    }
    /// The scope taps of the open device's plans; `None` while the player is busy or faulted.
    pub fn scope_hub(&self) -> Option<Arc<transit_mix::ScopeHub>> {
        self.owner().ok().map(|o| o.scope_hub())
    }
    /// Switches the scope tap of an insert (by `Insert.uid`) on or off, with no recompile; false when
    /// the insert has no tap yet.
    pub fn scope_enable(&self, track: TrackId, uid: u64, on: bool) -> bool {
        self.scope_hub().is_some_and(|h| h.set_enabled(track, uid, on))
    }
    /// Copies the last `out.len()` frames of an insert's scope tap into `out` (see
    /// [`transit_mix::ScopeTap::read`]); 0 while there is no tap.
    pub fn scope_frames(&self, track: TrackId, uid: u64, out: &mut [f32]) -> usize {
        self.scope_hub().map_or(0, |h| h.frames(track, uid, out))
    }
    /// The gain reduction in dB an insert last reported to its tap.
    pub fn scope_gain_reduction_db(&self, track: TrackId, uid: u64) -> f32 {
        self.scope_hub().map_or(0.0, |h| h.gain_reduction_db(track, uid))
    }
    pub fn meters(&self) -> Result<MeterSnapshot, PlayerError> {
        let mut out = MeterSnapshot::default();
        self.copy_meters(&mut out)?;
        Ok(out)
    }
    pub fn thread_priority(&self) -> scheduling::WorkerPriority {
        self.stream.as_ref().map_or(scheduling::WorkerPriority::NoWorker, |stream| stream.thread_priority())
    }
    pub fn clock_report(&self) -> Option<device_clock::ClockReport> {
        self.clock.try_borrow_mut().ok()?.as_mut().map(device_clock::MaintainedClock::report)
    }
    pub fn trouble(&self) -> Option<OutputTrouble> {
        let stalled = self
            .watch
            .try_borrow_mut()
            .is_ok_and(|mut seen| crate::stalled(&mut seen, self.fault.beats.load(Ordering::Relaxed), self.is_playing() && !self.silent));
        let trouble = self.fault.trouble(stalled);
        if trouble.is_some()
            && let Ok(mut owner) = self.owner()
        {
            owner.fault();
        }
        trouble
    }
    /// Whether the open device must be reopened because its driver asked for it, and why. Unlike
    /// [`Self::trouble`] this is not said once: it holds for the life of this Player, which the
    /// app replaces to reopen the device ([`crate::reopen`]). `None` for a silent runtime and for
    /// any other trouble (a bug, a lost device).
    pub fn reopen_cause(&self) -> Option<crate::reopen::ReopenCause> {
        let fault = &self.fault;
        if self.silent || fault.failed.load(Ordering::Acquire) {
            return None;
        }
        if fault.reset.load(Ordering::Acquire) {
            Some(crate::reopen::ReopenCause::DriverReset)
        } else {
            fault.driver_stalled.load(Ordering::Acquire).then_some(crate::reopen::ReopenCause::DriverStalled)
        }
    }
    /// Whether the native backend that serves the device has its host thread blocked in a driver
    /// call (an ASIO driver's settings panel is open): a reopen now would be refused, so the app
    /// waits for it. False for cpal and for a silent runtime.
    pub fn host_blocked(&self, config: &AudioConfig) -> bool {
        self.native_factory.as_ref().and_then(|native| native.backend(config.system.as_deref())).is_some_and(|backend| backend.host_blocked())
    }
    pub fn open_input(&self, config: &AudioConfig) -> Result<record::Recorder, String> {
        let mut input = self.input.try_borrow_mut().map_err(|_| "input transition is busy")?;
        match &mut *input {
            HeldInput::No => {
                // The session's rate first: Windows resets an endpoint's default format when it
                // re-enumerates, and the recorder must not silently run at another rate.
                let config = AudioConfig { sample_rate: Some(self.device_rate), ..config.clone() };
                // Through the backend that serves the output (issue #3): a native output's input
                // is the native backend's too, falling back to cpal only if it cannot open.
                let recorder = match &self.native_factory {
                    Some(native) => record::Recorder::open_through(&config, &**native).or_else(|error| {
                        log::warn!("the native backend's input did not open ({error}); using the cpal backend");
                        record::Recorder::open_with(&config)
                    })?,
                    None => record::Recorder::open_with(&config)?,
                };
                check_input_rate(recorder.sample_rate, self.device_rate)?;
                let system = config.system.as_deref().unwrap_or(&self.device_system);
                log_input_open(
                    &recorder,
                    self.native_backend,
                    system,
                    config.buffer_size.map_or(ab::Period::Default, ab::Period::Frames),
                    if self.device_exclusive { ab::Mode::Exclusive } else { ab::Mode::Shared },
                );
                Ok(recorder)
            }
            HeldInput::Held(slot) => {
                let recorder = slot.take().ok_or("audio input is already open")?;
                recorder.monitor.clear_monitor();
                Ok(recorder)
            }
            HeldInput::Failed(error) => Err(error.clone()),
        }
    }
    /// On refusal the caller retains its recorder, including the device handle.
    pub fn close_input(&self, recorder: &mut Option<record::Recorder>) -> Result<(), PlayerError> {
        let mut input = self.input.try_borrow_mut().map_err(|_| PlayerError::Busy)?;
        if matches!(*input, HeldInput::Held(Some(_))) && recorder.is_some() {
            return Err(PlayerError::Busy);
        }
        if recorder.as_ref().is_some_and(record::Recorder::has_pending_take) {
            return Err(PlayerError::RecordingActive);
        }
        let Some(recorder) = recorder.take() else { return Ok(()) };
        if let HeldInput::Held(slot @ None) = &mut *input {
            *slot = Some(recorder);
        }
        Ok(())
    }
}

/// Refuses an input that opened at a rate other than the session's: there is no input resampler.
fn check_input_rate(input: u32, session: u32) -> Result<(), String> {
    if input == session {
        return Ok(());
    }
    Err(format!("input opened at {input} Hz for a {session} Hz session; set the device to the session rate"))
}

/// The INFO line for an input that opened beside the output (same rate, buffer and mode) or alone.
fn log_input_open(recorder: &record::Recorder, native: bool, system: &str, asked_period: ab::Period, mode: ab::Mode) {
    let sample = recorder.sample_format.unwrap_or(ab::SampleFormat::F32);
    let channels = u16::try_from(recorder.channels).unwrap_or(u16::MAX);
    let format = ab::Format::new(recorder.sample_rate, channels, sample);
    let asked = ab::OpenRequest { format, period: asked_period, mode, origin: ab::HostOrigin::default(), calibration: None };
    let got = ab::Achieved {
        latency: recorder.latency,
        ..ab::Achieved::requested(format, recorder.buffer_frames.map_or(ab::Period::Default, ab::Period::Frames))
    };
    log::info!(
        "{}",
        crate::open_log::describe(&crate::open_log::OpenReport {
            direction: "input",
            backend: if native { "native" } else { "cpal" },
            host: system,
            device: &recorder.device_name,
            asked,
            got,
        })
    );
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(anchor) = &self.state {
            anchor.close();
        }
        drop(self.midi.get_mut().take());
        {
            self.silent_stop.store(true, Ordering::Release);
            if let Some(worker) = &self.silent_worker {
                worker.thread().unpark();
            }
        }
        if let Some(stream) = &self.stream {
            let _ = stream.stop();
        }
        // Admission, not the backend's pause semantics, establishes quiescence.
        if let Some(anchor) = &self.state {
            anchor.quiesce();
        }
        // Retain the anchor through ordinary stream/worker teardown. A backend
        // may retain its callback even after a failed build, so endpoint loss is
        // not a prerequisite for reclaiming Audio on Main after quiescence.
        let input = std::mem::replace(self.input.get_mut(), HeldInput::No);
        drop(input);
        drop(self.stream.take());
        if let Some(worker) = self.silent_worker.take() {
            let _ = worker.join();
        }
        // Retained endpoints can only produce silence. Their eventual teardown
        // may free the empty handoff allocation, outside timed rendering; it
        // cannot destroy Audio, plug-ins or retained panic payloads.
        let audio = self.state.take().map(Anchor::take);
        if let (Some(owner), Some(audio)) = (self.owner.get_mut().take(), audio) {
            owner.retire(audio.into_ports());
        }
    }
}

fn render_held(
    access: &mut AudioAccess<Audio>,
    fault: &Fault,
    shared: &Shared,
    out: &mut [f32],
    channels: usize,
    plans: &mut plan_bridge::CallbackBudget,
    actions: &mut action_lane::Budget,
) -> bool {
    let Some(mut audio) = access.enter() else {
        fault.telemetry.unavailable_silence_frames.fetch_add((out.len() / channels.max(1)) as u64, Ordering::Relaxed);
        out.fill(0.0);
        return false;
    };
    if fault.failed.load(Ordering::Acquire) || fault.gone.load(Ordering::Acquire) {
        audio.fault();
    }
    if audio.fill(out, channels, plans, actions).is_err() {
        fault.failed.store(true, Ordering::Release);
    }
    if fault.failed.load(Ordering::Acquire) || fault.gone.load(Ordering::Acquire) {
        shared.playing.store(false, Ordering::Relaxed);
        fault.telemetry.fault_silence_frames.fetch_add((out.len() / channels.max(1)) as u64, Ordering::Relaxed);
    }
    true
}

/// The real backend and silent worker share this preallocated render adapter.
/// Its sole endpoint borrows Audio; the Player's anchor remains its final owner.
struct DeviceOutput {
    access: AudioAccess<Audio>,
    channels: usize,
    timing: telemetry::Accumulator,
    shared: Arc<Shared>,
    fault: Arc<Fault>,
    /// The loopback calibration probe: its click goes on top of the block, by sample position.
    loop_probe: Option<Arc<crate::loop_probe::LoopProbe>>,
    /// Typed device buffers in tests convert through the scratch of the adapter; real streams
    /// convert inside their adapter.
    #[cfg(test)]
    convert: crate::OutputCallback,
}

impl DeviceOutput {
    fn new(access: AudioAccess<Audio>, player: &Player, channels: usize) -> Self {
        // Initialise the platform clock (including any frequency cache) before callbacks run.
        let _ = telemetry::Timer::start().elapsed();
        Self {
            access,
            channels: channels.max(1),
            timing: telemetry::Accumulator::default(),
            shared: Arc::clone(&player.shared),
            fault: Arc::clone(&player.fault),
            loop_probe: player.loop_probe.clone(),
            #[cfg(test)]
            convert: crate::OutputCallback::new(channels),
        }
    }

    /// One device callback: the callback scope is shared by every conversion chunk of the
    /// block; it must never be reconstructed inside a chunk.
    fn render(&mut self, block: &mut ab::RenderBlock<'_>) {
        let mut plans = plan_bridge::callback_scope();
        let mut actions = action_lane::callback_scope();
        let (access, fault, shared, channels) = (&mut self.access, &*self.fault, &*self.shared, self.channels);
        let (probe, position) = (self.loop_probe.as_deref(), block.device_position);
        let mut offset = 0u64;
        block.render(&mut |buffer| {
            render_held(access, fault, shared, buffer, channels, &mut plans, &mut actions);
            fault.telemetry.marker().enter(telemetry::Phase::DeviceIo, None);
            if let Some(probe) = probe {
                probe.play(buffer, channels, position.map(|p| p + offset));
            }
            offset += (buffer.len() / channels) as u64;
        });
    }

    /// The same callback scope over `f32` samples with no device conversion (the silent clock).
    fn fill_f32(&mut self, data: &mut [f32]) {
        let mut plans = plan_bridge::callback_scope();
        let mut actions = action_lane::callback_scope();
        for buffer in data.chunks_mut(ab::SCRATCH_FRAMES * self.channels) {
            render_held(&mut self.access, &self.fault, &self.shared, buffer, self.channels, &mut plans, &mut actions);
        }
    }

    /// A typed device buffer through the conversion of the adapter.
    #[cfg(test)]
    fn fill<T: crate::backend::cpal::Sample>(&mut self, data: &mut [T]) {
        let mut plans = plan_bridge::callback_scope();
        let mut actions = action_lane::callback_scope();
        let (access, fault, shared, channels) = (&mut self.access, &*self.fault, &*self.shared, self.channels);
        self.convert.convert(data, &mut |buffer| {
            render_held(access, fault, shared, buffer, channels, &mut plans, &mut actions);
        });
    }
}

/// The render and error callbacks the backend runs on its audio thread.
fn render_callbacks(
    player: &Player,
    access: AudioAccess<Audio>,
    mut clock: device_clock::CallbackClock,
    channels: usize,
    rate: u32,
) -> (ab::RenderCallback, ab::ErrorCallback) {
    let mut output = DeviceOutput::new(access, player, channels);
    let fault = Arc::clone(&player.fault);
    let error = error_callback(Arc::clone(&player.fault), Arc::clone(&player.shared), player.failure_signal.clone());
    let render = move |block: &mut ab::RenderBlock<'_>| {
        crate::mark_audio_thread();
        telemetry::callback_entered(block.frames, rate);
        let started = telemetry::Timer::start();
        fault.telemetry.marker().begin();
        let creating_before = transit_mix::creating_instance();
        let frames = block.frames;
        clock.observer.record(frames, block.stamp);
        fault.beats.fetch_add(1, Ordering::Relaxed);
        output.render(block);
        let creating = creating_before || transit_mix::creating_instance();
        fault.telemetry.marker().end();
        let editor_open = output.shared.editors_open.load(Ordering::Relaxed);
        output.timing.record_overlap(&fault.telemetry, frames, rate, started.elapsed(), creating, editor_open);
    };
    (Box::new(render), Box::new(error))
}

/// Virtual fallback pacing, not a physical device clock. Keep sub-frame time
/// and bounded-work backlog rather than rounding/dropping them on every wake.
#[derive(Default)]
struct SilentPacer {
    scaled_nanoseconds: u128,
}
impl SilentPacer {
    fn advance(&mut self, elapsed: std::time::Duration, rate: u32, limit: usize) -> Option<usize> {
        let added = elapsed.as_nanos().checked_mul(u128::from(rate))?;
        let total = self.scaled_nanoseconds.checked_add(added)?;
        let frames = (total / 1_000_000_000).min(limit as u128) as usize;
        self.scaled_nanoseconds = total.checked_sub((frames as u128).checked_mul(1_000_000_000)?)?;
        Some(frames)
    }
}

/// Registered directly with the backend. Device loss must revoke admission even
/// when no further render callback or Main service occurs. No logging/formatting.
fn error_callback(
    fault: Arc<Fault>,
    shared: Arc<Shared>,
    signal: Option<plan_bridge::RuntimeIdentity>,
) -> impl FnMut(ab::StreamError) + Send + 'static {
    move |error| {
        crate::mark_audio_thread();
        // A driver's reset request (ASIO) stops the output like a lost device, but is said as
        // itself: the app reopens the device ([`crate::reopen`], `Player::reopen_cause`). So does
        // a driver the backend's watchdog found had stopped calling back.
        let reset = matches!(error, ab::StreamError::ResetRequested);
        let stalled = matches!(error, ab::StreamError::Stalled);
        if reset {
            fault.reset.store(true, Ordering::Release);
        }
        if stalled {
            fault.driver_stalled.store(true, Ordering::Release);
        }
        let gone = reset || stalled || matches!(error, ab::StreamError::DeviceNotAvailable);
        if gone {
            if let Some(signal) = &signal {
                signal.fault();
            }
            fault.gone.store(true, Ordering::Release);
            shared.playing.store(false, Ordering::Release);
        }
        fault.telemetry.error(matches!(error, ab::StreamError::DeviceNotAvailable));
    }
}

#[cfg(test)]
mod golden;
#[cfg(test)]
mod rate_tests {
    use super::check_input_rate;

    #[test]
    fn an_input_at_another_rate_is_refused_with_a_clear_message() {
        assert!(check_input_rate(44_100, 44_100).is_ok());
        assert_eq!(
            check_input_rate(48_000, 44_100).unwrap_err(),
            "input opened at 48000 Hz for a 44100 Hz session; set the device to the session rate"
        );
    }
}
mod calibrate;
mod external_measure;
mod midi_map_api;
#[cfg(test)]
mod midi_map_tests;
mod realtime_render;
mod telemetry_api;

/// The External Instrument tracks of `session` that cannot play: their MIDI port is Missing or
/// Failed, or MIDI is unavailable (`status` None). A port the output thread has not looked at yet
/// counts as present.
fn external_unavailable(session: &Session, status: Option<&super::midi_out::OutputStatus>) -> Vec<TrackId> {
    use super::midi_out::DestinationState;
    session
        .tracks
        .iter()
        .filter(|t| t.external_instrument().is_some())
        .filter(|t| status.is_none_or(|status| matches!(status.track(t.id), Some(DestinationState::Missing | DestinationState::Failed(_)))))
        .map(|t| t.id)
        .collect()
}

#[cfg(test)]
mod external_measure_tests;
#[cfg(test)]
mod external_tests;
#[cfg(test)]
mod realtime_render_tests;
#[cfg(test)]
mod tests;

mod open;
