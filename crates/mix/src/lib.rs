//! The transit mix engine.
//!
//! [`MixEngine::render_active`] produces one block of the main output for a compiled session at a
//! timeline position: clips (with clip gain and fades) → trim/phase → inserts → pre-fader sends → fader and
//! mute (with automation) → pan → post-fader sends → busses → aux inputs → main → master faders.
//! The same code drives realtime playback and offline bounces, so what you hear is what you get.
//! Steady-state rendering does not allocate.
//!
//! ## Surround
//! The main mix has the format of the session's main output path ([`Session::main_format`]:
//! stereo by default, up to 16 channels such as 7.1.4 or 9.1.6) and every bus has its own format.
//! Channels are in SMPTE / WAV order (L R C LFE Ls Rs …, see
//! [`ChannelFormat::speakers`](transit_model::ChannelFormat::speakers)). Routing a strip into a
//! destination builds a gain matrix per block:
//! - mono/stereo sources into a stereo destination use the stereo pan exactly as before;
//! - into a multichannel destination they use the track's [`SurroundPan`] (pairwise
//!   constant-power panner over the speaker layout, see `transit_dsp::pan::surround_gains`),
//!   or go to L/R with the stereo pan when the track has no surround pan;
//! - sources whose format equals the destination's pass straight through, channel for channel;
//! - other multichannel sources fold to stereo with ITU-R BS.775 coefficients (C and surrounds at
//!   -3 dB, LFE dropped), or map speaker by speaker into other multichannel layouts.
//!
//! ## Plugin instances and threads
//! Hosted third-party plugins (`clap:` / `vst3:` ids, see [`is_third_party`]) are expensive and
//! have main-thread rules; built-in plugins only allocate, but allocating is not allowed on the
//! audio thread either. So every instance is made off the audio thread, by the control thread
//! with [`create_instance`] from [`realtime_instance_specs`] (restoring each hosted insert's
//! stored [`Insert::state`] and calling `prepare` there), or by the [`PlanCompiler`]. Every
//! instance a plan replaces or removes goes back to the control thread in the envelope
//! ([`Retired`]), so plugin destruction never runs on the audio thread either.
//!
//! An instance starts from its stored state; the insert's parameter values are then applied on
//! top before the first block (the session's values are kept in step with the plugin, including
//! edits made in its own editor, so both agree; the state carries everything else). A host reads
//! states back for saving through each instance's [`Plugin::state_reader`], on its UI thread.
//!
//! A built-in instrument's parameter values are applied the same way. A sample-playing
//! instrument (the Sampler) is handed the decoded audio of its insert's
//! [`Insert::sample`] source straight from the session's pool, shared and never copied.
//!
//! ## Prepared plans
//! An engine made with [`MixEngine::realtime`] never discovers structure on the audio thread.
//! A [`PlanCompiler`] on the control thread turns each Session into a [`PlanEnvelope`] whose
//! tables (strips, plug-in slots, clip effects, busses, main mix, delay lines, parameter
//! storage) are sized and bounded by [`PlanLimits`]. [`MixEngine::adopt_plan`] validates it
//! without changing anything, then only swaps: reused cells (a strip's running state, plug-in
//! slots) move into the new tables, and everything the old plan owned goes back in the same
//! envelope, to be dropped on the control thread at [`PlanCompiler::reclaim`].
//! [`PlanCompiler::compile`] preserves matching instances for edits and Save snapshots within
//! one document. Opening/replacing a document or explicitly restoring its saved state requires
//! [`PlanCompiler::compile_with_mode`] with [`CompileMode::NewDocument`]: every slot is prepared
//! afresh with the incoming state and a new [`SlotGen`]. The render target stays unchanged;
//! playback must preserve this restore intent through coalescing until adoption is confirmed.
//! [`MixEngine::render_active`] renders the active plan, for realtime playback and for offline
//! renders ([`render_range`], [`RangeRender`]) alike.
//!
//! [`Insert::state`]: transit_model::Insert::state
//! [`Insert::sample`]: transit_model::Insert::sample
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::ops::Bound;
use std::sync::Arc;
use transit_dsp::event::{CC_HOLD2, CC_SOFT, CC_SOSTENUTO, CC_SUSTAIN};
use transit_dsp::pan::{PanLaw, SpeakerPos, SurroundParams, gains};
use transit_dsp::{Plugin, ProcessStatus, SampleSource, Transport, db_to_gain};
use transit_midi::CtrlKind;
use transit_model::{
    AutoParam, BusId, ChannelFormat, Clip, ClipContent, ClipLoop, IoMap, Route, RouteCycle, RouteScratch, SEND_SLOTS, Session, SourceAudio, Speaker,
    SurroundPan, Track, TrackId, TrackKind,
};
use transit_time::{Range, Samples, TICKS_PER_QUARTER};

mod active;
mod automation;
/// Metronome: a short synthesized blip on every beat of the tempo map, accented on beat 1 of
/// each bar. Settings come from `setup.click_countoff` (`click.*` values).
mod click;
#[cfg(test)]
mod clip_loop_tests;
mod clips;
mod compile;
mod events;
mod external;
mod external_midi;
mod hardware;
mod instance;
mod key;
mod matrix;
mod midi;
mod offline;
mod plan;
#[cfg(test)]
mod plan_fault_tests;
#[cfg(test)]
mod plan_instrument_tests;
#[cfg(test)]
mod plan_latency_tests;
#[cfg(test)]
mod plan_ownership_tests;
#[cfg(test)]
mod plan_render_tests;
#[cfg(test)]
mod plan_tests;
mod render_pool;
mod routing;
mod scope;
mod sink;
mod strip;
#[cfg(not(target_arch = "wasm32"))]
mod strip_pool;
#[cfg(target_arch = "wasm32")]
#[path = "strip_pool_wasm.rs"]
mod strip_pool;
#[cfg(test)]
mod tests;

use automation::{apply_params, lerp_gain, pan_at, pan_segment, send_level, transport_at, volume_db_at};
#[cfg(test)]
use clips::render_clip;
pub use clips::render_clips;
use clips::{clip_fx_params, render_audio_clips};
pub use compile::{CompileError, CompileMode, MIN_DELAY_CAPACITY, PlanCompiler, PlanLimits};
pub use events::{
    Arrival, Due, EngineEpoch, EventLimits, EventOutcome, EventStatus, InstrumentEvent, MAX_OPEN, MAX_PEDALS, MAX_PORTS, Message, NoteId,
    PluginEvent, QUEUE_CAP, RenderFrame, Shed, ShedCounts, Source, TapRecord, TransportEpoch,
};
pub use external::{ExternalReport, ExternalState, external_report};
pub use external_midi::{ExternalMidi, external_midi_events};
pub use hardware::{HardwareReport, HardwareState, hardware_report};
pub use instance::{
    InstanceSpec, NewInstance, create_instance, create_instance_with, creating_instance, creation_line, instance_specs, realtime_instance_specs,
};
use key::{KeyBank, KeyEnd, KeyIn, KeySpec};
use matrix::{PanSpec, apply_matrix, apply_matrix_span, build_matrix, identity_matrix};
pub use matrix::{fold_down_stereo, itu_stereo_fold};
use midi::{
    ClipEvent, Rtp, chase_controllers, grid_start, midi_loop_frame, midi_offset, note_spans, played_note, schedule_notes, sorted_clip_events,
};
#[cfg(test)]
use midi::{PEDAL_SCANS, visit_clip_events};
pub use midi::{clip_note_ons, cycle_at, loop_frame, unroll_midi_loop};
pub use offline::{MAX_RENDER_SAMPLES, RangeRender, render_bus, render_range, render_track_pre_fader, render_track_pre_fader_timed, source_channels};
pub use plan::{
    Adoption, DeviceGeneration, EnvelopeState, PlanEnvelope, PlanGeneration, PlanIdentity, PlanStatus, PreparedPlan, RejectReason, Rejected,
    RenderTarget, Retired, SlotBinding, SlotGen,
};
#[cfg(test)]
use render_pool::flag_value;
use render_pool::{FORCE_SERIAL, PARALLEL_STRIPS, guarded, init_render_switch, run_strips};
pub use render_pool::{
    MAX_RENDER_WORKERS, POOL_HISTOGRAM_BUCKETS, POOL_HISTOGRAM_LIMITS_NS, RENDER_NO_PRIORITY_ENV, RENDER_POOL_ENV, StripFaults, StripFigure,
    TOP_STRIPS, mark_audio_thread, on_audio_thread, unmark_audio_thread,
};
use routing::{
    MixOut, PdcWalk, is_aux, live, log_loops, path_need, pdc_visit, processing_order, receives_solo, route_into, strip_channels, strip_format,
};
pub use scope::{INSTRUMENT_UID, SCOPE_FRAMES, ScopeHub, ScopeTap};
use sink::Sink;
pub use sink::{MAX_SINK_CHANNELS, SinkPlan, SinkSpec, plan_sinks, unresolved_routes};
use strip::{LiveInput, StripRender, head_latency, note_insert_latency, process_master, process_strip};
pub use strip_pool::StripFaultsReader;

/// Default (stereo) main mix width. The actual width is [`main_channels`].
pub const MAIN_CHANNELS: usize = 2;

/// Widest strip, bus or main mix (9.1.6 / 3rd-order Ambisonics). Wider formats are truncated.
pub const MAX_CHANNELS: usize = 16;

/// Number of channels of a session's main mix.
pub fn main_channels(s: &Session) -> usize {
    s.main_format().channels().clamp(MAIN_CHANNELS, MAX_CHANNELS)
}

/// Slot index used for a track's instrument plugin in [`InstanceSpec`]
/// (insert slots use their index).
pub const INSTRUMENT_SLOT: usize = usize::MAX;

/// Hosted third-party plugin ids (CLAP or VST3): created off the audio thread in realtime use.
pub fn is_third_party(id: &str) -> bool {
    id.starts_with(transit_clap_host::ID_PREFIX) || id.starts_with(transit_vst3_host::ID_PREFIX) || id.starts_with(transit_au_host::ID_PREFIX)
}

/// Top bit of a clip-effect slot index (see [`clip_fx_slot`]). Insert slots are small, and
/// [`INSTRUMENT_SLOT`] is never a clip-effect slot.
const CLIP_FX_BIT: usize = 1 << (usize::BITS - 1);
/// Built-in plugins a clip's effect chain is made of (Clip Effects).
const CLIP_EQ: &str = transit_dsp::plugins::eq::EQ7_INFO.id;
const CLIP_COMP: &str = transit_dsp::plugins::dynamics::COMP_INFO.id;

/// Slot index of a clip's effect module in [`InstanceSpec`]: its EQ
/// (`comp` false) or its compressor. `None` for a clip id too large to encode (not made by
/// [`Session::new_clip_id`] in practice); such a clip plays without its effects in realtime.
#[must_use]
pub fn clip_fx_slot(clip: u64, comp: bool) -> Option<usize> {
    let clip = usize::try_from(clip).ok().filter(|c| *c < (1 << (usize::BITS - 2)) - 1)?;
    Some(CLIP_FX_BIT | (clip << 1) | usize::from(comp))
}

/// The clip and module ([`clip_fx_slot`]) a slot index names, if it is a clip-effect slot.
fn clip_fx_of_slot(slot: usize) -> Option<(u64, bool)> {
    if slot == INSTRUMENT_SLOT || slot & CLIP_FX_BIT == 0 {
        return None;
    }
    Some((((slot & !CLIP_FX_BIT) >> 1) as u64, slot & 1 == 1))
}

/// Which clip-effect modules (EQ, compressor) a clip's current parameters need.
fn clip_fx_modules(s: &Session, clip: u64) -> (bool, bool) {
    clip_fx_params(s, clip).map_or((false, false), |p| (p.clone().any(|(k, _)| k.starts_with("eq.")), p.clone().any(|(k, _)| k.starts_with("comp."))))
}

/// Peak meter values for one strip (linear, per channel, max over the last block).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StripMeter {
    /// First two channels (mono strips repeat channel 0), for stereo meters.
    pub peak: [f32; 2],
    /// Every channel of the strip (or of the main mix).
    pub peaks: Vec<f32>,
    /// Gain reduction of the first dynamics insert, dB.
    pub gain_reduction_db: f32,
    pub plugin_failures: PluginFailures,
}

/// How often a strip's plugins were taken out of the signal path for putting out NaN or infinity.
/// Each stays out until the engine is next reset (playback starts or stops).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PluginFailures {
    /// It only grows.
    pub count: u32,
    /// The slot of the latest: an insert index or [`INSTRUMENT_SLOT`].
    pub last: usize,
}

impl PluginFailures {
    fn record(&mut self, slot: usize) {
        self.count = self.count.wrapping_add(1);
        self.last = slot;
    }
}

impl StripMeter {
    fn measure(&mut self, bufs: &[Vec<f32>], frames: usize, gr: f32) {
        for (p, b) in self.peaks.iter_mut().zip(bufs.iter()) {
            *p = peak(b.get(..frames).unwrap_or(&[]));
        }
        let last = self.peaks.len().saturating_sub(1);
        self.peak = [self.peaks.first().copied().unwrap_or(0.0), self.peaks.get(1.min(last)).copied().unwrap_or(0.0)];
        self.gain_reduction_db = gr;
    }
}

/// A bus: its format and one buffer per channel.
struct BusBuf {
    fmt: ChannelFormat,
    buf: Vec<Vec<f32>>,
    /// Delay compensation, this block: the latency every signal summed onto the bus is aligned
    /// to (the slowest path into it), so what reads the bus starts at this latency.
    arrival: usize,
    /// Render position of the first strip that reads the bus this block (`usize::MAX`: none).
    /// A strip at or after it writing here is a routing loop's feedback edge: it is dropped.
    reader: usize,
}

impl BusBuf {
    fn new(fmt: ChannelFormat, buf: Vec<Vec<f32>>) -> Self {
        BusBuf { fmt, buf, arrival: 0, reader: usize::MAX }
    }

    /// Silence the block and forget the last block's delay-compensation pass.
    fn clear(&mut self, frames: usize) {
        for c in self.buf.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        self.arrival = 0;
        self.reader = usize::MAX;
    }
}

/// The plan's busses, sorted by id.
#[derive(Default)]
struct BusTable(Vec<(BusId, BusBuf)>);

impl BusTable {
    fn bus(&self, id: BusId) -> Option<&BusBuf> {
        let i = self.0.binary_search_by_key(&id, |(b, _)| *b).ok()?;
        self.0.get(i).map(|(_, b)| b)
    }
    fn bus_mut(&mut self, id: BusId) -> Option<&mut BusBuf> {
        let i = self.0.binary_search_by_key(&id, |(b, _)| *b).ok()?;
        self.0.get_mut(i).map(|(_, b)| b)
    }
}

/// Parameter maps up to this long are tracked inline in a [`PluginSlot`] (every built-in's is).
const APPLIED_INLINE: usize = 64;

/// The parameter values last pushed to a plugin, in the insert's parameter-map order. Up to
/// [`APPLIED_INLINE`] values live inline, so a slot filled on the audio thread allocates nothing
/// for them; longer maps use heap storage prepared by the compiler.
struct Applied {
    len: usize,
    inline: [f32; APPLIED_INLINE],
    /// Plan mode: `heap` is prepared off the audio thread (see `Attach::Applied`) and never
    /// resized here. A map whose length does not match it pushes the values that fit.
    heap: Vec<f32>,
    /// How often a map did not match the prepared storage.
    mismatch: u32,
}

impl Applied {
    const fn new() -> Self {
        Applied { len: 0, inline: [f32::NAN; APPLIED_INLINE], heap: Vec::new(), mismatch: 0 }
    }

    /// Plan mode: `heap` is the prepared storage (empty for a map that fits inline).
    fn prepared(heap: Vec<f32>) -> Self {
        Applied { len: 0, inline: [f32::NAN; APPLIED_INLINE], heap, mismatch: 0 }
    }

    /// The `n` values, all NaN ("never pushed") when the map's length changed. In plan mode a
    /// long map whose length differs from the prepared storage gets `min(n, storage)` values.
    fn values(&mut self, n: usize) -> &mut [f32] {
        if n != self.len {
            self.len = n;
            if n <= APPLIED_INLINE {
                self.inline.fill(f32::NAN);
            } else {
                self.heap.fill(f32::NAN);
                if self.heap.len() != n {
                    self.mismatch = self.mismatch.saturating_add(1);
                }
            }
        }
        if n <= APPLIED_INLINE { self.inline.get_mut(..n).unwrap_or_default() } else { &mut self.heap }
    }
}

struct PluginSlot {
    id: String,
    /// Channel count the plugin was prepared for.
    ch: usize,
    plugin: Box<dyn Plugin>,
    /// Last values pushed to the plugin, in the insert's parameter-map order.
    applied: Applied,
    failed: bool,
    /// A hosted plugin that has not had its first parameter pass yet (see `apply_values`).
    fresh: bool,
    /// A hosted (third-party) effect insert: it may be left unprocessed while its input is
    /// silent and its [`ProcessStatus`] allows it. Never set for instruments or built-ins.
    suspendable: bool,
    /// Samples of silent input the plugin has processed since its input was last not silent
    /// (how far into its tail it is).
    quiet: usize,
    /// The block the plugin last processed came out silent (true before the first). A plugin
    /// whose output still rings is not suspended.
    output_silent: bool,
    /// A parameter was sent to the plugin since it last processed a block. Its next block is
    /// processed even while suspended, so the queued change reaches it.
    woken: bool,
    /// The slot's identity across plans.
    generation: SlotGen,
    /// Plan mode: channel-format code of the strip it was prepared for.
    layout: u16,
    /// Plan mode: the pool audio and the wrapper handed to the instrument with `set_sample`. Both
    /// owners stay here, so the plug-in never drops the last one.
    sample_audio: Option<Arc<SourceAudio>>,
    sample_wrapper: Option<Arc<dyn SampleSource>>,
    /// Plan mode, instruments: the event queue and the open notes and pedals (F10), prepared by
    /// the compiler. They travel with the instance and retire with it.
    events: Option<Box<events::InstrumentEvents>>,
    /// Plan mode, inserts and instruments: the scope ring the panel reads (see [`ScopeTap`]).
    tap: Option<Arc<ScopeTap>>,
}

impl PluginSlot {
    /// A slot for a new instance in slot `slot` ([`INSTRUMENT_SLOT`], an insert index or a
    /// [`clip_fx_slot`]). Hosted plug-ins start `fresh`; hosted effect inserts are suspendable.
    fn new(plugin: Box<dyn Plugin>, id: String, ch: usize, slot: usize) -> Self {
        let third = is_third_party(&id);
        PluginSlot {
            id,
            ch,
            plugin,
            applied: Applied::new(),
            failed: false,
            fresh: third,
            suspendable: third && slot != INSTRUMENT_SLOT && clip_fx_of_slot(slot).is_none(),
            quiet: 0,
            output_silent: true,
            woken: false,
            generation: SlotGen::VACANT,
            layout: 0,
            sample_audio: None,
            sample_wrapper: None,
            events: None,
            tap: None,
        }
    }
}

struct Strip {
    buf: Vec<Vec<f32>>,
    /// Pre-fader copy for pre-fader sends (and pre-fader keys, [`Strip::key_pre`]).
    pre: Vec<Vec<f32>>,
    /// Plan mode: another strip's insert takes a pre-fader key from this one, so `pre` is filled.
    key_pre: bool,
    scratch: Vec<f32>,
    plugins: Vec<Option<PluginSlot>>,
    instrument: Option<PluginSlot>,
    /// The instrument's next block starts somewhere new: send it the controllers in effect there.
    chase: bool,
    muted: bool,
    gr: f32,
    /// Per-clip effect chains (Clip Effects) and a scratch buffer to render clips into.
    clip_fx: ClipFxTable,
    clipbuf: Vec<Vec<f32>>,
    /// Total latency of the instrument, the active inserts and the delays before keyed inserts
    /// (samples).
    latency: usize,
    /// Plan mode: the delay of this strip's own signal before each keyed insert (a key that
    /// arrives late, [`KeyBank::align`]).
    key_in: Vec<key::KeyIn>,
    /// Delay compensation (F05): the post-fader and the pre-fader signal, read back by each edge
    /// the strip feeds at that edge's own delay, so every path into a summing point arrives with
    /// the slowest one.
    tap: DelayLine,
    tap_pre: DelayLine,
    /// Scratch for one delayed edge (channels x block).
    outbuf: Vec<Vec<f32>>,
    /// Delay compensation, this block: the latency at the strip's output (the latency its input
    /// arrives with plus its own), and its place in the render order.
    pdc_out: usize,
    pdc_pos: usize,
    /// Each edge's read of the delay lines: 0 the output, 1 + i send i.
    edges: [EdgeTap; SEND_SLOTS + 1],
    /// Compiled sink indices for output and send edges.
    sink_routes: Vec<Option<usize>>,
    failures: PluginFailures,
    /// The strip's identity across plans. [`SlotGen::VACANT`] in a strip shell that waits for a
    /// move and in a strip whose state moved out.
    generation: SlotGen,
    /// Plan mode: index of the strip's track in the plan's session.
    track: u16,
    /// Plan mode: the track's id.
    id: TrackId,
    /// Plan mode: channel-format code of `buf`.
    layout: u16,
    /// The strip's meter.
    meter: StripMeter,
    /// Plan mode: the strip's hardware inserts (ADR 0015 section 5), made by the compiler.
    hw: Vec<hardware::HwSlot>,
    /// Plan mode: the External Instrument in the instrument slot (ADR 0020), made by the compiler.
    ext: Option<external::ExtSlot>,
    /// A panic out of this strip's processing in phase 1, caught by [`guarded`] on the thread that
    /// rendered the strip (moving the box allocates nothing). The render re-raises the first one to
    /// its caller; any other stays here until a later render re-raises it, or leaves with the
    /// strip for the control side (a plan's returned tables, [`MixEngine::into_retired`]). While
    /// it holds one the strip does not render, so nothing overwrites (drops) it.
    panic: Option<Box<dyn std::any::Any + Send>>,
}

/// Most latency delay compensation handles: 2^20 samples, about 22 s at 48 kHz. A strip reports
/// at most this, whatever its plugins claim, so a bogus value can't stall offline renders.
const MAX_LATENCY: usize = 1 << 20;

/// A multichannel multi-tap delay line: a strip writes each block once, and every edge it feeds
/// (its output, each send) reads it back at its own delay. The ring holds one block more than the
/// longest delay, so a tap of up to `len` samples reads the block just written.
#[derive(Default)]
struct DelayLine {
    bufs: Vec<Vec<f32>>,
    /// Next write position in the ring.
    idx: usize,
    /// Longest delay a tap can read.
    len: usize,
    /// A stand-in in a strip shell, whose live line moves in with the strip state.
    placeholder: bool,
    /// The ring holds the strip's recent blocks: it was written every block since it was zeroed.
    live: bool,
    /// Samples of continuous history the ring holds (at most its length).
    written: usize,
    /// The line started this block (it was idle): its taps start at their delay, from silence.
    fresh: bool,
}

/// Samples over which an edge crossfades from its old delay to a new one.
const XFADE: usize = 256;

impl DelayLine {
    /// A stand-in in a strip shell whose live lines move in with the strip state.
    fn placeholder() -> Self {
        DelayLine { placeholder: true, ..DelayLine::default() }
    }

    fn is_placeholder(&self) -> bool {
        self.placeholder && self.bufs.is_empty()
    }

    /// A delay line serving delays up to `cap` samples, in blocks up to `block`. Its storage is
    /// sized off the audio thread and never resized; a longer delay is clamped to `cap`.
    fn with_capacity(ch: usize, cap: usize, block: usize) -> Self {
        DelayLine { bufs: vec![vec![0.0; cap.saturating_add(block)]; ch], len: cap, ..DelayLine::default() }
    }

    fn ring(&self) -> usize {
        self.bufs.first().map_or(0, Vec::len)
    }

    /// Audio side: whether the line holds a delay of `need`. Never allocates: a longer delay is
    /// read clamped to the line's capacity, and the delay asked for is returned (else 0) so the
    /// shortfall can be reported and the line grown by the next plan.
    fn fit(&self, need: usize) -> usize {
        let need = need.min(MAX_LATENCY);
        if need > self.len { need } else { 0 }
    }

    /// Takes over `old`'s history (its most recent samples, in order), so the delay continues
    /// across a replacement. Copies only; allocates nothing.
    fn take_history(&mut self, old: &DelayLine) {
        let (size, osize) = (self.ring(), old.ring());
        let n = old.written.min(osize).min(size);
        for (c, ring) in self.bufs.iter_mut().enumerate() {
            ring.fill(0.0);
            let Some(src) = old.bufs.get(c).filter(|_| osize > 0) else { continue };
            for k in 0..n {
                if let (Some(d), Some(x)) = (ring.get_mut(size - n + k), src.get((old.idx + osize - n + k) % osize)) {
                    *d = *x;
                }
            }
        }
        self.idx = 0;
        self.written = n;
        self.live = old.live && n > 0;
        self.fresh = false;
    }

    /// Appends one block. A line that was idle is zeroed first, so no stale audio comes back.
    fn write(&mut self, src: &[Vec<f32>], frames: usize) {
        let size = self.ring();
        if size == 0 || frames > size {
            return;
        }
        self.fresh = !self.live;
        if !self.live {
            self.clear();
            self.idx = 0;
            self.written = 0;
            self.live = true;
        }
        let start = self.idx;
        for (ring, ch) in self.bufs.iter_mut().zip(src.iter()) {
            let mut i = start;
            for x in ch.iter().take(frames) {
                if let Some(r) = ring.get_mut(i) {
                    *r = *x;
                }
                i += 1;
                if i >= size {
                    i = 0;
                }
            }
        }
        self.idx = (start + frames) % size;
        self.written = self.written.saturating_add(frames).min(size);
    }

    /// Nothing reads this block delayed: the line is not written, and is zeroed before it is next.
    fn idle(&mut self) {
        self.live = false;
    }

    /// The block just written, `from` samples late and crossfading to `to` samples late (each
    /// clamped to the line's longest delay), `done` samples into the crossfade (`from == to`: a
    /// plain read), into `dst` (each channel resized to `frames`, within its capacity).
    fn read_x(&self, from: usize, to: usize, done: usize, dst: &mut [Vec<f32>], frames: usize) {
        let size = self.ring();
        let (a, b) = (from.min(self.len), to.min(self.len));
        let base = |d: usize| if size == 0 { 0 } else { (self.idx + 2 * size - (frames % size) - (d % size)) % size };
        let (sa, sb) = (base(a), base(b));
        for (c, out) in dst.iter_mut().enumerate() {
            out.clear();
            out.resize(frames.min(out.capacity()), 0.0);
            let Some(ring) = self.bufs.get(c).filter(|_| self.live && size > 0) else { continue };
            for (i, x) in out.iter_mut().enumerate() {
                let va = ring.get((sa + i) % size).copied().unwrap_or(0.0);
                *x = if a == b {
                    va
                } else {
                    let vb = ring.get((sb + i) % size).copied().unwrap_or(0.0);
                    let g = ((done + i) as f32 / XFADE as f32).min(1.0);
                    va + (vb - va) * g
                };
            }
        }
    }

    fn clear(&mut self) {
        self.bufs.iter_mut().for_each(|b| b.fill(0.0));
        self.written = 0;
    }
}

/// One edge's read of its strip's delay line (F05): the delay it plays now and, while it moves to
/// a new one, the crossfade. A longer delay waits until the line holds the history it needs (a
/// delay jump never reads silence the line never held); a shorter one starts at once.
#[derive(Debug, Clone, Copy, Default)]
struct EdgeTap {
    eff: usize,
    from: usize,
    left: usize,
    pre: bool,
    init: bool,
}

impl EdgeTap {
    /// This block's read for target delay `d` on `line` (written this block): (from, to, done).
    fn step(&mut self, d: usize, pre: bool, line: &DelayLine, frames: usize) -> (usize, usize, usize) {
        let d = d.min(line.len);
        if !self.init || self.pre != pre || line.fresh || !line.live {
            *self = EdgeTap { eff: d, from: d, left: 0, pre, init: true };
        }
        if self.left == 0 && d != self.eff && (d < self.eff || line.written >= d.saturating_add(frames)) {
            self.from = self.eff;
            self.eff = d;
            self.left = XFADE;
        }
        let r = (self.from, self.eff, XFADE - self.left.min(XFADE));
        self.left = self.left.saturating_sub(frames);
        if self.left == 0 {
            self.from = self.eff;
        }
        r
    }

    /// The longest delay this edge may still read (its current and crossfade-from delays).
    fn holds(&self) -> usize {
        if self.init { self.eff.max(self.from) } else { 0 }
    }
}

/// Clip Effects: an EQ and a compressor per clip, configured from `edit.values`
/// (`clip_fx.<clip>.eq.<param>`, `clip_fx.<clip>.comp.<param>`, `clip_fx.<clip>.gain`).
struct ClipFx {
    eq: Option<ClipModule>,
    comp: Option<ClipModule>,
    /// The clip's parameters use the module this block. A plan keeps a module whose parameters
    /// went away (it is not dropped on the audio thread) until the next plan retires it.
    eq_on: bool,
    comp_on: bool,
    gain_db: f32,
    sr: f32,
    ch: usize,
    /// Consolidation: modules are made when a clip's parameters first need them and dropped when
    /// they no longer do. A plan's chains get their modules from the compiler.
    lazy: bool,
}

/// A clip-effect module, in a slot like an insert's: the plugin id it came with from the host
/// stays with it, so that string is not freed on the audio thread when the module arrives.
type ClipModule = PluginSlot;

/// A strip's clip-effect chains, sorted by clip id, so a lookup neither hashes nor allocates.
#[derive(Default)]
struct ClipFxTable(Vec<(u64, ClipFx)>);

impl ClipFxTable {
    fn get_mut(&mut self, clip: u64) -> Option<&mut ClipFx> {
        let i = self.0.binary_search_by_key(&clip, |(c, _)| *c).ok()?;
        self.0.get_mut(i).map(|(_, fx)| fx)
    }
}

impl Strip {
    fn new() -> Self {
        Strip {
            buf: Vec::new(),
            pre: Vec::new(),
            key_pre: false,
            scratch: Vec::new(),
            plugins: Vec::new(),
            instrument: None,
            chase: true,
            muted: false,
            gr: 0.0,
            clip_fx: ClipFxTable::default(),
            clipbuf: Vec::new(),
            latency: 0,
            key_in: Vec::new(),
            tap: DelayLine::default(),
            tap_pre: DelayLine::default(),
            outbuf: Vec::new(),
            pdc_out: 0,
            pdc_pos: 0,
            edges: [EdgeTap::default(); SEND_SLOTS + 1],
            sink_routes: Vec::new(),
            failures: PluginFailures::default(),
            generation: SlotGen::VACANT,
            track: 0,
            id: TrackId(0),
            layout: 0,
            meter: StripMeter::default(),
            hw: Vec::new(),
            ext: None,
            panic: None,
        }
    }
}

/// The click of a record count-in. The transport is parked at the record start while it runs, so
/// the beats come from this grid (project samples), not from the tempo map at the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountInClick {
    /// Project samples of count-in already rendered before the block.
    pub elapsed: Samples,
    /// Samples per beat.
    pub beat: Samples,
    /// Beats per bar; the first beat of each bar is accented.
    pub per_bar: u32,
}

/// Renders sessions. Keep one per playback stream; plugin state persists across blocks.
pub struct MixEngine {
    max_block: usize,
    pub pan_law: PanLaw,
    /// Last rendered position end (to detect seeks).
    last_end: Samples,
    /// Current total compensation delay (samples) of the main mix: the slowest path into it
    /// (instruments, inserts, auxes and busses along the way), plus the master faders' inserts.
    pub latency: usize,
    /// The slowest path into the main mix (before the master faders), this block.
    main_arrival: usize,
    /// Live input for the next block (planar), set by the audio host for input monitoring.
    pub input: Vec<Vec<f32>>,
    /// Transport stopped: only monitored inputs sound (no clips).
    pub monitor_only: bool,
    /// Recording: record-armed tracks hear their input (auto input).
    pub recording: bool,
    /// Never leave a hosted effect unprocessed for silent input: every active insert runs every
    /// block. The host sets this while any plug-in editor is open, so an editor never freezes.
    pub no_suspend: bool,
    /// Mix the metronome (Options > Click) into the output. Playback turns this on; offline renders
    /// (bounces, stems, exports) leave it off so the click is never printed into a file.
    pub metronome: bool,
    /// Linear trim on the click level (the transport's metronome control). Not part of the session.
    pub metronome_gain: f32,
    /// A record count-in in progress: the click sounds while the transport is parked (see
    /// [`CountInClick`]), whatever Options > Click says.
    pub count_in: Option<CountInClick>,
    /// Render every strip on the calling thread (tests compare it with the parallel render).
    serial: bool,
    /// The active plan [`MixEngine::render_active`] renders.
    plan: Option<Box<plan::ActivePlan>>,
    /// Instrument events (F10): the epoch, the render clock and the counters.
    events: events::EngineEvents,
    /// The callback's phase marker (#162): written with relaxed stores on the audio thread.
    marker: Option<std::sync::Arc<transit_rt::phase_marker::PhaseMarker>>,
    /// The render workers ([`MixEngine::start_render_workers`]; issue #124, ADR 0029).
    pool: Option<strip_pool::StripPool>,
    /// Overrides the strips' deadline share of the block period ([`MixEngine::set_strip_deadline`]).
    strip_deadline: Option<std::time::Duration>,
    /// The next block's strip deadline from the callback's entry ([`MixEngine::set_block_deadline`]);
    /// taken by the next `render_active`.
    block_deadline: Option<std::time::Instant>,
}

impl MixEngine {
    /// Give the engine the phase marker its render writes (#162). Control thread, before the
    /// engine renders.
    pub fn set_phase_marker(&mut self, marker: std::sync::Arc<transit_rt::phase_marker::PhaseMarker>) {
        self.marker = Some(marker);
    }

    /// Force (or stop forcing) every engine in the process to render its audio-thread strips
    /// serially on the audio thread, never on its render workers. Serial is the default (see
    /// [`RENDER_POOL_ENV`]); `force_serial(false)` hands the strips to an engine's render workers
    /// once it has them ([`MixEngine::start_render_workers`]). Takes effect at the next block (a
    /// strip a worker still holds comes back first). Offline renders are not affected.
    pub fn force_serial(on: bool) {
        FORCE_SERIAL.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// True while serial rendering is in effect: the default, until [`MixEngine::force_serial`]`(false)`
    /// or [`RENDER_POOL_ENV`] turns the render workers on.
    #[must_use]
    pub fn serial_forced() -> bool {
        FORCE_SERIAL.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Also reads [`RENDER_POOL_ENV`] on first use (never from the audio thread).
    fn new(max_block: usize) -> Self {
        init_render_switch();
        MixEngine {
            max_block: max_block.clamp(16, 16_384),
            pan_law: PanLaw::Minus3,
            last_end: 0,
            latency: 0,
            main_arrival: 0,
            input: Vec::new(),
            monitor_only: false,
            recording: false,
            no_suspend: false,
            metronome: false,
            metronome_gain: 1.0,
            count_in: None,
            serial: false,
            marker: None,
            plan: None,
            events: events::EngineEvents::new(None, EventLimits::default()),
            pool: None,
            strip_deadline: None,
            block_deadline: None,
        }
    }

    /// Every strip of the active plan.
    fn plan_strips(&mut self) -> impl Iterator<Item = &mut Strip> {
        self.plan.iter_mut().flat_map(|p| p.tables.strips.iter_mut())
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Format of the main mix of the active plan.
    pub fn main_format(&self) -> ChannelFormat {
        self.plan.as_deref().map_or(ChannelFormat::Stereo, |p| p.tables.main_fmt)
    }

    /// Channels of the main mix of the active plan.
    pub fn main_channels(&self) -> usize {
        self.plan.as_deref().map_or(MAIN_CHANNELS, |p| p.tables.main.len())
    }

    /// Play (velocity > 0) or release (velocity 0) `pitch` on a track's instrument at the next
    /// block, outside the clips: the MIDI Editor's audition. Does nothing before a plan is adopted,
    /// nor while a render worker holds the track's strip (an overrun, ADR 0029).
    ///
    /// Superseded by [`MixEngine::queue_event`] (F10), which carries the source, channel, voice
    /// identity and due frame and reserves the release; kept until the Player migrates. Notes
    /// sent here are not tracked: a revocation or seek does not release them.
    pub fn live_note(&mut self, track: TrackId, pitch: u8, velocity: u8) {
        let Some(p) = self.plan.as_deref_mut() else { return };
        let strip = p.tables.strip_of(track).and_then(|i| p.tables.strips.get_mut(i));
        let Some(inst) = strip.and_then(|s| s.instrument.as_mut()) else { return };
        if velocity > 0 { inst.plugin.note_on(0, pitch.min(127), velocity.min(127)) } else { inst.plugin.note_off(0, pitch.min(127)) }
    }

    /// Stop or panic: releases every instrument event source, live and clip (every open note's
    /// off and held pedal's up go to the plug-in), then resets all plugin state. Plug-in hosts
    /// keep those releases through their reset. For a locate or loop wrap during playback use
    /// [`MixEngine::set_transport_epoch`] and the position jump in [`MixEngine::render_active`]
    /// instead, which release only what the clips play (see the `events` module).
    pub fn reset(&mut self) {
        // A strip a render worker holds is reset when it is back, and nothing deferred for any
        // strip plays after the stop (ADR 0029).
        if let Some(pool) = self.pool.as_mut() {
            pool.mark_away(strip_pool::PENDING_RESET);
            pool.discard_parked();
        }
        for s in self.plan_strips() {
            render_pool::reset_render_state(s);
            s.tap.clear();
            s.tap_pre.clear();
            s.tap.idle();
            s.tap_pre.idle();
        }
        // Meters are fixed storage, zeroed in place.
        if let Some(p) = self.plan.as_deref_mut() {
            for m in p.tables.strips.iter_mut().map(|s| &mut s.meter).chain(std::iter::once(&mut p.tables.main_meter)) {
                m.peak = [0.0; 2];
                m.peaks.fill(0.0);
                m.gain_reduction_db = 0.0;
                m.plugin_failures = PluginFailures::default();
            }
        }
    }

    /// Release every held note on every instrument now. Unlike `reset`, this leaves plugin state,
    /// the playback position and note chasing alone, so queued releases still sound out.
    /// Every source is released: queued instrument events are dropped and each open note and
    /// pedal gets its release first (stop and panic).
    pub fn all_notes_off(&mut self) {
        if let Some(pool) = self.pool.as_mut() {
            pool.mark_away(strip_pool::PENDING_NOTES_OFF);
            pool.discard_parked();
        }
        for st in self.plan_strips() {
            render_pool::release_instrument(st);
        }
    }

    /// Give every instrument exactly one processing pass over silence, so a host that delivers
    /// queued events on its next `process` (a `reset` or `all_notes_off` only queues the release)
    /// gets to send them, even when the engine would otherwise not run (playback ended, stopped
    /// with nothing monitoring). `frames` is clamped to `max_block`. Nothing is scheduled, the
    /// position is untouched, inserts do not run, and the output is discarded. Allocation-free.
    pub fn flush_releases(&mut self, frames: usize) {
        let frames = frames.min(self.max_block);
        for st in self.plan_strips() {
            let Some(inst) = st.instrument.as_mut().filter(|i| !i.failed) else { continue };
            // Defensive: the compiler sizes `buf` and `scratch` to `max_block`, but a plug-in must never
            // be handed a buffer shorter than `frames`, so clamp to the shortest one.
            if st.buf.len() >= 2 {
                let frames = frames.min(st.buf.iter().map(Vec::len).min().unwrap_or(0));
                for c in st.buf.iter_mut() {
                    c.iter_mut().take(frames).for_each(|x| *x = 0.0);
                }
                inst.plugin.process(&mut st.buf, frames);
            } else if let Some(c0) = st.buf.first_mut() {
                let mut io = [std::mem::take(c0), std::mem::take(&mut st.scratch)];
                let frames = frames.min(io[0].len()).min(io[1].len());
                for c in io.iter_mut() {
                    c.iter_mut().take(frames).for_each(|x| *x = 0.0);
                }
                inst.plugin.process(&mut io, frames);
                let [a, b] = io;
                *c0 = a;
                st.scratch = b;
            }
        }
    }
}

/// A short key formatted on the stack: lookups made on the audio thread must not allocate.
#[derive(Clone, Copy)]
struct Key {
    buf: [u8; 64],
    len: usize,
}

impl Key {
    /// None when the key doesn't fit.
    fn new(args: std::fmt::Arguments<'_>) -> Option<Key> {
        let mut k = Key { buf: [0; 64], len: 0 };
        std::fmt::Write::write_fmt(&mut k, args).ok()?;
        Some(k)
    }

    fn as_str(&self) -> &str {
        self.buf.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl std::fmt::Write for Key {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(std::fmt::Error)?;
        self.buf.get_mut(self.len..end).ok_or(std::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl ClipFx {
    fn new(sr: f32, ch: usize, lazy: bool) -> ClipFx {
        ClipFx { eq: None, comp: None, eq_on: false, comp_on: false, gain_db: 0.0, sr, ch, lazy }
    }
    fn make(&self, id: &str) -> Option<ClipModule> {
        transit_dsp::create(id).map(|mut p| {
            p.prepare(self.sr, 16_384, self.ch.max(1));
            PluginSlot::new(p, String::new(), self.ch, CLIP_FX_BIT)
        })
    }
    /// A module runs only while one of its parameters is set. Offline, modules are made and
    /// dropped here accordingly; in a plan the compiler places and retires them.
    fn configure<'a>(&mut self, params: impl Iterator<Item = (&'a str, f32)> + Clone) {
        self.gain_db = 0.0;
        self.eq_on = params.clone().any(|(k, _)| k.starts_with("eq."));
        self.comp_on = params.clone().any(|(k, _)| k.starts_with("comp."));
        if self.lazy {
            if self.eq_on && self.eq.is_none() {
                self.eq = self.make(CLIP_EQ);
            } else if !self.eq_on {
                self.eq = None;
            }
            if self.comp_on && self.comp.is_none() {
                self.comp = self.make(CLIP_COMP);
            } else if !self.comp_on {
                self.comp = None;
            }
        }
        for (k, v) in params {
            if let Some(p) = k.strip_prefix("eq.") {
                if let Some(eq) = &mut self.eq
                    && eq.plugin.param(p) != Some(v)
                {
                    eq.plugin.set_param(p, v);
                }
            } else if let Some(p) = k.strip_prefix("comp.") {
                if let Some(c) = &mut self.comp
                    && c.plugin.param(p) != Some(v)
                {
                    c.plugin.set_param(p, v);
                }
            } else if k == "gain" {
                self.gain_db = v;
            }
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if let Some(eq) = self.eq.as_mut().filter(|_| self.eq_on) {
            eq.plugin.process(io, frames);
        }
        if let Some(c) = self.comp.as_mut().filter(|_| self.comp_on) {
            c.plugin.process(io, frames);
        }
        if self.gain_db.abs() > 1e-6 {
            let g = db_to_gain(self.gain_db);
            for ch in io.iter_mut() {
                ch.iter_mut().take(frames).for_each(|x| *x *= g);
            }
        }
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
}
