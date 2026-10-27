//! A live plug-in instance: component, audio processor and edit controller.

use super::*;

// ---- instances -----------------------------------------------------------------------------

/// One parameter as reported by `IEditController::getParameterInfo`, plus its plain range.
#[derive(Debug, Clone)]
pub(crate) struct RawParam {
    pub id: u32,
    pub title: String,
    pub units: String,
    pub step_count: i32,
    pub default_normalized: f64,
    pub flags: i32,
    /// The `IUnitInfo` unit the parameter belongs to (0 is the root unit).
    pub unit_id: i32,
}

pub(crate) const PARAM_HIDDEN: i32 = sv::ParameterInfo_::ParameterFlags_::kIsHidden;
pub(crate) const PARAM_READ_ONLY: i32 = sv::ParameterInfo_::ParameterFlags_::kIsReadOnly;
pub(crate) const PARAM_BYPASS: i32 = sv::ParameterInfo_::ParameterFlags_::kIsBypass;

/// The audio bus layout chosen by [`Instance::configure`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Layout {
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
    pub event_input: bool,
    /// The activated auxiliary (sidechain) audio input bus, when the plug-in declares one and
    /// accepted an arrangement for it.
    pub key_bus: Option<usize>,
}

pub(super) fn apply_transport(c: &mut sv::ProcessContext, t: &Transport) {
    use sv::ProcessContext_::StatesAndFlags_::*;
    let mut state = kTempoValid | kTimeSigValid | kProjectTimeMusicValid | kBarPositionValid | kContTimeValid;
    if t.playing {
        state |= kPlaying;
    }
    if t.recording {
        state |= kRecording;
    }
    // The flag constants are the C default enum type: i32 on Windows, u32 elsewhere.
    #[allow(clippy::unnecessary_cast)]
    let state = state as u32;
    c.state = state;
    c.tempo = t.tempo;
    c.timeSigNumerator = i32::try_from(t.numerator).unwrap_or(4);
    c.timeSigDenominator = i32::try_from(t.denominator).unwrap_or(4);
    c.projectTimeSamples = t.samples;
    c.projectTimeMusic = t.quarters;
    c.barPositionMusic = t.bar_start;
}

/// A live plugin: component + audio processor (+ edit controller) and the host objects.
pub(crate) struct Instance {
    pub(super) processor: Option<ComPtr<sv::IAudioProcessor>>,
    pub(super) controller: Option<ComPtr<sv::IEditController>>,
    /// The controller is the component itself (single-component plugin).
    pub(super) single: bool,
    pub(super) connections: Option<(ComPtr<sv::IConnectionPoint>, ComPtr<sv::IConnectionPoint>)>,
    pub(super) component_initialized: bool,
    pub(super) controller_initialized: bool,
    pub(super) active: bool,
    pub(super) processing: bool,
    pub(super) max_frames: usize,
    pub(super) in_params: ComWrapper<ParamChanges>,
    pub(super) out_params: ComWrapper<ParamChanges>,
    pub(super) in_events: ComWrapper<EventList>,
    pub(super) out_events: ComWrapper<EventList>,
    pub(super) context: sv::ProcessContext,
    pub(super) event_in: bool,
    pub(super) editor: Arc<EditorLink>,
    // Field order matters for drop: `Drop::drop` shuts the plugin down first; then the component
    // and host objects are released, and the bundle (which keeps the code mapped) goes last.
    pub(super) component: ComPtr<sv::IComponent>,
    pub(super) host: ComWrapper<HostContext>,
    pub(super) bundle: Arc<Bundle>,
}

// SAFETY: an `Instance` is used through `&mut self` only, so calls into the plugin are always
// serialised. SoundCraft creates instances on the engine/UI thread and then moves them into the
// mix engine (documented limitation: VST3 expects `setActive` and `getLatencySamples` on the UI
// thread; they may then happen on the audio thread). Edit-controller calls never do: parameter
// values reach the processor converted through curves sampled on the UI thread (`params.rs`).
unsafe impl Send for Instance {}

impl Instance {
    fn processor(&self) -> Option<&ComPtr<sv::IAudioProcessor>> {
        self.processor.as_ref()
    }

    /// A counted reference to the host context (editor handles read its edit queue).
    pub fn host_ref(&self) -> ComWrapper<HostContext> {
        self.host.clone()
    }

    /// The editor link shared with editor handles (`None` without an edit controller).
    pub fn editor(&self) -> Option<Arc<EditorLink>> {
        self.editor.has_controller().then(|| Arc::clone(&self.editor))
    }

    /// The same link, for state handles.
    pub fn state_link(&self) -> Arc<EditorLink> {
        Arc::clone(&self.editor)
    }

    /// Writes one state into a fresh memory stream.
    pub(super) fn read_state(f: impl FnOnce(*mut sb::IBStream) -> sb::tresult) -> Option<Vec<u8>> {
        let stream = MemoryStream::new(Vec::new());
        let s = stream.as_com_ref::<sb::IBStream>()?;
        ok(f(s.as_ptr())).then(|| stream.take())
    }

    /// Restores a [`EditorLink::save_state`] blob (or a bare component state): component
    /// `setState`, then the controller's `setComponentState` and `setState`. `false` when the
    /// blob is malformed or a component/controller refuses its state.
    pub fn load_state(&self, data: &[u8]) -> bool {
        let Some((comp, ctrl)) = split_state(data) else { return false };
        let stream = MemoryStream::new(comp.to_vec());
        let Some(s) = stream.as_com_ref::<sb::IBStream>() else { return false };
        // SAFETY: valid component; the stream lives on this frame for the call.
        if !ok(unsafe { self.component.setState(s.as_ptr()) }) {
            return false;
        }
        if let Some(c) = &self.controller {
            stream.lock().1 = 0;
            // SAFETY: valid controller; the same stream, rewound.
            let result = unsafe { c.setComponentState(s.as_ptr()) };
            // A controller may leave component-state synchronization unimplemented.
            if !ok(result) && result != sb::kNotImplemented {
                return false;
            }
            if !ctrl.is_empty() {
                let cs = MemoryStream::new(ctrl.to_vec());
                let Some(r) = cs.as_com_ref::<sb::IBStream>() else { return false };
                // SAFETY: valid controller; the stream lives on this frame for the call.
                if !ok(unsafe { c.setState(r.as_ptr()) }) {
                    return false;
                }
            }
        }
        true
    }

    /// Copies the component's state into the controller (`getState` → `setComponentState`).
    pub(super) fn sync_controller_state(&self) {
        let Some(c) = &self.controller else { return };
        let stream = ComWrapper::new(MemoryStream { inner: Mutex::new((Vec::new(), 0)) });
        let Some(s) = stream.as_com_ref::<sb::IBStream>() else { return };
        // SAFETY: valid component; the stream lives on this frame for the whole call.
        if ok(unsafe { self.component.getState(s.as_ptr()) }) {
            stream.lock().1 = 0;
            // SAFETY: valid controller; same stream, rewound.
            unsafe { c.setComponentState(s.as_ptr()) };
        }
    }

    pub fn params(&self) -> Vec<RawParam> {
        let Some(c) = &self.controller else { return Vec::new() };
        // SAFETY: valid controller.
        let n = unsafe { c.getParameterCount() }.clamp(0, MAX_PARAMS);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: `ParameterInfo` is plain data; all-zero is valid.
            let mut info: sv::ParameterInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            if !ok(unsafe { c.getParameterInfo(i, &mut info) }) {
                continue;
            }
            let d = info.defaultNormalizedValue;
            out.push(RawParam {
                id: info.id,
                title: read_c16(&info.title),
                units: read_c16(&info.units),
                step_count: info.stepCount.max(0),
                default_normalized: if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 },
                flags: info.flags,
                unit_id: info.unitId,
            });
        }
        out
    }

    /// Each unit's path from the root (`IUnitInfo`), names joined by `/`; the root unit (id 0) is
    /// the empty path and is left out. Empty when the controller has no unit info. Main thread.
    pub fn unit_paths(&self) -> std::collections::HashMap<i32, String> {
        let Some(u) = self.controller.as_ref().and_then(|c| c.cast::<sv::IUnitInfo>()) else { return Default::default() };
        // SAFETY: valid unit-info interface on the main thread.
        let n = unsafe { u.getUnitCount() }.clamp(0, MAX_PARAMS);
        let mut raw: Vec<(i32, i32, String)> = Vec::new();
        for i in 0..n {
            // SAFETY: `UnitInfo` is plain data; all-zero is valid.
            let mut info: sv::UnitInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            if ok(unsafe { u.getUnitInfo(i, &mut info) }) {
                raw.push((info.id, info.parentUnitId, read_c16(&info.name)));
            }
        }
        unit_paths_from(&raw)
    }

    /// Normalized → plain (`None` without a controller or for a non-finite answer). UI thread
    /// only (VST3): the audio thread converts through `ParamCurves` instead.
    pub fn to_plain(&self, id: u32, normalized: f64) -> Option<f64> {
        let c = self.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.normalizedParamToPlain(id, normalized.clamp(0.0, 1.0)) };
        v.is_finite().then_some(v)
    }

    pub fn normalized_value(&self, id: u32) -> Option<f64> {
        let c = self.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.getParamNormalized(id) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    /// The parameter each MIDI controller drives on each of the 16 channels (`IMidiMapping`):
    /// channel-major, each channel indexed by VST3's controller numbers (CCs 0–127, then
    /// [`AFTERTOUCH`] and [`PITCH_BEND`]). Main thread only.
    pub fn midi_mapping(&self) -> Vec<Option<u32>> {
        let Some(m) = self.controller.as_ref().and_then(|c| c.cast::<sv::IMidiMapping>()) else { return Vec::new() };
        (0..MIDI_CHANNELS)
            .flat_map(|ch| (0..=PITCH_BEND).map(move |n| (ch, n)))
            .map(|(ch, n)| {
                let mut id: sv::ParamID = 0;
                let (ch, n) = (i16::try_from(ch).unwrap_or(0), i16::try_from(n).unwrap_or(0));
                // SAFETY: valid mapping interface on the main thread; `id` is a local.
                ok(unsafe { m.getMidiControllerAssignment(0, ch, n, &mut id) }).then_some(id)
            })
            .collect()
    }

    pub fn value_text(&self, id: u32, normalized: f64) -> Option<String> {
        let c = self.controller.as_ref()?;
        let mut s: sv::String128 = [0; 128];
        // SAFETY: valid controller; `s` is a writable String128.
        ok(unsafe { c.getParamStringByValue(id, normalized.clamp(0.0, 1.0), &mut s) }).then(|| read_c16(&s)).filter(|t| !t.is_empty())
    }

    pub fn latency(&self) -> u32 {
        // SAFETY: valid processor.
        self.processor().map_or(0, |p| unsafe { p.getLatencySamples() })
    }

    pub fn tail(&self) -> u32 {
        // SAFETY: valid processor.
        self.processor().map_or(0, |p| unsafe { p.getTailSamples() })
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn bus_count(&self, media: sv::MediaTypes, dir: sv::BusDirections) -> i32 {
        // SAFETY: valid component.
        unsafe { self.component.getBusCount(media as sv::MediaType, dir as sv::BusDirection) }.clamp(0, MAX_BUSES)
    }

    fn arrangement(&self, dir: sv::BusDirections, index: i32) -> Option<sv::SpeakerArrangement> {
        let p = self.processor()?;
        let mut arr: sv::SpeakerArrangement = 0;
        // SAFETY: valid processor; `arr` is writable.
        ok(unsafe { p.getBusArrangement(dir as sv::BusDirection, index, &mut arr) }).then_some(arr)
    }

    fn bus_channels(&self, dir: sv::BusDirections, index: i32) -> u32 {
        if let Some(arr) = self.arrangement(dir, index) {
            return arr.count_ones().min(MAX_BUS_CHANNELS);
        }
        // SAFETY: plain data; all-zero is valid.
        let mut info: sv::BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: valid component; index below the bus count; `info` is writable.
        if ok(unsafe { self.component.getBusInfo(sv::MediaTypes_::kAudio as sv::MediaType, dir as sv::BusDirection, index, &mut info) }) {
            u32::try_from(info.channelCount).unwrap_or(0).min(MAX_BUS_CHANNELS)
        } else {
            0
        }
    }

    /// Whether the component declares an auxiliary audio input (a sidechain) to activate.
    pub fn declares_key_input(&self) -> bool {
        self.aux_input_bus(self.bus_count(sv::MediaTypes_::kAudio, sv::BusDirections_::kInput)).is_some()
    }

    /// The first audio input bus after the main one that the plug-in declares auxiliary (`kAux`).
    fn aux_input_bus(&self, count: i32) -> Option<i32> {
        (1..count).find(|&i| {
            // SAFETY: plain data; all-zero is valid.
            let mut info: sv::BusInfo = unsafe { std::mem::zeroed() };
            // SAFETY: valid component; index below the bus count; `info` is writable.
            let got = unsafe {
                self.component.getBusInfo(sv::MediaTypes_::kAudio as sv::MediaType, sv::BusDirections_::kInput as sv::BusDirection, i, &mut info)
            };
            ok(got) && info.busType == sv::BusTypes_::kAux as sv::BusType
        })
    }

    /// Stops processing, deactivates, negotiates the bus layout (main buses mono or stereo as
    /// asked, falling back to stereo and then to the plugin's own choice), activates the main
    /// buses, sets up 32-bit processing and activates the component.
    pub fn configure(&mut self, sample_rate: f64, max_frames: usize, mono: bool) -> Result<Layout, Vst3Error> {
        self.shutdown();
        let audio = sv::MediaTypes_::kAudio;
        let event = sv::MediaTypes_::kEvent;
        let (din, dout) = (sv::BusDirections_::kInput, sv::BusDirections_::kOutput);
        let nin = self.bus_count(audio, din);
        let nout = self.bus_count(audio, dout);
        let current = |s: &Self, dir: sv::BusDirections, n: i32| -> Vec<sv::SpeakerArrangement> {
            (0..n).map(|i| s.arrangement(dir, i).unwrap_or(sv::SpeakerArr::kStereo)).collect()
        };
        let want = if mono { sv::SpeakerArr::kMono } else { sv::SpeakerArr::kStereo };
        let p = self.processor().ok_or_else(|| Vst3Error::Process("no audio processor".into()))?.clone();
        let aux = self.aux_input_bus(nin);
        let mut tried_ok = false;
        'arrangements: for main in [want, sv::SpeakerArr::kStereo] {
            // The sidechain bus follows the main bus; failing that, it keeps its own arrangement.
            for follow in [true, false] {
                let mut ins = current(self, din, nin);
                let mut outs = current(self, dout, nout);
                if let Some(a) = ins.first_mut() {
                    *a = main;
                }
                if let Some(a) = outs.first_mut() {
                    *a = main;
                }
                if follow {
                    if let Some(a) = aux.and_then(|i| ins.get_mut(usize::try_from(i).unwrap_or(usize::MAX))) {
                        *a = main;
                    }
                } else if aux.is_none() {
                    break;
                }
                // SAFETY: valid processor; the arrays hold exactly `nin`/`nout` arrangements and
                // outlive the call.
                if ok(unsafe { p.setBusArrangements(ins.as_mut_ptr(), nin, outs.as_mut_ptr(), nout) }) {
                    tried_ok = true;
                    break 'arrangements;
                }
            }
            if main == sv::SpeakerArr::kStereo {
                break;
            }
        }
        if !tried_ok {
            log::debug!("vst3: plugin refused mono/stereo bus arrangements; using its own");
            if aux.is_some() {
                log::warn!("vst3: plugin refused an arrangement for its sidechain input; the key is not delivered");
            }
        }
        let inputs: Vec<u32> = (0..nin).map(|i| self.bus_channels(din, i)).collect();
        let key_bus = aux.filter(|_| tried_ok).and_then(|i| usize::try_from(i).ok()).filter(|&i| inputs.get(i).is_some_and(|&c| c > 0));
        let layout =
            Layout { inputs, outputs: (0..nout).map(|i| self.bus_channels(dout, i)).collect(), event_input: self.bus_count(event, din) > 0, key_bus };
        // Main buses on, plus the sidechain bus when it has an arrangement; other aux buses off
        // (VST3 still gets buffers for every bus).
        for (dir, n) in [(din, nin), (dout, nout)] {
            for i in 0..n {
                let on = i == 0 || (dir == din && layout.key_bus == usize::try_from(i).ok());
                // SAFETY: valid component; index below the bus count; the component is inactive here
                // (`shutdown` above, `setActive(1)` below), as the SDK requires for bus activation.
                unsafe { self.component.activateBus(audio as sv::MediaType, dir as sv::BusDirection, i, u8::from(on)) };
            }
        }
        if layout.event_input {
            // SAFETY: valid component; event input bus 0 exists.
            unsafe { self.component.activateBus(event as sv::MediaType, din as sv::BusDirection, 0, 1) };
        }
        self.event_in = layout.event_input;
        // SDK enum constants are signed on Windows and unsigned on Unix.
        #[allow(clippy::unnecessary_cast)]
        let sample32 = sv::SymbolicSampleSizes_::kSample32 as i32;
        // SAFETY: valid processor.
        if !ok(unsafe { p.canProcessSampleSize(sample32) }) {
            return Err(Vst3Error::Process("plugin cannot process 32-bit float".into()));
        }
        let max = i32::try_from(max_frames).unwrap_or(i32::MAX);
        let mut setup = sv::ProcessSetup {
            #[allow(clippy::unnecessary_cast)] // SDK enum signedness differs by target.
            processMode: sv::ProcessModes_::kRealtime as i32,
            symbolicSampleSize: sample32,
            maxSamplesPerBlock: max,
            sampleRate: sample_rate,
        };
        // SAFETY: valid processor, inactive; `setup` is a valid struct for the call.
        if !ok(unsafe { p.setupProcessing(&mut setup) }) {
            return Err(Vst3Error::Process("setupProcessing failed".into()));
        }
        // SAFETY: valid component, configured above.
        if !ok(unsafe { self.component.setActive(1) }) {
            return Err(Vst3Error::Process("setActive failed".into()));
        }
        self.active = true;
        self.max_frames = max_frames;
        self.context.sampleRate = sample_rate;
        self.context.tempo = 120.0;
        self.context.timeSigNumerator = 4;
        self.context.timeSigDenominator = 4;
        // The flag constants are the C default enum type: i32 on Windows, u32 elsewhere.
        #[allow(clippy::unnecessary_cast)]
        let flags = (sv::ProcessContext_::StatesAndFlags_::kTempoValid | sv::ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32;
        self.context.state = flags;
        self.host.latency_changed.store(false, Ordering::Relaxed);
        Ok(layout)
    }

    /// `setProcessing(false)` + `setActive(false)` when needed.
    pub fn shutdown(&mut self) {
        if self.processing {
            if let Some(p) = self.processor() {
                // SAFETY: valid processor in the processing state.
                unsafe { p.setProcessing(0) };
            }
            self.processing = false;
        }
        if self.active {
            // SAFETY: valid, active component.
            unsafe { self.component.setActive(0) };
            self.active = false;
        }
    }

    /// Runs one `process` call over `frames` frames starting at block time `offset`: `points`
    /// (all inside this call) become one parameter queue each, ascending (see
    /// [`ParamChanges::fill`]; `points` is sorted in place), and `notes` (sorted by time) have
    /// their times made relative to `offset`. Returns the parameter points dropped because the
    /// slab ran out.
    pub fn process(
        &mut self,
        frames: usize,
        inputs: &mut BusBuffers,
        outputs: &mut BusBuffers,
        points: &mut [ParamPoint],
        notes: &[NoteEvent],
        offset: u32,
        project_time: i64,
        transport: Option<&Transport>,
    ) -> Result<u32, sb::tresult> {
        if !self.active {
            return Err(sb::kNotInitialized);
        }
        if frames == 0 || frames > self.max_frames || frames > inputs.frames || frames > outputs.frames {
            return Err(sb::kInvalidArgument);
        }
        let p = self.processor.clone().ok_or(sb::kNoInterface)?;
        if !self.processing {
            // Many plugins return kNotImplemented here; that is not an error.
            // SAFETY: valid, active processor.
            unsafe { p.setProcessing(1) };
            self.processing = true;
        }
        let dropped = self.in_params.fill(points, offset);
        self.out_params.clear();
        let event_in = self.event_in;
        self.in_events.with(|v| {
            v.clear();
            if event_in {
                for n in notes.iter().take(EVENT_CAP) {
                    v.push(n.to_raw(offset));
                }
            }
        });
        self.out_events.with(Vec::clear);
        inputs.refresh();
        outputs.refresh();
        self.context.continousTimeSamples = project_time;
        match transport {
            Some(t) => apply_transport(&mut self.context, t),
            None => self.context.projectTimeSamples = project_time,
        }
        let mut data = sv::ProcessData {
            #[allow(clippy::unnecessary_cast)] // SDK enum signedness differs by target.
            processMode: sv::ProcessModes_::kRealtime as i32,
            #[allow(clippy::unnecessary_cast)] // SDK enum signedness differs by target.
            symbolicSampleSize: sv::SymbolicSampleSizes_::kSample32 as i32,
            numSamples: i32::try_from(frames).unwrap_or(0),
            numInputs: i32::try_from(inputs.bufs.len()).unwrap_or(0),
            numOutputs: i32::try_from(outputs.bufs.len()).unwrap_or(0),
            inputs: if inputs.bufs.is_empty() { std::ptr::null_mut() } else { inputs.bufs.as_mut_ptr() },
            outputs: if outputs.bufs.is_empty() { std::ptr::null_mut() } else { outputs.bufs.as_mut_ptr() },
            inputParameterChanges: self.in_params.as_com_ref::<sv::IParameterChanges>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            outputParameterChanges: self.out_params.as_com_ref::<sv::IParameterChanges>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            inputEvents: self.in_events.as_com_ref::<sv::IEventList>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            outputEvents: self.out_events.as_com_ref::<sv::IEventList>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            processContext: &mut self.context,
        };
        // SAFETY: valid, active processor. Every channel pointer refers to `frames` (or more) valid
        // f32s owned by `inputs`/`outputs`, borrowed for the whole call; the bus tables, the
        // parameter/event objects and the context are owned by `self` and untouched until the
        // call returns.
        let r = unsafe { p.process(&mut data) };
        if ok(r) { Ok(dropped) } else { Err(r) }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Close any editor and cut its handles off before the controller goes away.
        if self.editor.kill() {
            self.bundle.mark_leaked();
        }
        self.shutdown();
        if let Some((a, b)) = self.connections.take() {
            // SAFETY: the pair was connected in `instantiate`; disconnect before terminating.
            unsafe {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
        }
        if let Some(c) = self.controller.take() {
            // SAFETY: valid controller; detach our handler before it goes away.
            unsafe { c.setComponentHandler(std::ptr::null_mut()) };
            if self.controller_initialized {
                // SAFETY: separately created controller, initialised once; terminated once.
                unsafe { c.terminate() };
            }
        }
        self.processor = None;
        if self.component_initialized {
            // SAFETY: valid component, inactive; terminated exactly once.
            unsafe { self.component.terminate() };
            self.component_initialized = false;
        }
    }
}

/// Splits a state blob into (component, controller) parts. A blob without SoundCraft's container
/// header is taken as a bare component state.
pub(super) fn split_state(data: &[u8]) -> Option<(&[u8], &[u8])> {
    crate::preset::split_state(data)
}
