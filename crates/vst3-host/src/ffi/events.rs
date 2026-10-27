//! Event and parameter queues, note events and audio bus buffers handed to `process`.

use super::*;

/// One parameter point the host sends: its time in the block, the parameter, its class
/// (0: automation and host values, 1: a mapped MIDI controller, which follows automation at the
/// same offset), the order it was set in, and the normalized value. `release`: a controller
/// from a release-class message ([`Message::survives_reset`](transit_dsp::event::Message)),
/// which the host keeps through a plug-in reset.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct ParamPoint {
    pub time: u32,
    pub id: u32,
    pub class: u8,
    pub seq: u32,
    pub value: f64,
    pub release: bool,
}

/// A queue's points live inline (one point a plug-in wrote), not in the slab.
const INLINE: u32 = u32::MAX;

/// The points one block carries: (offset, value bits).
type Slab = Arc<[(AtomicI32, AtomicU64)]>;

/// One parameter's points for one block: a contiguous range of its [`ParamChanges`]'s slab,
/// ascending by offset (host → plug-in), or one inline point (a plug-in's output changes; only the
/// last one it adds is kept).
pub(crate) struct ParamQueue {
    pub(super) id: AtomicU32,
    /// First slab point, or [`INLINE`].
    start: AtomicU32,
    pub(super) points: AtomicI32,
    offset: AtomicI32,
    value: AtomicU64,
    slab: Slab,
}

impl ParamQueue {
    fn new(slab: Slab) -> ParamQueue {
        ParamQueue {
            id: AtomicU32::new(0),
            start: AtomicU32::new(INLINE),
            points: AtomicI32::new(0),
            offset: AtomicI32::new(0),
            value: AtomicU64::new(0),
            slab,
        }
    }

    /// Point `index`: (offset, value).
    pub(super) fn point(&self, index: i32) -> Option<(i32, f64)> {
        if index < 0 || index >= self.points.load(Ordering::Relaxed) {
            return None;
        }
        let start = self.start.load(Ordering::Relaxed);
        if start == INLINE {
            return (index == 0).then(|| (self.offset.load(Ordering::Relaxed), f64::from_bits(self.value.load(Ordering::Relaxed))));
        }
        let (o, v) = self.slab.get((start as usize).checked_add(index as usize)?)?;
        Some((o.load(Ordering::Relaxed), f64::from_bits(v.load(Ordering::Relaxed))))
    }
}

impl Class for ParamQueue {
    type Interfaces = (sv::IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> sv::ParamID {
        self.id.load(Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> sb::int32 {
        self.points.load(Ordering::Relaxed)
    }
    unsafe fn getPoint(&self, index: sb::int32, offset: *mut sb::int32, value: *mut sv::ParamValue) -> sb::tresult {
        let Some((at, v)) = self.point(index) else { return sb::kInvalidArgument };
        // SAFETY: out-pointers from the plugin (or null, handled).
        if let (Some(o), Some(out)) = (unsafe { offset.as_mut() }, unsafe { value.as_mut() }) {
            *o = at;
            *out = v;
            sb::kResultOk
        } else {
            sb::kInvalidArgument
        }
    }
    unsafe fn addPoint(&self, offset: sb::int32, value: sv::ParamValue, index: *mut sb::int32) -> sb::tresult {
        self.offset.store(offset, Ordering::Relaxed);
        self.value.store(value.to_bits(), Ordering::Relaxed);
        self.start.store(INLINE, Ordering::Relaxed);
        self.points.store(1, Ordering::Relaxed);
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(i) = unsafe { index.as_mut() } {
            *i = 0;
        }
        sb::kResultOk
    }
}

/// A fixed pool of parameter queues (`IParameterChanges`) and the slab of their points, reused
/// every block. Nothing here allocates after `new`.
pub(crate) struct ParamChanges {
    pub(super) queues: Vec<ComWrapper<ParamQueue>>,
    count: AtomicI32,
    slab: Slab,
    /// Slab points handed out this block.
    used: AtomicUsize,
}

impl Class for ParamChanges {
    type Interfaces = (sv::IParameterChanges,);
}

impl ParamChanges {
    /// `cap` queues sharing a slab of `points` points (0 for output changes, which keep one
    /// inline point per queue).
    pub(super) fn new(cap: usize, points: usize) -> ParamChanges {
        let slab: Slab = (0..points).map(|_| (AtomicI32::new(0), AtomicU64::new(0))).collect::<Vec<_>>().into();
        ParamChanges {
            queues: (0..cap).map(|_| ComWrapper::new(ParamQueue::new(Arc::clone(&slab)))).collect(),
            count: AtomicI32::new(0),
            slab,
            used: AtomicUsize::new(0),
        }
    }

    pub(super) fn clear(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.used.store(0, Ordering::Relaxed);
    }

    /// Finds or adds the queue for `id`; `None` when the pool is full.
    fn queue_for(&self, id: u32) -> Option<(i32, &ParamQueue)> {
        let n = self.count.load(Ordering::Relaxed).max(0);
        for (i, q) in self.queues.iter().enumerate().take(n as usize) {
            if q.id.load(Ordering::Relaxed) == id {
                return Some((i32::try_from(i).ok()?, q));
            }
        }
        let q = self.queues.get(n as usize)?;
        q.id.store(id, Ordering::Relaxed);
        q.start.store(INLINE, Ordering::Relaxed);
        q.points.store(0, Ordering::Relaxed);
        self.count.store(n + 1, Ordering::Relaxed);
        Some((n, q))
    }

    /// Host side: this block's points (times relative to `offset`, all inside the block) become
    /// one queue per parameter with its points ascending by offset. At one offset the last point
    /// wins (a controller after automation). When the slab runs out, a parameter keeps only its
    /// last point. Sorts `points` in place (`sort_unstable_by_key`: no allocation). Returns the
    /// points dropped (slab or queue pool exhausted).
    pub(crate) fn fill(&self, points: &mut [ParamPoint], offset: u32) -> u32 {
        self.clear();
        points.sort_unstable_by_key(|p| (p.id, p.time, p.class, p.seq));
        let mut groups = points.chunk_by(|a, b| a.id == b.id).count();
        let mut dropped = 0u32;
        for group in points.chunk_by(|a, b| a.id == b.id) {
            groups -= 1;
            let Some(first) = group.first() else { continue };
            // The last point at each offset.
            let last_at = |k: usize| group.get(k + 1).is_none_or(|n| n.time != group.get(k).map_or(0, |p| p.time));
            let distinct = (0..group.len()).filter(|&k| last_at(k)).count();
            let Some((_, q)) = self.queue_for(first.id) else {
                dropped = dropped.saturating_add(distinct as u32);
                continue;
            };
            let start = self.used.load(Ordering::Relaxed);
            // Keep one slab point for each parameter still to come.
            let room = self.slab.len().saturating_sub(start).saturating_sub(groups);
            let all = distinct <= room;
            let mut w = 0usize;
            for (k, p) in group.iter().enumerate() {
                if !last_at(k) || (!all && k + 1 != group.len()) {
                    continue;
                }
                let Some((o, v)) = self.slab.get(start + w) else { break };
                o.store(i32::try_from(p.time.saturating_sub(offset)).unwrap_or(i32::MAX), Ordering::Relaxed);
                v.store(p.value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                w += 1;
            }
            dropped = dropped.saturating_add((distinct - w) as u32);
            q.start.store(u32::try_from(start).unwrap_or(INLINE), Ordering::Relaxed);
            q.points.store(i32::try_from(w).unwrap_or(0), Ordering::Relaxed);
            self.used.store(start + w, Ordering::Relaxed);
        }
        dropped
    }

    fn queue_ptr(&self, index: usize) -> *mut sv::IParamValueQueue {
        self.queues.get(index).and_then(|q| q.as_com_ref::<sv::IParamValueQueue>()).map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> sb::int32 {
        self.count.load(Ordering::Relaxed)
    }
    unsafe fn getParameterData(&self, index: sb::int32) -> *mut sv::IParamValueQueue {
        if index < 0 || index >= self.count.load(Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        self.queue_ptr(index as usize)
    }
    unsafe fn addParameterData(&self, id: *const sv::ParamID, index: *mut sb::int32) -> *mut sv::IParamValueQueue {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(&id) = (unsafe { id.as_ref() }) else { return std::ptr::null_mut() };
        let Some((i, _)) = self.queue_for(id) else { return std::ptr::null_mut() };
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(out) = unsafe { index.as_mut() } {
            *out = i;
        }
        self.queue_ptr(i as usize)
    }
}

/// A bounded event list (`IEventList`). Only ever touched by one thread at a time (the host
/// fills it, then the plugin reads it inside `process`), so the lock is never contended; the
/// plugin side uses `try_lock` and never blocks.
pub(crate) struct EventList {
    events: Mutex<Vec<sv::Event>>,
    cap: usize,
}

impl Class for EventList {
    type Interfaces = (sv::IEventList,);
}

impl EventList {
    pub(super) fn new(cap: usize) -> EventList {
        let list = EventList { events: Mutex::new(Vec::with_capacity(cap)), cap };
        // On macOS the first lock boxes the mutex's pthread_mutex_t (64 bytes). Take it once here,
        // on the control thread, so `process` never allocates (see the audio-thread knowledge note).
        drop(list.events.lock());
        list
    }
    pub(super) fn with<R>(&self, f: impl FnOnce(&mut Vec<sv::Event>) -> R) -> Option<R> {
        match self.events.try_lock() {
            Ok(mut g) => Some(f(&mut g)),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(f(&mut p.into_inner())),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> sb::int32 {
        self.with(|v| i32::try_from(v.len()).unwrap_or(i32::MAX)).unwrap_or(0)
    }
    unsafe fn getEvent(&self, index: sb::int32, e: *mut sv::Event) -> sb::tresult {
        // SAFETY: out-pointer from the plugin (or null, handled).
        let Some(out) = (unsafe { e.as_mut() }) else { return sb::kInvalidArgument };
        let Ok(i) = usize::try_from(index) else { return sb::kInvalidArgument };
        match self.with(|v| v.get(i).copied()).flatten() {
            Some(ev) => {
                *out = ev;
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, e: *mut sv::Event) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(&ev) = (unsafe { e.as_ref() }) else { return sb::kInvalidArgument };
        let cap = self.cap;
        // Output events (e.g. MIDI out) are accepted while there is room; SoundCraft ignores them.
        match self.with(|v| {
            if v.len() < cap {
                v.push(ev);
                true
            } else {
                false
            }
        }) {
            Some(true) => sb::kResultOk,
            _ => sb::kResultFalse,
        }
    }
}

// ---- events and buffers --------------------------------------------------------------------

/// A host → plugin note event, safe to build anywhere; converted inside `process`. `channel` is
/// the MIDI channel (0..=15) and `id` the host's voice id (`-1`: none), as VST3's `noteId`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NoteEvent {
    On { time: u32, key: u8, velocity: f32, channel: u8, id: i32 },
    Off { time: u32, key: u8, velocity: f32, channel: u8, id: i32 },
    Pressure { time: u32, key: u8, pressure: f32, channel: u8, id: i32 },
}

impl NoteEvent {
    pub fn time(&self) -> u32 {
        match *self {
            NoteEvent::On { time, .. } | NoteEvent::Off { time, .. } | NoteEvent::Pressure { time, .. } => time,
        }
    }
    pub fn set_time(&mut self, t: u32) {
        match self {
            NoteEvent::On { time, .. } | NoteEvent::Off { time, .. } | NoteEvent::Pressure { time, .. } => *time = t,
        }
    }

    pub(super) fn to_raw(self, offset: u32) -> sv::Event {
        // SAFETY: `Event` is plain data (numbers and a union of plain structs); all-zero is valid.
        let mut e: sv::Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = i32::try_from(self.time().saturating_sub(offset)).unwrap_or(0);
        e.flags = sv::Event_::EventFlags_::kIsLive as u16;
        match self {
            NoteEvent::On { key, velocity, channel, id, .. } => {
                e.r#type = sv::Event_::EventTypes_::kNoteOnEvent as u16;
                e.__field0.noteOn =
                    sv::NoteOnEvent { channel: i16::from(channel & 0x0F), pitch: i16::from(key), tuning: 0.0, velocity, length: 0, noteId: id };
            }
            NoteEvent::Off { key, velocity, channel, id, .. } => {
                e.r#type = sv::Event_::EventTypes_::kNoteOffEvent as u16;
                e.__field0.noteOff =
                    sv::NoteOffEvent { channel: i16::from(channel & 0x0F), pitch: i16::from(key), velocity, noteId: id, tuning: 0.0 };
            }
            NoteEvent::Pressure { key, pressure, channel, id, .. } => {
                e.r#type = sv::Event_::EventTypes_::kPolyPressureEvent as u16;
                e.__field0.polyPressure = sv::PolyPressureEvent { channel: i16::from(channel & 0x0F), pitch: i16::from(key), pressure, noteId: id };
            }
        }
        e
    }
}

/// Planar f32 buffers for every audio bus in one direction, with the pointer tables VST3 needs.
/// Allocated in `new`; `process` only rewrites pointers in place (no allocation).
pub(crate) struct BusBuffers {
    data: Vec<Vec<Vec<f32>>>,
    ptrs: Vec<Vec<*mut f32>>,
    pub(super) bufs: Vec<sv::AudioBusBuffers>,
    pub(super) frames: usize,
    /// Every channel of the next block is silent: `refresh` sets each bus's `silenceFlags` to all ones.
    silent: bool,
    /// Per-bus silence set by [`BusBuffers::set_bus_silent`]; wins over `silent` for that bus.
    bus_silent: Vec<Option<bool>>,
}

// SAFETY: the raw pointers only ever point into `data`, which this struct owns; moving the struct
// to another thread moves that ownership with it.
unsafe impl Send for BusBuffers {}

impl BusBuffers {
    /// One buffer set per bus, `channels[b]` channels each (capped), `frames` long.
    pub fn new(channels: &[u32], frames: usize) -> BusBuffers {
        let data: Vec<Vec<Vec<f32>>> =
            channels.iter().take(MAX_BUSES as usize).map(|&c| (0..c.min(MAX_BUS_CHANNELS)).map(|_| vec![0.0; frames]).collect()).collect();
        let ptrs = data.iter().map(|p| vec![std::ptr::null_mut(); p.len()]).collect();
        let bufs = data
            .iter()
            .map(|_| sv::AudioBusBuffers {
                numChannels: 0,
                silenceFlags: 0,
                __field0: sv::AudioBusBuffers__type0 { channelBuffers32: std::ptr::null_mut() },
            })
            .collect();
        let bus_silent = vec![None; data.len()];
        BusBuffers { data, ptrs, bufs, frames, silent: false, bus_silent }
    }

    pub fn channels(&self, bus: usize) -> usize {
        self.data.get(bus).map_or(0, Vec::len)
    }

    pub fn channel(&self, bus: usize, ch: usize) -> Option<&[f32]> {
        self.data.get(bus).and_then(|p| p.get(ch)).map(Vec::as_slice)
    }

    pub fn channel_mut(&mut self, bus: usize, ch: usize) -> Option<&mut [f32]> {
        self.data.get_mut(bus).and_then(|p| p.get_mut(ch)).map(Vec::as_mut_slice)
    }

    /// Flags every channel of the next block silent, or clears the flags (every bus, including
    /// any set by [`BusBuffers::set_bus_silent`]).
    pub fn set_silent(&mut self, silent: bool) {
        self.silent = silent;
        self.bus_silent.iter_mut().for_each(|b| *b = None);
    }

    /// Flags one bus silent, or clears its flags, whatever [`BusBuffers::set_silent`] said.
    pub fn set_bus_silent(&mut self, bus: usize, silent: bool) {
        if let Some(b) = self.bus_silent.get_mut(bus) {
            *b = Some(silent);
        }
    }

    /// The planar channels of one bus.
    pub fn bus_mut(&mut self, bus: usize) -> Option<&mut [Vec<f32>]> {
        self.data.get_mut(bus).map(Vec::as_mut_slice)
    }

    /// Points the VST3 tables at the current data (no allocation).
    pub(super) fn refresh(&mut self) {
        for (((d, p), b), own) in self.data.iter_mut().zip(self.ptrs.iter_mut()).zip(self.bufs.iter_mut()).zip(&self.bus_silent) {
            let silent = own.unwrap_or(self.silent);
            for (dc, pc) in d.iter_mut().zip(p.iter_mut()) {
                *pc = dc.as_mut_ptr();
            }
            b.numChannels = i32::try_from(p.len()).unwrap_or(0);
            b.silenceFlags = if silent { silence_flags(p.len()) } else { 0 };
            b.__field0.channelBuffers32 = p.as_mut_ptr();
        }
    }
}

/// `AudioBusBuffers::silenceFlags` with every one of `channels` channels silent (one bit per
/// channel; all 64 from 64 channels up).
pub(crate) fn silence_flags(channels: usize) -> u64 {
    u32::try_from(channels).ok().and_then(|c| 1u64.checked_shl(c)).map_or(u64::MAX, |b| b.wrapping_sub(1))
}
