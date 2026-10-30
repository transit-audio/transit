use super::*;
use transit_rt::phase_marker::Phase;

/// Live input routed to monitored tracks.
pub(crate) struct LiveInput<'a> {
    pub(crate) input: &'a [Vec<f32>],
    pub(crate) monitor_only: bool,
    pub(crate) recording: bool,
    /// [`MixEngine::no_suspend`]: every insert runs every block.
    pub(crate) no_suspend: bool,
    /// The block's first render frame (plan mode: instrument events are due against it).
    pub(crate) frame: u64,
    /// The machine's I/O paths, which resolve an input route's name (a label's number is the fallback).
    pub(crate) io: Option<&'a transit_model::IoMap>,
    /// The callback's phase marker (#162), when the engine has one: strips say which they are in.
    pub(crate) marker: Option<&'a transit_rt::phase_marker::PhaseMarker>,
}

pub(crate) struct StripRender<'a> {
    pub(crate) s: &'a Session,
    pub(crate) busses: &'a BusTable,
    pub(crate) live: &'a LiveInput<'a>,
    pub(crate) pos: Samples,
    pub(crate) frames: usize,
    pub(crate) any_solo: bool,
    pub(crate) keys: &'a KeyBank,
}

/// Source → trim → inserts → (pre-fader copy) → mute/fader for one strip. Independent of other
/// strips except that aux inputs read `busses`, so non-aux strips can run in parallel.
pub(crate) fn process_strip(t: &Track, strip: &mut Strip, block: &StripRender<'_>) {
    if let Some(marker) = block.live.marker {
        let phase = if is_aux(t) { Phase::Auxes } else { Phase::StripRender };
        marker.enter(phase, Some(u32::from(strip.track)));
    }
    let transport = transport_at(block.s, block.pos, !block.live.monitor_only, block.live.recording);
    process_strip_source(t, strip, block, &transport);
    process_strip_inserts(t, strip, block, &transport);
    process_strip_fader(t, strip, block);
}

fn process_strip_source(t: &Track, strip: &mut Strip, block: &StripRender<'_>, transport: &Transport) {
    let StripRender { s, busses, live, pos, frames, .. } = *block;
    for c in strip.buf.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
    match t.kind {
        TrackKind::Audio if !live.input.is_empty() && (t.mixer.input_monitor || (t.mixer.record_arm && (live.monitor_only || live.recording))) => {
            // Input monitoring: the track hears its hardware input instead of its clips.
            let n_in = live.input.len().max(1);
            for (k, dst) in strip.buf.iter_mut().enumerate() {
                let channel = match &t.mixer.input {
                    Route::Hardware(h) => transit_model::input_channel(live.io, h, k),
                    _ => k,
                };
                if let Some(src) = live.input.get(channel % n_in) {
                    for (d, x) in dst.iter_mut().zip(src.iter()).take(frames) {
                        *d = *x;
                    }
                }
            }
        }
        TrackKind::Audio if live.monitor_only => {}
        TrackKind::Audio => {
            render_audio_clips(s, t, strip, pos, frames);
        }
        TrackKind::Aux | TrackKind::Folder => {
            if let Route::Bus(b) = &t.mixer.input
                && let Some(src) = busses.bus(*b)
            {
                let (ns, nd) = (src.buf.len(), strip.buf.len());
                let dst_fmt = strip_format(t, nd);
                let m = if src.fmt == dst_fmt && ns == nd {
                    identity_matrix(ns)
                } else if ns == 1 && nd == 2 {
                    // A mono bus feeds both sides of a stereo aux.
                    let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
                    m[0][0] = 1.0;
                    m[0][1] = 1.0;
                    m
                } else {
                    let stereo = if ns == 1 { [0.0, 0.0] } else { [-1.0, 1.0] };
                    build_matrix(src.fmt, ns, dst_fmt, nd, &PanSpec { stereo, surround: None }, PanLaw::Minus3)
                };
                apply_matrix(&src.buf, frames, &mut strip.buf, &m, 1.0);
            }
        }
        TrackKind::Instrument | TrackKind::Midi => {
            // An External Instrument: the synth's return from the live input, monitored with no
            // added delay whether or not the transport runs (ADR 0020 decision 2).
            if let Some(x) = &strip.ext {
                let gain = t.external_instrument().map_or(1.0, |e| db_to_gain(e.gain_db()));
                x.run(&mut strip.buf, live.input, frames, gain);
            }
            if let Some(inst) = strip.instrument.as_mut().filter(|i| !i.failed) {
                // #152: push the instrument's stored parameter values into the live plugin
                // before processing, the same way inserts do below. Without this, the values
                // loaded from a session (or set via `mix.insert_state`) never reach the plugin
                // instance and the rendered audio matches the plugin's defaults — e.g. a saved
                // `level = -12 dB` sounds the same as `level = -6 dB`.
                if let Some(ins) = &t.instrument {
                    apply_params(Some(s), inst, ins, t, &strip.touch, INSTRUMENT_SLOT, pos, timed_span(transport, frames));
                }
                inst.plugin.set_transport(transport);
                if let Some(e) = inst.events.as_deref_mut() {
                    e.retry_releases(&mut *inst.plugin);
                }
                // Stopped: no clips, but the instrument still plays auditioned notes.
                // An external MIDI destination (#61) takes the clips' notes and controllers; the
                // instrument still gets note-offs, so switching the destination mid-note cannot
                // leave an instrument voice stuck.
                if !live.monitor_only {
                    let internal = t.midi_output.is_internal();
                    if std::mem::take(&mut strip.chase) && internal {
                        chase_controllers(s, t, pos, inst);
                    }
                    schedule_notes(s, t, pos, frames, live.frame, inst, internal);
                }
                // Deliver the remaining live events, including when stopped (F10).
                if let Some(e) = inst.events.as_deref_mut() {
                    e.deliver(&mut *inst.plugin, live.frame, frames);
                }
                if strip.buf.len() >= 2 {
                    inst.plugin.process(&mut strip.buf, frames);
                } else if let Some(c0) = strip.buf.first_mut() {
                    // Mono MIDI track: render the instrument in stereo and fold.
                    let mut st = [std::mem::take(c0), std::mem::take(&mut strip.scratch)];
                    inst.plugin.process(&mut st, frames);
                    let [a, b] = st;
                    *c0 = a;
                    strip.scratch = b;
                    for (x, y) in c0.iter_mut().zip(strip.scratch.iter()).take(frames) {
                        *x = 0.5 * (*x + *y);
                    }
                }
                if let Some(tap) = &inst.tap {
                    tap.push(&strip.buf, frames, 0.0);
                    if tap.enabled() {
                        tap.push_voices(inst.plugin.voice_activity());
                    }
                }
                if contain_failure(inst, &mut strip.buf, frames) {
                    strip.failures.record(INSTRUMENT_SLOT);
                    // Out until the next reset: its queue is voided and what it held released (F10).
                    if let Some(e) = inst.events.as_deref_mut() {
                        e.void(&mut *inst.plugin);
                    }
                }
            }
        }
        _ => {}
    }
}

fn process_strip_inserts(t: &Track, strip: &mut Strip, block: &StripRender<'_>, transport: &Transport) {
    let StripRender { live, pos, frames, keys, .. } = *block;
    let trim = db_to_gain(t.mixer.trim_db) * if t.mixer.phase_invert { -1.0 } else { 1.0 };
    if (trim - 1.0).abs() > f32::EPSILON {
        for c in strip.buf.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x *= trim);
        }
    }
    let mut gr = 0.0f32;
    // A latent instrument delays its track like an insert does (G8), and so does an External
    // Instrument that plays (ADR 0020 decision 6).
    // A frozen track reports the latency of the chain its render replaces.
    let mut latency = head_latency(t, strip, !live.input.is_empty());
    strip.key_in.iter_mut().for_each(|k| k.ran = false);
    strip.hw.iter_mut().for_each(hardware::HwSlot::rest);
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        // A hardware insert sends the signal out and takes the return up; with no live input, or
        // an unresolved path, it passes the signal through dry and adds no latency.
        if let Some(ins) = ins.as_ref().filter(|x| x.hardware.is_some()) {
            if ins.active
                && !ins.bypass
                && let Some(h) = strip.hw.iter_mut().find(|h| usize::from(h.spec.slot) == i)
                && h.run(&mut strip.buf, live.input, frames)
            {
                latency = latency.saturating_add(h.round_trip());
            }
            continue;
        }
        let (Some(ins), Some(Some(slot))) = (ins, strip.plugins.get_mut(i)) else { continue };
        if !ins.active || ins.bypass || slot.failed {
            continue;
        }
        apply_params(Some(block.s), slot, ins, t, &strip.touch, i, pos, timed_span(transport, frames));
        // An insert with a key detects on it when the plan made one for the slot (else on its own input).
        let key = ins.key.and_then(|_| keys.get(t.id, i));
        // A key that arrives late holds the strip's signal back to meet it (`KeyBank::align`).
        if key.is_some()
            && let Some(k) = strip.key_in.iter_mut().find(|k| usize::from(k.slot) == i)
        {
            k.ran = true;
            k.line.apply(k.want, &mut strip.buf, &mut k.scratch, frames);
            latency = latency.saturating_add(k.want.min(k.line.cap()));
        }
        if run_insert(slot, &mut strip.buf, key, frames, transport, live.no_suspend) && contain_failure(slot, &mut strip.buf, frames) {
            strip.failures.record(i);
            continue;
        }
        latency = latency.saturating_add(slot.plugin.latency());
        let slot_gr = slot.plugin.gain_reduction_db();
        if gr == 0.0 {
            gr = slot_gr;
        }
        if let Some(tap) = &slot.tap {
            tap.push(&strip.buf, frames, slot_gr);
        }
    }
    strip.key_in.iter_mut().filter(|k| !k.ran).for_each(|k| k.line.idle());
    strip.gr = gr;
    strip.latency = latency.min(MAX_LATENCY);
}

fn process_strip_fader(t: &Track, strip: &mut Strip, block: &StripRender<'_>) {
    let StripRender { s, pos, frames, any_solo, .. } = *block;
    let auto_mute = t.mixer.automation_mode.reads() && t.lane(&AutoParam::Mute).is_some_and(|l| l.value_at(pos, 0.0) >= 0.5);
    let soloed_out = any_solo && !t.mixer.solo && !t.mixer.solo_safe && !receives_solo(s, t);
    strip.muted = t.mixer.mute || auto_mute || soloed_out;
    if strip.key_pre || t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
        for (d, src) in strip.pre.iter_mut().zip(strip.buf.iter()) {
            d.clear();
            d.extend_from_slice(src.get(..frames.min(d.capacity())).unwrap_or(&[]));
        }
    }
    // Endpoints are the first and last sample of this block. The next block starts on its own
    // first sample, so a breakpoint there does not ramp the block before it.
    let last = pos.saturating_add(frames.saturating_sub(1) as i64);
    let end = frames.saturating_sub(1);
    let ramps = Some(&strip.touch);
    let mut v0 = volume_db_at(t, ramps, pos, 0);
    let mut v1 = volume_db_at(t, ramps, last, end);
    // Trim automation (an offset on top of the volume curve).
    if let Some(l) = t
        .automation
        .iter()
        .find(|l| matches!(&l.param, AutoParam::Plugin { slot: u8::MAX, param } if param == "trim"))
        .filter(|l| !l.points.is_empty())
    {
        v0 += touch::lane_value(t, ramps, &l.param, l, 0.0, pos, 0);
        v1 += touch::lane_value(t, ramps, &l.param, l, 0.0, last, end);
    }
    // VCA master: its fader offsets the member's, and its mute mutes the member. The VCA's return
    // ramp lives on its own strip, so a released VCA fader goes back to its lane without one here.
    if let Some(vid) = t.mixer.vca
        && let Some(vca) = s.tracks.iter().find(|x| x.id.0 == vid && x.kind == TrackKind::Vca)
    {
        v0 += volume_db_at(vca, None, pos, 0);
        v1 += volume_db_at(vca, None, last, end);
        if vca.mixer.mute {
            strip.muted = true;
        }
    }
    let (g0, g1) = if strip.muted { (-144.0, -144.0) } else { (v0.min(12.0), v1.min(12.0)) };
    for c in strip.buf.iter_mut() {
        for i in 0..frames {
            if let Some(x) = c.get_mut(i) {
                *x *= lerp_gain(g0, g1, i, frames.saturating_sub(1));
            }
        }
    }
}

/// A master fader over the main mix: copies the mix into the strip, runs its inserts and writes
/// it back through the fader.
pub(crate) fn process_master(
    t: &Track,
    strip: &mut Strip,
    main: &mut [Vec<f32>],
    pos: Samples,
    frames: usize,
    transport: &Transport,
    no_suspend: bool,
) {
    for (dst, src) in strip.buf.iter_mut().zip(main.iter()) {
        if let (Some(d), Some(sr)) = (dst.get_mut(..frames), src.get(..frames)) {
            d.copy_from_slice(sr);
        }
    }
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        if let (Some(ins), Some(Some(slot))) = (ins, strip.plugins.get_mut(i))
            && ins.active
            && !ins.bypass
            && !slot.failed
        {
            apply_params(None, slot, ins, t, &strip.touch, i, pos, timed_span(transport, frames));
            if run_insert(slot, &mut strip.buf, None, frames, transport, no_suspend) && contain_failure(slot, &mut strip.buf, frames) {
                strip.failures.record(i);
            } else if let Some(tap) = &slot.tap {
                tap.push(&strip.buf, frames, slot.plugin.gain_reduction_db());
            }
        }
    }
    let v0 = volume_db_at(t, Some(&strip.touch), pos, 0);
    // Same endpoints as a track fader: the block's first and last sample, ramping over frames - 1.
    let end = frames.saturating_sub(1);
    let v1 = volume_db_at(t, Some(&strip.touch), pos.saturating_add(end as i64), end);
    let n = main.len();
    for (ch, src) in strip.buf.iter().enumerate().take(n) {
        if let Some(m) = main.get_mut(ch) {
            for i in 0..frames {
                let g = lerp_gain(v0, v1, i, frames.saturating_sub(1));
                if let (Some(d), Some(x)) = (m.get_mut(i), src.get(i)) {
                    *d = x * g;
                }
            }
        }
    }
}

/// Below this magnitude (about -120 dBFS) a sample counts as silence for insert suspension.
const SILENCE_THRESHOLD: f32 = 1e-6;

/// The span whose automation breakpoints `apply_params` sends at their offsets: the block while
/// the transport plays, none while it is stopped. Stopped, every block renders at the same
/// position, so sending a breakpoint inside it would flip the parameter between the playhead's
/// value and the breakpoint's every block (#117 review C1).
fn timed_span(transport: &Transport, frames: usize) -> usize {
    if transport.playing { frames } else { 0 }
}

/// True when every sample of every channel in the block is exactly 0.0: no threshold, because
/// CLAP's `CLAP_PROCESS_SLEEP` means "sleep until a non-zero input", however quiet.
fn is_all_zero(buf: &[Vec<f32>], frames: usize) -> bool {
    buf.iter().all(|c| c.get(..frames).unwrap_or(c).iter().fold(true, |ok, x| ok & (*x == 0.0)))
}

/// True when every sample of every channel in the block is below [`SILENCE_THRESHOLD`].
fn is_silent(buf: &[Vec<f32>], frames: usize) -> bool {
    // A branch-free fold vectorizes (see `contain_failure`).
    buf.iter().all(|c| c.get(..frames).unwrap_or(c).iter().fold(true, |ok, x| ok & (x.abs() < SILENCE_THRESHOLD)))
}

/// Runs one insert over the block, or leaves it unprocessed when it is a hosted effect, its
/// input is silent and its [`ProcessStatus`] allows that (`Sleep` while the input is exactly
/// zero, any nonzero sample wakes it; `ContinueIfNotQuiet` once its last output was silent too; `Tail` once `tail_samples()` of
/// silence have gone through and its last output was silent). The buffers are then left as they
/// are: silent in, silent out. Any non-silent block resumes it, and a parameter change wakes it
/// until a block delivers the value ([`Plugin::params_pending`], #78), without restarting its tail
/// count. `no_suspend` runs every insert. A `key`
/// (sidechain) counts as input: a silent block with a loud key is not silent. True when the
/// plugin processed the block.
fn run_insert(slot: &mut PluginSlot, buf: &mut [Vec<f32>], key: Option<&[Vec<f32>]>, frames: usize, transport: &Transport, no_suspend: bool) -> bool {
    if slot.suspendable {
        // Exactly zero implies below the threshold, so the second scan runs only when it must.
        let all_zero = is_all_zero(buf, frames) && key.is_none_or(|k| is_all_zero(k, frames));
        let silent = all_zero || (is_silent(buf, frames) && key.is_none_or(|k| is_silent(k, frames)));
        if silent {
            let skip = !no_suspend
                && !slot.woken
                && match slot.plugin.process_status() {
                    ProcessStatus::Continue => false,
                    ProcessStatus::ContinueIfNotQuiet => slot.output_silent,
                    ProcessStatus::Sleep => all_zero,
                    ProcessStatus::Tail => slot.output_silent && slot.quiet >= slot.plugin.tail_samples(),
                };
            if skip {
                return false;
            }
            slot.quiet = slot.quiet.saturating_add(frames);
        } else {
            slot.quiet = 0;
        }
        // The flag asserts true silence (VST3 `silenceFlags`, CLAP `constant_mask`), so a quiet but
        // nonzero block must not set it.
        slot.plugin.set_input_silent(all_zero);
    }
    slot.plugin.set_transport(transport);
    match key {
        Some(k) => slot.plugin.process_with_key(buf, k, frames),
        None => slot.plugin.process(buf, frames),
    }
    // Cleared only once the plug-in holds no undelivered value (#78): a host that skipped the
    // block keeps the value queued, and the slot stays awake until a block delivers it.
    slot.woken = slot.plugin.params_pending();
    if slot.suspendable {
        slot.output_silent = is_silent(buf, frames);
    }
    true
}

/// Silences a block the plugin filled with NaN or infinity and takes the plugin out until the
/// next [`MixEngine::reset`]. True then.
fn contain_failure(slot: &mut PluginSlot, buf: &mut [Vec<f32>], frames: usize) -> bool {
    // A branch-free fold vectorizes; `all` runs about 8 times slower here.
    if buf.iter().all(|c| c.get(..frames).unwrap_or(c).iter().fold(true, |ok, x| ok & x.is_finite())) {
        return false;
    }
    for c in buf.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
    slot.failed = true;
    true
}

/// Record insert latency without processing audio, so this block can compensate for it.
/// The latency at the head of a strip, before its inserts: a frozen track's chain, a latent
/// instrument's, and an External Instrument's when it plays (`duplex`: live input is open).
pub(crate) fn head_latency(t: &Track, strip: &Strip, duplex: bool) -> usize {
    frozen_latency(t)
        .saturating_add(strip.instrument.as_ref().filter(|i| !i.failed).map_or(0, |i| i.plugin.latency()))
        .saturating_add(strip.ext.as_ref().map_or(0, |x| x.latency(duplex)))
}

/// The latency in samples a frozen track reports (0 for any other).
pub(crate) fn frozen_latency(t: &Track) -> usize {
    t.freeze.as_ref().map_or(0, |f| f.latency as usize)
}

pub(crate) fn note_insert_latency(t: &Track, strip: &mut Strip, pos: Samples, duplex: bool) {
    let mut latency = frozen_latency(t);
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        // As `process_strip`: a hardware insert that runs (live input, resolved paths) adds its round trip.
        if let Some(l) = hardware::slot_latency(ins, &strip.hw, i, duplex) {
            latency = latency.saturating_add(l);
            continue;
        }
        let (Some(ins), Some(Some(slot))) = (ins, strip.plugins.get_mut(i)) else { continue };
        // As `process_strip`: a failed insert is skipped, so its latency no longer counts.
        if !ins.active || ins.bypass || slot.failed {
            continue;
        }
        // Block-start values only: processing sends the breakpoints inside the block.
        apply_params(None, slot, ins, t, &strip.touch, i, pos, 0);
        // The delay `KeyBank::align` gave the signal before a keyed insert (0 without one).
        if ins.key.is_some()
            && let Some(k) = strip.key_in.iter().find(|k| usize::from(k.slot) == i)
        {
            latency = latency.saturating_add(k.want.min(k.line.cap()));
        }
        latency = latency.saturating_add(slot.plugin.latency());
    }
    strip.latency = latency.min(MAX_LATENCY);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{PlanRig, add_dc_track, render_block, session_with_dc, synth_session};
    use transit_model::Insert;

    /// An effect that reports a fixed [`ProcessStatus`] and tail, counts its `process` calls and
    /// records what it was told with `set_input_silent`. After its input goes silent it rings on:
    /// it outputs 0.5 for `decay` more processed blocks.
    struct Sleepy {
        inner: Box<dyn Plugin>,
        status: ProcessStatus,
        tail: usize,
        decay: usize,
        /// Ringing blocks left.
        left: usize,
        /// `set_param` calls.
        sets: Arc<std::sync::atomic::AtomicUsize>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        told: Arc<std::sync::Mutex<Vec<bool>>>,
    }

    impl Plugin for Sleepy {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.sets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let silent_in = io.iter().all(|c| c.iter().take(frames).all(|x| *x == 0.0));
            self.inner.process(io, frames);
            if !silent_in {
                self.left = self.decay;
            } else if self.left > 0 {
                self.left -= 1;
                io.iter_mut().for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = 0.5));
            }
        }
        fn tail_samples(&self) -> usize {
            self.tail
        }
        fn process_status(&self) -> ProcessStatus {
            self.status
        }
        fn set_input_silent(&mut self, silent: bool) {
            self.told.lock().unwrap().push(silent);
        }
    }

    /// A hosted effect that sleeps on silence and counts plain and keyed blocks.
    struct Ducker {
        plain: Arc<std::sync::atomic::AtomicUsize>,
        keyed: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Plugin for Ducker {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            transit_dsp::plugin_info("gain").unwrap()
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool {
            true
        }
        fn param(&self, _: &str) -> Option<f32> {
            None
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
            self.plain.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn process_with_key(&mut self, _: &mut [Vec<f32>], _: &[Vec<f32>], _: usize) {
            self.keyed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn process_status(&self) -> ProcessStatus {
            ProcessStatus::Sleep
        }
    }

    #[test]
    fn a_loud_key_wakes_a_sleeping_insert_whose_own_input_is_silent() {
        use std::sync::atomic::Ordering::Relaxed;
        let (plain, keyed) = (Arc::<std::sync::atomic::AtomicUsize>::default(), Arc::<std::sync::atomic::AtomicUsize>::default());
        let mut slot = PluginSlot::new(Box::new(Ducker { plain: plain.clone(), keyed: keyed.clone() }), "clap:test.ducker".into(), 2, 0);
        assert!(slot.suspendable);
        let mut buf = vec![vec![0.0f32; 64]; 2];
        let (silent, loud) = (vec![vec![0.0f32; 64]; 2], vec![vec![0.5f32; 64]; 2]);
        let t = Transport::default();
        // Silent main, silent key (or none): asleep.
        assert!(!run_insert(&mut slot, &mut buf, None, 64, &t, false));
        assert!(!run_insert(&mut slot, &mut buf, Some(&silent), 64, &t, false));
        // Silent main, loud key: the ducker runs, and gets the key.
        assert!(run_insert(&mut slot, &mut buf, Some(&loud), 64, &t, false));
        assert_eq!((plain.load(Relaxed), keyed.load(Relaxed)), (0, 1));
        // Loud main and no key: plain processing, as before keys existed.
        buf.iter_mut().for_each(|c| c.fill(0.5));
        assert!(run_insert(&mut slot, &mut buf, None, 64, &t, false));
        assert_eq!((plain.load(Relaxed), keyed.load(Relaxed)), (1, 1));
    }

    #[test]
    fn hosted_effects_are_suspended_by_their_own_status() {
        use ProcessStatus::*;
        // DC at 0.5 for the first 1024 samples, silence after: blocks at 4096.. are silent input.
        const LOUD: i64 = 0;
        const QUIET: [i64; 4] = [4096, 4608, 5120, 5632];
        struct Row {
            status: ProcessStatus,
            tail: usize,
            /// Blocks the fake rings on (outputs 0.5) after its input goes silent.
            decay: usize,
            /// Before this block, the session changes one of the insert's parameters.
            param_at: Option<usize>,
            no_suspend: bool,
            instrument: bool,
            /// Level of the DC clip that feeds the insert (0.5 unless a row needs a tiny one).
            dc: f32,
            /// Block positions rendered, in order.
            blocks: &'static [i64],
            /// `process` calls after each block.
            calls: &'static [usize],
            /// Everything passed to `set_input_silent`, in order.
            told: &'static [bool],
        }
        let rows = [
            Row {
                status: Continue,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[QUIET[0]],
                calls: &[1],
                told: &[true],
            },
            Row {
                status: ContinueIfNotQuiet,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[QUIET[0], LOUD],
                calls: &[0, 1],
                told: &[false],
            },
            Row {
                status: Sleep,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[QUIET[0], LOUD],
                calls: &[0, 1],
                told: &[false],
            },
            Row {
                status: Tail,
                tail: 1024,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[QUIET[0], QUIET[1], QUIET[2], LOUD],
                calls: &[1, 2, 2, 3],
                told: &[true, true, false],
            },
            Row {
                status: Sleep,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: true,
                instrument: false,
                dc: 0.5,
                blocks: &[QUIET[0]],
                calls: &[1],
                told: &[true],
            },
            // Output, not input: a decaying ContinueIfNotQuiet effect is processed while its output
            // rings after the input went silent, and suspended once it came out silent.
            Row {
                status: ContinueIfNotQuiet,
                tail: 0,
                decay: 2,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[LOUD, QUIET[0], QUIET[1], QUIET[2], QUIET[3]],
                calls: &[1, 2, 3, 4, 4],
                told: &[false, true, true, true],
            },
            // Tail: the tail counter alone (0 here) does not suspend an effect still ringing.
            Row {
                status: Tail,
                tail: 0,
                decay: 1,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[LOUD, QUIET[0], QUIET[1], QUIET[2]],
                calls: &[1, 2, 3, 3],
                told: &[false, true, true],
            },
            // A parameter change wakes a sleeping effect for that block only, so the change reaches it.
            Row {
                status: Sleep,
                tail: 0,
                decay: 0,
                param_at: Some(1),
                no_suspend: false,
                instrument: false,
                dc: 0.5,
                blocks: &[QUIET[0], QUIET[1], QUIET[2]],
                calls: &[0, 1, 1],
                told: &[true],
            },
            Row {
                status: Sleep,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: true,
                dc: 0.5,
                blocks: &[QUIET[0], QUIET[1]],
                calls: &[1, 2],
                told: &[],
            },
            // Sleep wakes on any nonzero sample, even one below SILENCE_THRESHOLD, and sleeps on exact zero.
            Row {
                status: Sleep,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 1e-7,
                blocks: &[LOUD],
                calls: &[1],
                told: &[false],
            },
            // Exact zero is told it is silent.
            Row {
                status: Continue,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.0,
                blocks: &[LOUD],
                calls: &[1],
                told: &[true],
            },
            Row {
                status: Sleep,
                tail: 0,
                decay: 0,
                param_at: None,
                no_suspend: false,
                instrument: false,
                dc: 0.0,
                blocks: &[LOUD],
                calls: &[0],
                told: &[],
            },
        ];
        for (n, row) in rows.iter().enumerate() {
            let (mut s, mut t) = session_with_dc(row.dc, 1024);
            let id = "clap:test.sleepy";
            let slot = if row.instrument {
                t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
                s.track_mut(t).unwrap().instrument = Some(Insert::new(id));
                INSTRUMENT_SLOT
            } else {
                s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new(id));
                0
            };
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let told = Arc::new(std::sync::Mutex::new(Vec::new()));
            let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let fake = Sleepy {
                inner: transit_dsp::create("gain").unwrap(),
                status: row.status,
                tail: row.tail,
                decay: row.decay,
                left: 0,
                sets: Arc::clone(&sets),
                calls: Arc::clone(&calls),
                told: Arc::clone(&told),
            };
            let mut rig = PlanRig::new(512).fake(t, slot, fake);
            rig.eng.no_suspend = row.no_suspend;
            rig.publish(&s);
            for (b, (&pos, &want)) in row.blocks.iter().zip(row.calls).enumerate() {
                if row.param_at == Some(b)
                    && let Some(Some(ins)) = s.track_mut(t).unwrap().mixer.inserts.get_mut(0)
                {
                    ins.params.insert("gain".into(), -6.0);
                    rig.publish(&s);
                }
                let out = render_block(&mut rig, pos);
                assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), want, "row {n} ({:?}), block {b} at {pos}", row.status);
                if pos != LOUD && row.decay == 0 {
                    assert_eq!(out, 0.0, "row {n}: silent in, silent out");
                }
            }
            assert_eq!(*told.lock().unwrap(), row.told, "row {n} ({:?}): set_input_silent", row.status);
            let want_sets = usize::from(row.param_at.is_some());
            assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), want_sets, "row {n} ({:?}): set_param", row.status);
        }
    }

    struct Nan(Box<dyn Plugin>);

    impl Plugin for Nan {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.0.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.0.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.0.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.0.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.0.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            io.iter_mut().for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = f32::NAN));
        }
    }

    #[test]
    fn an_insert_that_puts_out_nan_is_taken_out_until_the_next_reset() {
        let (mut s, a) = session_with_dc(0.5, 48_000);
        let b = add_dc_track(&mut s, 0.5, 48_000);
        s.track_mut(a).unwrap().mixer.inserts[2] = Some(Insert::new("clap:test.nan"));
        // Without the instance the slot passes audio through.
        let mut dry = PlanRig::new(512);
        dry.publish(&s);
        let both = render_block(&mut dry, 0);
        let mut rig = PlanRig::new(512).fake(a, 2, Nan(transit_dsp::create("gain").unwrap()));
        rig.publish(&s);
        let only_b = render_block(&mut rig, 512);
        assert!(only_b > 0.3 && (only_b - both / 2.0).abs() < 1e-6, "{only_b} vs {both}");
        let failures = |rig: &PlanRig, t: TrackId| rig.eng.meters().find(|(id, _)| *id == t).map(|(_, m)| m.plugin_failures).unwrap();
        assert_eq!(failures(&rig, a), PluginFailures { count: 1, last: 2 });
        assert_eq!(failures(&rig, b), PluginFailures::default());
        assert_eq!(render_block(&mut rig, 1024), both);
        rig.eng.reset();
        assert_eq!(render_block(&mut rig, 1536), only_b);
        assert_eq!(failures(&rig, a).count, 2);
    }

    #[test]
    fn nan_from_an_instrument_or_a_master_insert_is_contained() {
        let mut s = synth_session(-6.0);
        let synth = s.tracks[0].id;
        s.track_mut(synth).unwrap().instrument = Some(Insert::new("clap:test.nan"));
        let b = add_dc_track(&mut s, 0.5, 48_000);
        let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        s.track_mut(m).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.nan"));
        let mut rig = PlanRig::new(512).fake(synth, INSTRUMENT_SLOT, Nan(transit_dsp::create("gain").unwrap()));
        // The master insert is absent in the first plan.
        let mut without = s.clone();
        without.track_mut(m).unwrap().mixer.inserts[0] = None;
        rig.publish(&without);
        let failures = |rig: &PlanRig, t: TrackId| rig.eng.meters().find(|(id, _)| *id == t).map(|(_, m)| m.plugin_failures).unwrap();
        let only_b = render_block(&mut rig, 0);
        assert!(only_b > 0.3, "{only_b}");
        assert_eq!(failures(&rig, synth), PluginFailures { count: 1, last: INSTRUMENT_SLOT });
        assert_eq!(failures(&rig, b), PluginFailures::default());
        rig.fakes.push((m, 0, Box::new(Nan(transit_dsp::create("gain").unwrap()))));
        rig.publish(&s);
        assert_eq!(render_block(&mut rig, 512), 0.0);
        assert_eq!(failures(&rig, m), PluginFailures { count: 1, last: 0 });
        assert_eq!(render_block(&mut rig, 1024), only_b);
    }
}
