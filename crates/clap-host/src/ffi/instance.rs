//! A live plug-in instance: port, parameter and note-port info, and the plug-in vtable calls.

use super::*;

// ---- instances -----------------------------------------------------------------------------

/// What one audio port looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PortInfo {
    pub channels: u32,
    pub main: bool,
}

/// One parameter as reported by `clap_plugin_params.get_info`.
#[derive(Debug, Clone)]
pub(crate) struct RawParam {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub flags: u32,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// Note dialects of the plugin's first input note port.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NotePort {
    pub supported: u32,
    pub preferred: u32,
}

/// The plug-in's reported tail through `clap.tail`, if it offers the extension and the call succeeds.
///
/// # Safety
/// `ext` is null or points to a valid `clap_plugin_tail` whose `get` accepts `plugin`.
pub(super) unsafe fn read_tail(ext: *const clap_plugin_tail, plugin: *const clap_plugin) -> Option<u32> {
    // SAFETY: caller contract.
    let f = unsafe { ext.as_ref() }.and_then(|t| t.get)?;
    // SAFETY: caller contract.
    Some(unsafe { f(plugin) })
}

/// A live plugin instance plus the host struct it was created with.
pub(crate) struct Instance {
    pub(super) plugin: *const clap_plugin,
    pub(super) params: *const clap_plugin_params,
    pub(super) ports: *const clap_plugin_audio_ports,
    pub(super) latency: *const clap_plugin_latency,
    pub(super) tail: *const clap_plugin_tail,
    pub(super) notes: *const clap_plugin_note_ports,
    pub(super) state_ext: *const clap_plugin_state,
    pub(super) gui: Arc<GuiLink>,
    pub(super) raw: Vec<RawEvent>,
    /// Parameter values the plugin reported through output events in the current `process`.
    pub(super) out_params: Vec<(u32, f64)>,
    pub(super) active: bool,
    pub(super) processing: bool,
    pub(super) max_frames: usize,
    pub(super) created: bool,
    // Field order matters for drop: the plugin is destroyed in `Drop::drop` first; then the host
    // struct and its state, then the bundle (which keeps the code mapped) go.
    pub(super) _host: Box<clap_host>,
    pub(super) state: Arc<HostState>,
    pub(super) _bundle: Arc<Bundle>,
}

// SAFETY: an `Instance` is used through `&mut self` only, so calls into the plugin are always
// serialised. CLAP lets audio-thread functions run on any one thread at a time; SoundCraft creates
// instances on the engine/UI thread and then moves them into the mix engine (documented
// limitation: some main-thread calls such as `activate` may then happen on the audio thread).
unsafe impl Send for Instance {}

impl Instance {
    pub(super) fn vt(&self) -> clap_plugin {
        // SAFETY: `plugin` is non-null (checked at creation) and alive until `Drop`.
        unsafe { *self.plugin }
    }

    pub(super) fn extension<T>(&self, id: &CStr) -> *const T {
        match self.vt().get_extension {
            // SAFETY: valid instance; `id` is a static NUL-terminated string.
            Some(f) => unsafe { f(self.plugin, id.as_ptr()) }.cast::<T>(),
            None => std::ptr::null(),
        }
    }

    pub fn state(&self) -> &HostState {
        &self.state
    }

    /// A counted reference to the request flags, for the audio side and the main-thread service.
    pub fn host_state(&self) -> Arc<HostState> {
        Arc::clone(&self.state)
    }

    /// The editor/state link, whether or not the plugin has an editor (its `idle` also runs a
    /// requested `on_main_thread`).
    pub fn link(&self) -> Arc<GuiLink> {
        Arc::clone(&self.gui)
    }

    /// Between `start_processing` and `stop_processing`.
    pub fn is_processing(&self) -> bool {
        self.processing
    }

    /// The editor link shared with editor handles (`None` when the plugin has no `clap.gui`).
    pub fn gui(&self) -> Option<Arc<GuiLink>> {
        self.gui.available().then(|| Arc::clone(&self.gui))
    }

    /// The same link, for state handles (`None` when the plugin has no `clap.state`).
    pub fn state_link(&self) -> Option<Arc<GuiLink>> {
        (!self.state_ext.is_null()).then(|| Arc::clone(&self.gui))
    }

    /// `clap_plugin_state.load` from memory. `false` without the extension or when refused.
    pub fn load_state(&self, data: &[u8]) -> bool {
        // SAFETY: the extension pointer, when non-null, points to a static vtable in the plugin.
        let Some(load) = (unsafe { self.state_ext.as_ref() }).and_then(|s| s.load) else { return false };
        let mut cursor = ReadCursor { data, pos: 0 };
        let stream = clap_istream { ctx: (&mut cursor as *mut ReadCursor<'_>).cast::<c_void>(), read: Some(istream_read) };
        // SAFETY: valid instance; the stream and its cursor live on this frame for the call.
        unsafe { load(self.plugin, &stream) }
    }

    /// Runs `on_main_thread` if the plugin asked for a callback.
    pub fn service_callback(&mut self) {
        if self.state.callback.swap(false, Ordering::Relaxed)
            && let Some(f) = self.vt().on_main_thread
        {
            // SAFETY: valid instance; the plugin asked for this call via `request_callback`.
            unsafe { f(self.plugin) };
        }
    }

    pub fn params(&self) -> Vec<RawParam> {
        // SAFETY: the extension pointer, when non-null, points to a static vtable in the plugin.
        let Some(p) = (unsafe { self.params.as_ref() }) else { return Vec::new() };
        let (Some(count), Some(get_info)) = (p.count, p.get_info) else { return Vec::new() };
        // SAFETY: vtable call with the instance it belongs to.
        let n = unsafe { count(self.plugin) }.min(MAX_PARAMS);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: `clap_param_info` is plain data (integers, floats, char arrays, a nullable
            // pointer); all-zero is a valid value.
            let mut info: clap_param_info = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is a valid, writable struct.
            if !unsafe { get_info(self.plugin, i, &mut info) } {
                continue;
            }
            let (min, max) = (info.min_value, info.max_value);
            if !(min.is_finite() && max.is_finite()) || max < min {
                continue;
            }
            let default = if info.default_value.is_finite() { info.default_value.clamp(min, max) } else { min };
            out.push(RawParam { id: info.id, name: read_name(&info.name), module: read_name(&info.module), flags: info.flags, min, max, default });
        }
        out
    }

    pub fn param_value(&self, id: u32) -> Option<f64> {
        // SAFETY: see `params`.
        let p = unsafe { self.params.as_ref() }?;
        let get = p.get_value?;
        let mut v = 0.0f64;
        // SAFETY: valid instance; `v` is writable.
        (unsafe { get(self.plugin, id, &mut v) } && v.is_finite()).then_some(v)
    }

    pub fn value_text(&self, id: u32, value: f64) -> Option<String> {
        // SAFETY: see `params`.
        let p = unsafe { self.params.as_ref() }?;
        let f = p.value_to_text?;
        let mut buf = [0 as c_char; 128];
        // SAFETY: valid instance; we pass the buffer's true capacity.
        let ok = unsafe { f(self.plugin, id, value, buf.as_mut_ptr(), buf.len() as u32) };
        ok.then(|| read_name(&buf)).filter(|s| !s.is_empty())
    }

    pub fn audio_ports(&self, is_input: bool) -> Vec<PortInfo> {
        // SAFETY: see `params`.
        let Some(p) = (unsafe { self.ports.as_ref() }) else { return Vec::new() };
        let (Some(count), Some(get)) = (p.count, p.get) else { return Vec::new() };
        // SAFETY: vtable call with its instance.
        let n = unsafe { count(self.plugin, is_input) }.min(MAX_PORTS);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: plain data; all-zero (null port_type pointer) is valid.
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            if unsafe { get(self.plugin, i, is_input, &mut info) } {
                out.push(PortInfo { channels: info.channel_count.min(MAX_PORT_CHANNELS), main: info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0 });
            } else {
                // Keep port indices aligned: an unreadable port still needs a (silent) buffer slot.
                out.push(PortInfo { channels: 0, main: false });
            }
        }
        out
    }

    pub fn note_port(&self) -> Option<NotePort> {
        // SAFETY: see `params`.
        let p = unsafe { self.notes.as_ref() }?;
        let (count, get) = (p.count?, p.get?);
        // SAFETY: vtable call with its instance.
        if unsafe { count(self.plugin, true) } == 0 {
            return None;
        }
        // SAFETY: plain data; all-zero is valid.
        let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
        // SAFETY: index 0 is below the non-zero count; `info` is writable.
        unsafe { get(self.plugin, 0, true, &mut info) }.then_some(NotePort { supported: info.supported_dialects, preferred: info.preferred_dialect })
    }

    pub fn latency(&self) -> u32 {
        // SAFETY: see `params`.
        match unsafe { self.latency.as_ref() }.and_then(|l| l.get) {
            // SAFETY: valid instance.
            Some(f) => unsafe { f(self.plugin) },
            None => 0,
        }
    }

    /// The plugin's tail in samples from the `clap.tail` extension (`get` is callable from the
    /// audio thread), as reported: `i32::MAX` and above mean an infinite tail. `None` without the
    /// extension.
    pub fn tail(&self) -> Option<u32> {
        // SAFETY: `tail` is null or the plugin's extension struct, valid for the instance's life.
        unsafe { read_tail(self.tail, self.plugin) }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn activate(&mut self, sample_rate: f64, min_frames: u32, max_frames: u32) -> Result<(), ClapError> {
        self.deactivate();
        let f = self.vt().activate.ok_or_else(|| ClapError::Process("plugin has no activate".into()))?;
        // SAFETY: valid, inactive instance; arguments are sane (checked by the caller).
        if !unsafe { f(self.plugin, sample_rate, min_frames, max_frames) } {
            return Err(ClapError::Process("activate failed".into()));
        }
        self.active = true;
        self.max_frames = max_frames as usize;
        Ok(())
    }

    pub fn deactivate(&mut self) {
        self.stop_processing();
        if self.active {
            if let Some(f) = self.vt().deactivate {
                // SAFETY: valid, active instance.
                unsafe { f(self.plugin) };
            }
            self.active = false;
        }
    }

    /// CLAP `stop_processing` (audio thread) when processing.
    pub fn stop_processing(&mut self) {
        if self.processing {
            if let Some(f) = self.vt().stop_processing {
                // SAFETY: valid instance in the processing state.
                unsafe { f(self.plugin) };
            }
            self.processing = false;
        }
    }

    pub fn reset(&mut self) {
        if self.active
            && let Some(f) = self.vt().reset
        {
            // SAFETY: valid, active instance.
            unsafe { f(self.plugin) };
        }
    }

    /// Runs one `clap_process` call over `frames` frames. Events must be sorted by time; their
    /// times are made relative to `offset`. Returns the CLAP process status.
    pub fn process(
        &mut self,
        frames: usize,
        steady_time: i64,
        inputs: &mut PortBuffers,
        outputs: &mut PortBuffers,
        events: &[Event],
        offset: u32,
        transport: Option<&Transport>,
    ) -> Result<i32, &'static str> {
        if !self.active {
            return Err("not active");
        }
        if frames == 0 || frames > self.max_frames || frames > inputs.frames || frames > outputs.frames {
            return Err("block larger than the activated size");
        }
        if !self.processing {
            let ok = match self.vt().start_processing {
                // SAFETY: valid, active instance.
                Some(f) => unsafe { f(self.plugin) },
                None => true,
            };
            if !ok {
                return Err("start_processing failed");
            }
            self.processing = true;
        }
        let process = self.vt().process.ok_or("plugin has no process")?;
        self.raw.clear();
        for e in events.iter().take(self.raw.capacity()) {
            self.raw.push(RawEvent::from_event(e, offset));
        }
        inputs.refresh();
        outputs.refresh();
        let in_events = clap_input_events {
            ctx: (&self.raw as *const Vec<RawEvent>).cast_mut().cast::<c_void>(),
            size: Some(in_events_size),
            get: Some(in_events_get),
        };
        self.out_params.clear();
        let out_events =
            clap_output_events { ctx: (&mut self.out_params as *mut Vec<(u32, f64)>).cast::<c_void>(), try_push: Some(out_events_try_push) };
        let transport = transport.map(transport_event);
        let p = clap_process {
            steady_time,
            frames_count: u32::try_from(frames).unwrap_or(0),
            transport: transport.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
            audio_inputs: inputs.bufs.as_ptr(),
            audio_outputs: outputs.bufs.as_mut_ptr(),
            audio_inputs_count: u32::try_from(inputs.bufs.len()).unwrap_or(0),
            audio_outputs_count: u32::try_from(outputs.bufs.len()).unwrap_or(0),
            in_events: &in_events,
            out_events: &out_events,
        };
        // SAFETY: valid, processing instance. Every buffer pointer refers to `frames` (or more)
        // valid f32s owned by `inputs`/`outputs`, which are borrowed for the whole call; the event
        // lists and their contexts and the transport live on this stack frame / in `self.raw`,
        // untouched until the call returns.
        let status = unsafe { process(self.plugin, &p) };
        if !self.out_params.is_empty()
            && let Ok(mut edits) = self.state.edits.try_lock()
        {
            // Never blocks or grows: a busy UI or a full queue drops the edits.
            for &e in &self.out_params {
                if edits.len() < edits.capacity() {
                    edits.push(e);
                }
            }
        }
        self.out_params.clear();
        if status == CLAP_PROCESS_ERROR { Err("plugin reported a processing error") } else { Ok(status) }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Editors first (CLAP: destroy the GUI before the plugin), and no handle may call in after.
        self.gui.kill();
        self.deactivate();
        if self.created
            && let Some(f) = self.vt().destroy
        {
            // SAFETY: valid instance, deactivated above; destroyed exactly once.
            unsafe { f(self.plugin) };
        }
        self.created = false;
        // After `destroy`: the plug-in had its chance to unregister its timers and descriptors.
        #[cfg(target_os = "linux")]
        self.state.close_run_loop();
    }
}
