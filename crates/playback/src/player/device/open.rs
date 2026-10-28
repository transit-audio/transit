use super::*;

impl Player {
    pub fn new(session: Arc<Session>) -> Self {
        Self::with_config(session, &AudioConfig::default())
    }

    /// Device failure falls back to an explicitly silent runtime. Initialization
    /// or identity exhaustion instead returns a faulted facade that rejects work.
    pub fn with_config(session: Arc<Session>, cfg: &AudioConfig) -> Self {
        match Self::open(Arc::clone(&session), cfg) {
            Ok(player) => player,
            Err(error) => {
                log::warn!("audio output unavailable: {error}");
                Self::silent_runtime(session, error)
            }
        }
    }

    /// Prepare the real Player without discovering or opening any device.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn prepare_device_free(session: Arc<Session>) -> Result<Self, String> {
        let rate = session.sample_rate.hz();
        Self::prepare(session, rate, IoMap::default()).map(|(mut player, access, _clock)| {
            let output = DeviceOutput::new(access, &player, 2);
            *player.test_output.get_mut() = Some(output);
            player
        })
    }

    /// Runs this Player's MIDI side on `io` (a fake backend in caller integration tests) instead of
    /// the system's MIDI devices.
    #[cfg(all(feature = "test-hooks", not(target_arch = "wasm32")))]
    #[doc(hidden)]
    pub fn enable_midi_for_test(&mut self, io: transit_midi_io::MidiIo) {
        if let Some(midi) = self.midi.get_mut().as_mut() {
            midi.enable(io);
        }
    }

    /// Consume one complete stereo block for device-free caller integration tests.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn step_device_free(&self) {
        // The caller's thread is the audio thread for this block only: left marked, its later
        // panics would be swallowed by the engine's audio-thread panic hook (no message at all
        // for a failing test) and its log records deferred.
        let was = crate::on_audio_thread();
        crate::mark_audio_thread();
        if let Ok(mut output) = self.test_output.try_borrow_mut()
            && let Some(output) = output.as_mut()
        {
            output.fill_f32(&mut [0.0f32; 1024]);
        }
        if !was {
            transit_mix::unmark_audio_thread();
        }
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            editors_open: AtomicBool::new(false),
            metro: crate::Metro::default(),
        })
    }

    pub(super) fn empty(rate: u32) -> Self {
        Self {
            shared: Self::shared(),
            fault: Arc::default(),
            failure_signal: None,
            owner: RefCell::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            midi: RefCell::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            midi_error: None,
            #[cfg(not(target_arch = "wasm32"))]
            external_off: RefCell::new(Vec::new()),
            record_start: RefCell::new(None),
            capture: RefCell::new(None),
            midi_pass: RefCell::new(None),
            state: None,
            #[cfg(feature = "test-hooks")]
            test_output: RefCell::new(None),
            stream: None,
            input: RefCell::new(HeldInput::No),
            clock: RefCell::new(None),
            origin: device_clock::HostOrigin::default(),
            epoch: EngineEpoch(0),
            audition: None,
            arrival: Cell::new(0),
            initial_error: None,
            #[cfg(not(target_arch = "wasm32"))]
            watch: RefCell::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            silent_worker: None,
            #[cfg(not(target_arch = "wasm32"))]
            silent_stop: Arc::new(AtomicBool::new(false)),
            device_name: "No audio device (silent)".into(),
            paired_input: None,
            device_system: String::new(),
            native_backend: false,
            native_factory: None,
            output_latency: None,
            output_sample: None,
            output_channel_names: Vec::new(),
            input_channel_names: Vec::new(),
            device_exclusive: false,
            backend_note: None,
            device_rate: rate,
            device_channels: 0,
            io: RefCell::new(Arc::new(IoMap::default())),
            probe: None,
            ext_probe: None,
            render_tap: None,
            render_job: RefCell::new(None),
            loop_probe: None,
            calibration_key: None,
            calibration: None,
            plan_input: Cell::new(false),
            device_buffer: None,
            silent: true,
        }
    }

    /// The stream clock is allocated BEFORE the engine; its exact generation is
    /// shared by the target, event identity, and consumed-output timeline.
    pub(super) fn prepare(
        session: Arc<Session>,
        device_rate: u32,
        io: IoMap,
    ) -> Result<(Self, AudioAccess<Audio>, device_clock::CallbackClock), String> {
        let (callback_clock, reader) = device_clock::stream_clock(device_clock::Direction::Output, device_rate)?;
        let key = reader.key();
        let target = RenderTarget {
            device: DeviceGeneration(key.generation),
            sample_rate: session.sample_rate.hz(),
            max_block: 512,
            out_channels: io.outputs,
            in_channels: io.inputs,
        };
        let mut player = Self::empty(device_rate);
        player.epoch = EngineEpoch(key.generation);
        player.origin = reader.host_origin();
        let (mut owner, ports) = Owner::new(session, None, target, PlanLimits::default()).map_err(|e| e.to_string())?;
        let audition = match owner.register_source(sources::AUDITION_PORT) {
            Ok(token) => token,
            Err(error) => {
                owner.retire(ports);
                return Err(error.to_string());
            }
        };
        let io = Arc::new(io);
        owner.use_io(Arc::clone(&io));
        *player.io.get_mut() = io;
        owner.service();
        if owner.faulted() {
            owner.retire(ports);
            return Err("initial audio plan preparation faulted".into());
        }
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let (mut audio, record_start) = match Audio::from_ports(ports, key, device_rate, Arc::clone(&player.shared)) {
            Ok(audio) => audio,
            Err(failure) => {
                owner.retire(*failure.ports);
                return Err(format!("invalid output timeline: {:?}", failure.error));
            }
        };
        *player.clock.get_mut() =
            device_clock::MaintainedClock::start(reader).map_err(|error| log::warn!("output clock maintenance unavailable: {error}")).ok();
        #[cfg(not(target_arch = "wasm32"))]
        audio.set_clock(player.clock.get_mut().as_mut().and_then(device_clock::MaintainedClock::take_audio_reader), player.origin);
        #[cfg(not(target_arch = "wasm32"))]
        match owner.midi_lane() {
            Ok((control, endpoint)) => match super::native::Native::new(control, player.origin) {
                Ok(mut native) => {
                    audio.set_midi(endpoint);
                    if let Some(out) = native.take_playback() {
                        audio.set_midi_out(out);
                    }
                    *player.midi.get_mut() = Some(native);
                }
                Err(error) => player.midi_error = Some(error.to_string()),
            },
            Err(error) => player.midi_error = Some(error.to_string()),
        }
        *player.record_start.get_mut() = Some(record_start);
        let (capture, sink) = crate::midi_capture::pair(crate::midi_capture::CAPACITY);
        audio.set_midi_capture(sink);
        let probe = Arc::new(crate::hardware_probe::HwProbe::new(target.sample_rate));
        audio.set_probe(Arc::clone(&probe));
        player.probe = Some(probe);
        let ext_probe = Arc::new(crate::external_probe::ExtProbe::new(target.sample_rate));
        audio.set_ext_probe(Arc::clone(&ext_probe));
        player.ext_probe = Some(ext_probe);
        let render_tap = Arc::new(crate::realtime_render::RenderTap::new(target.sample_rate));
        audio.set_render_tap(Arc::clone(&render_tap));
        audio.set_phase_marker(Arc::clone(player.fault.telemetry.marker()));
        player.render_tap = Some(render_tap);
        player.loop_probe = Some(Arc::new(crate::loop_probe::LoopProbe::new(device_rate)));
        *player.capture.get_mut() = Some(capture);
        let (anchor, access) = Anchor::new(audio);
        player.state = Some(anchor);
        player.failure_signal = Some(owner.failure_signal());
        *player.owner.get_mut() = Some(owner);
        player.audition = Some(audition);
        Ok((player, access, callback_clock))
    }

    fn open(session: Arc<Session>, cfg: &AudioConfig) -> Result<Self, String> {
        if let Some((native, kind)) = crate::backend::native::factory_for(cfg) {
            return Self::open_native(session, cfg, native, &crate::backend::cpal::CpalFactory, kind);
        }
        Self::open_with(session, cfg, &crate::backend::cpal::CpalFactory)
    }

    /// A native ASIO Player over a scripted duplex driver, for the caller's device-free tests: the
    /// input opens with the output and the player holds it.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn open_fake_native_asio(session: Arc<Session>, cfg: &AudioConfig, driver: &ab::fake::FakeBackend) -> Result<Self, String> {
        let fallback = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32));
        Self::open_native(session, cfg, Box::new(driver.clone()), &fallback, NativeKind::Asio)
    }

    /// [`Self::open_fake_native_asio`], or the silent runtime with its error when it does not open
    /// (as [`Self::with_config`] does), for the caller's device-free tests.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn with_fake_native_asio(session: Arc<Session>, cfg: &AudioConfig, driver: &ab::fake::FakeBackend) -> Self {
        match Self::open_fake_native_asio(Arc::clone(&session), cfg, driver) {
            Ok(player) => player,
            Err(error) => Self::silent_runtime(session, error),
        }
    }

    /// [`Self::open_native`] for native WASAPI with a scripted backend (tests).
    #[cfg(test)]
    pub(super) fn open_native_or_cpal(
        session: Arc<Session>,
        cfg: &AudioConfig,
        native: &ab::fake::FakeBackend,
        fallback: &dyn ab::BackendFactory,
    ) -> Result<Self, String> {
        Self::open_native(session, cfg, Box::new(native.clone()), fallback, NativeKind::Wasapi)
    }

    /// A native backend (WASAPI or ASIO, through the same [`Self::open_stepped`] ladder); when it
    /// cannot open (before `start()` succeeded) the cpal backend opens the same choices and the
    /// Player says why. WASAPI starts shared or exclusive as the user chose and steps down the
    /// exclusive-mode ladder; ASIO has one mode (the driver's), so it opens as planned and has no
    /// step. The failed attempt's streams are dropped here, on the control thread: dropping a
    /// native stream joins its thread or waits for the driver's host thread (see
    /// `transit-audio-wasapi`'s `RenderStream`, `transit-audio-asio`'s `AsioStream`) and must never
    /// happen on a callback thread.
    pub(super) fn open_native(
        session: Arc<Session>,
        cfg: &AudioConfig,
        native: Box<dyn ab::BackendFactory>,
        fallback: &dyn ab::BackendFactory,
        kind: NativeKind,
    ) -> Result<Self, String> {
        // WASAPI: shared unless the user asked for exclusive control of the device; each step down
        // the ladder (`exclusive_fallback`) reopens the whole device, before any stream has started.
        let mut step = match kind {
            NativeKind::Wasapi => {
                OpenStep { mode: Some(if cfg.share_device { ab::Mode::Shared } else { ab::Mode::Exclusive }), default_period: false, native: true }
            }
            // ASIO and JACK have one mode (the driver's or the server's).
            NativeKind::Asio | NativeKind::Jack => OpenStep { native: true, ..OpenStep::PLANNED },
        };
        // A device the native backend knows is not installed (an ASIO driver removed since it was
        // saved) is said as such: neither another device nor the cpal backend opens instead.
        if let Some(why) = cfg.output.as_deref().and_then(|name| native.backend(cfg.system.as_deref())?.missing_device(name)) {
            log::warn!("{} did not open: {why}", kind.label());
            return Err(why);
        }
        let mut notes: Vec<String> = Vec::new();
        loop {
            match Self::open_stepped(Arc::clone(&session), cfg, &*native, step) {
                Ok(mut player) => {
                    player.native_backend = true;
                    player.native_factory = Some(native);
                    player.backend_note = (!notes.is_empty()).then(|| notes.join("; "));
                    return Ok(player);
                }
                Err(failure) => {
                    if let Some((next, note)) = failure.error.as_ref().and_then(|error| exclusive_fallback(step, cfg, error)) {
                        log::warn!("{note} ({})", failure.text);
                        notes.push(note);
                        step = next;
                        continue;
                    }
                    // A host thread stuck in a driver call (its settings panel is open) refuses the
                    // open; the cpal fallback would load the same driver a second time.
                    if native.backend(cfg.system.as_deref()).is_some_and(|backend| backend.host_blocked()) {
                        log::warn!("{} did not open ({}); not falling back to cpal", kind.label(), failure.text);
                        return Err("the ASIO driver's settings panel is open; close it and reconnect".to_owned());
                    }
                    let reason = failure.text;
                    log::warn!("{} did not open ({reason}); using the cpal backend", kind.label());
                    let mut player = Self::open_with(session, cfg, fallback)?;
                    player.backend_note = Some(kind.fallback_note(&reason));
                    return Ok(player);
                }
            }
        }
    }

    /// Open the output through the audio system `factory` makes. Every device call goes through
    /// the backend trait; the adapter owns the sample format, the ASIO duplex order and the
    /// timestamps of the clocks.
    fn open_with(session: Arc<Session>, cfg: &AudioConfig, factory: &dyn ab::BackendFactory) -> Result<Self, String> {
        Self::open_stepped(session, cfg, factory, OpenStep::PLANNED).map_err(|failure| failure.text)
    }

    /// [`Self::open_with`] with `step` applied to every planned request; the failure keeps the
    /// backend's own error for the exclusive-mode ladder.
    ///
    /// **Open order.** For a duplex backend (`capture_with_render`: cpal's and the native ASIO
    /// driver, one set of buffers for both directions) both streams are opened before either
    /// starts: the input is built first ([`record::Recorder::build_on`], `open_capture`), then the
    /// output (`open_render`), then the input starts, then the output. That is the ASIO open-order
    /// rule of `transit-audio-asio` (`createBuffers` once for both directions, the driver started
    /// by the first `start`) and the order cpal's ASIO host needs; native WASAPI pairs two
    /// endpoints the same way. Recording later takes the held input without touching the streams.
    fn open_stepped(session: Arc<Session>, cfg: &AudioConfig, factory: &dyn ab::BackendFactory, step: OpenStep) -> Result<Self, OpenFailure> {
        let want_ch = u16::try_from(transit_mix::main_channels(&session)).unwrap_or(2);
        let backend = factory.backend(cfg.system.as_deref()).ok_or_else(|| "no audio system".to_string())?;
        // Prefer a config at the chosen (else session) rate; else the device default (we resample).
        let rates = cfg.sample_rate.into_iter().chain(Some(session.sample_rate.hz())).collect();
        let plan = backend.plan_render(&ab::OutputWish { device: cfg.output.clone(), rates, channels: want_ch, period: cfg.buffer_size })?;
        let (endpoint, system, name) = (&plan.endpoint, backend.system().to_owned(), plan.endpoint.name.clone());
        let first = plan.attempts.first().ok_or("no usable output stream")?;
        let duplex = backend.capture_with_render(endpoint);
        let paired = if duplex { backend.paired_input_name(endpoint) } else { None };
        // Every output the stream opened, and the duplex device's inputs, get default paths; the
        // app replaces them with this machine's saved table (`set_io`).
        let inputs = if duplex { backend.capture_format(endpoint, Some(first.format.rate)).map_or(0, |format| format.channels) } else { 0 };
        let wide = first.format.channels;
        let mut error = "no usable output stream".to_owned();
        // The first backend error decides the ladder: the first attempt is the device's own rate.
        let mut typed: Option<ab::OpenError> = None;
        for attempt in &plan.attempts {
            // A measured loopback of this exact pair, mode, rate and period makes both streams `Measured`.
            let mut request = *attempt;
            if let Some(mode) = step.mode {
                request.mode = mode;
            }
            if step.default_period {
                request.period = ab::Period::Default;
            }
            // On ASIO the loop depends on the buffer the driver runs, which it may choose itself (its
            // panel's size for "driver buffer", a fixed-size driver's one size): the calibration is
            // looked up by the achieved buffer after the open and handed to both streams then.
            let asio = system == devices::ASIO_SYSTEM;
            request.calibration = if duplex && !asio { cfg.calibration_for(&system, &name, paired.as_deref(), &request) } else { None };
            let (rate, channels) = (request.format.rate, request.format.channels);
            let io = IoMap::for_device(&system, &name, inputs, channels);
            let (mut player, access, clock) = Self::prepare(Arc::clone(&session), rate, io)?;
            *player.input.get_mut() = if duplex {
                let progress = player.record_start.get_mut().as_ref().map(record::start::Control::input_progress);
                let built = progress.ok_or_else(|| ab::OpenError::Other("the record start latch is missing".to_owned())).and_then(|progress| {
                    record::Recorder::build_on(
                        &*backend,
                        endpoint,
                        rate,
                        request.period,
                        request.mode,
                        progress,
                        request.calibration,
                        player.loop_probe.clone(),
                    )
                });
                match built {
                    Ok(recorder) => HeldInput::Held(Some(recorder)),
                    // Both streams must open in the same mode: an exclusive output beside a shared
                    // input would have the Windows mixer holding the device the output owns. So an
                    // exclusive input the ladder can answer fails the whole open, and the next
                    // step reopens output and input together. Any other input failure stays soft.
                    Err(error) if request.mode == ab::Mode::Exclusive && exclusive_fallback(step, cfg, &error).is_some() => {
                        return Err(OpenFailure { text: error.to_string(), error: Some(error) });
                    }
                    Err(error) => HeldInput::Failed(format!("the audio input did not open with the output ({error})")),
                }
            } else if system == devices::ASIO_SYSTEM {
                HeldInput::Failed("the ASIO output has no usable input".into())
            } else {
                HeldInput::No
            };
            let (render, on_error) = render_callbacks(&player, access, clock, usize::from(channels).max(1), rate);
            let (stream, achieved) = match backend.open_render(endpoint, &request, render, on_error) {
                Ok(opened) => opened,
                Err(failure) => {
                    error = failure.to_string();
                    typed.get_or_insert(failure);
                    continue;
                }
            };
            player.stream = Some(stream);
            // The key of a calibration of this pair: on ASIO the buffer the driver runs.
            let mut keyed = request;
            if asio && let Some(frames) = achieved.period.frames() {
                keyed.period = ab::Period::Frames(frames);
            }
            let calibration = match (duplex, asio) {
                (true, true) => cfg.calibration_for(&system, &name, None, &keyed),
                (true, false) => request.calibration,
                (false, _) => None,
            };
            if asio && calibration.is_some() {
                let output = player.stream.as_ref().is_some_and(|stream| stream.set_calibration(calibration));
                let input = match player.input.get_mut() {
                    HeldInput::Held(Some(recorder)) => recorder.set_calibration(calibration),
                    _ => false,
                };
                if !(output && input) {
                    log::warn!("the ASIO calibration of {name} could not be handed to both streams; the driver's figures are used");
                }
            }
            if let HeldInput::Held(Some(recorder)) = player.input.get_mut()
                && let Err(failure) = recorder.start()
            {
                // Keep the already-built ASIO direction owned until output closes;
                // removing it here could rebuild the shared driver's buffers.
                log::warn!("audio input did not start: {failure}");
                error = failure.to_string();
                continue;
            }
            let started = player.stream.as_ref().ok_or("output stream disappeared")?.start();
            if let Err(failure) = started {
                error = failure.to_string();
                typed.get_or_insert(failure);
                continue;
            }
            log::info!(
                "{}",
                crate::open_log::describe(&crate::open_log::OpenReport {
                    direction: "output",
                    backend: if step.native { "native" } else { "cpal" },
                    host: &system,
                    device: &name,
                    asked: request,
                    got: achieved,
                })
            );
            if wide != achieved.format.channels {
                log::info!("audio output: asked for {wide} channels, opened with {}", achieved.format.channels);
            }
            if let HeldInput::Held(Some(recorder)) = player.input.get_mut() {
                log_input_open(recorder, step.native, &system, request.period, request.mode);
            }
            player.device_name = name.clone();
            player.paired_input.clone_from(&paired);
            player.device_system = system.clone();
            player.device_channels = achieved.format.channels;
            player.device_buffer = achieved.period.frames();
            // The render workers' share follows the device's callback length, which the engine
            // learns at the first callback (#172); said here, on control, with the same rule.
            if let Some(frames) = player.device_buffer.filter(|_| !transit_mix::MixEngine::serial_forced()) {
                let overridden = transit_mix::strip_deadline_override();
                let rate = achieved.format.rate;
                let percent = transit_mix::resolve_strip_deadline_percent(overridden, frames, rate);
                log::info!(
                    "audio output: strip deadline {}",
                    transit_mix::strip_deadline_text(u64::from(percent), u64::from(frames), u64::from(rate), overridden.is_some())
                );
            }
            player.output_latency = achieved.latency;
            player.output_sample = Some(achieved.format.sample);
            player.output_channel_names = backend.channel_names(endpoint, ab::Direction::Output);
            player.input_channel_names = if duplex { backend.channel_names(endpoint, ab::Direction::Input) } else { Vec::new() };
            player.device_exclusive = request.mode == ab::Mode::Exclusive;
            // On ASIO the input is the driver's own: its calibration names none.
            let key_input = if system == devices::ASIO_SYSTEM { None } else { cfg.input_pairing.as_deref().or(paired.as_deref()) };
            player.calibration_key = if duplex { devices::CalibrationKey::of(&system, &name, key_input, &keyed) } else { None };
            player.calibration = calibration;
            player.silent = false;
            // A unit test enables a fake MIDI backend itself: no test reaches the system's MIDI
            // ports (a session naming a real synth's port would otherwise open it and play it).
            #[cfg(all(not(target_arch = "wasm32"), not(test)))]
            if let Some(midi) = player.midi.get_mut() {
                midi.enable(transit_midi_io::MidiIo::new("transit"));
            }
            return Ok(player);
        }
        Err(OpenFailure { text: error, error: typed })
    }

    fn silent_runtime(session: Arc<Session>, error: String) -> Self {
        let rate = session.sample_rate.hz();
        let (mut player, access, _clock) = match Self::prepare(session, rate, IoMap::default()) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let mut player = Self::empty(rate);
                player.initial_error = Some(format!("{error}; silent runtime failed: {failure}"));
                player.fault.failed.store(true, Ordering::Release);
                return player;
            }
        };
        // A timer-driven fallback never claims a physical output clock map.
        *player.clock.get_mut() = None;
        player.initial_error = Some(error);
        #[cfg(not(target_arch = "wasm32"))]
        player.start_silent(access, |task| std::thread::Builder::new().name("soundcraft-silent-clock".into()).spawn(task));
        #[cfg(target_arch = "wasm32")]
        drop(access);
        player
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn start_silent(
        &mut self,
        access: AudioAccess<Audio>,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) {
        let rate = self.device_rate;
        let mut callback = DeviceOutput::new(access, self, 2);
        let (shared, fault, stop) = (Arc::clone(&self.shared), Arc::clone(&self.fault), Arc::clone(&self.silent_stop));
        let failure_signal = self.failure_signal.clone();
        let spawn = spawn(Box::new(move || {
            let mut data = vec![0.0f32; 16_384];
            let mut last = std::time::Instant::now();
            let mut pacer = SilentPacer::default();
            while !stop.load(Ordering::Acquire) {
                std::thread::park_timeout(std::time::Duration::from_millis(10));
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let now = std::time::Instant::now();
                let frames = pacer.advance(now - last, rate, data.len() / 2);
                last = now;
                let Some(frames) = frames else {
                    if let Some(signal) = &failure_signal {
                        signal.fault();
                    }
                    fault.failed.store(true, Ordering::Release);
                    shared.playing.store(false, Ordering::Release);
                    break;
                };
                let len = frames * 2;
                fault.beats.fetch_add(1, Ordering::Relaxed);
                if let Some(data) = data.get_mut(..len) {
                    callback.fill_f32(data);
                }
            }
        }));
        match spawn {
            Ok(worker) => self.silent_worker = Some(worker),
            Err(error) => {
                self.fault.failed.store(true, Ordering::Release);
                if let Some(owner) = self.owner.get_mut() {
                    owner.fault();
                }
                self.initial_error = Some(format!("silent clock did not start: {error}"));
            }
        }
    }
}

/// How one open attempt departs from what the backend planned: the exclusive-mode ladder's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenStep {
    /// The mode to request; `None` keeps the planned one (the cpal backend has only shared).
    mode: Option<ab::Mode>,
    /// Open at the device's own period instead of the chosen buffer size.
    default_period: bool,
    /// The native backend (WASAPI or ASIO) opens, not cpal (the log line says which).
    native: bool,
}

impl OpenStep {
    const PLANNED: Self = Self { mode: None, default_period: false, native: false };
}

/// Why an open failed: the text, and the backend's error when it gave one.
struct OpenFailure {
    text: String,
    /// Read by the native backend's ladder only.
    error: Option<ab::OpenError>,
}

impl From<String> for OpenFailure {
    fn from(text: String) -> Self {
        Self { text, error: None }
    }
}

impl From<&str> for OpenFailure {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

/// The next step down the exclusive-mode ladder (ADR 0016) for an open at `step` that failed with
/// `error`, and what to tell the user; `None` when `error` is not one the ladder answers (the
/// caller falls back to cpal). Only before `start()` succeeded: a running stream is never reopened.
fn exclusive_fallback(step: OpenStep, cfg: &AudioConfig, error: &ab::OpenError) -> Option<(OpenStep, String)> {
    if step.mode != Some(ab::Mode::Exclusive) {
        return None;
    }
    let shared = OpenStep { mode: Some(ab::Mode::Shared), default_period: false, ..step };
    match error {
        ab::OpenError::PeriodRefused { .. } => {
            let frames = cfg.buffer_size.filter(|_| !step.default_period)?;
            Some((OpenStep { default_period: true, ..step }, format!("Exclusive mode could not use {frames} frames; using the device period")))
        }
        ab::OpenError::InUse(_) => {
            Some((shared, "Exclusive mode unavailable: another app holds the device; opened shared (higher latency)".to_owned()))
        }
        ab::OpenError::ExclusiveDisallowed(_) => Some((shared, "Exclusive mode is turned off for this device in Windows; opened shared".to_owned())),
        _ => None,
    }
}
