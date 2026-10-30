use super::*;

/// Sends slot `i`'s parameters for the block of `frames` frames at `pos`: each automated one at
/// its curve's value at the block start, then (#117) each breakpoint of its lane inside the block
/// at its sample offset, to plug-ins whose [`Plugin::sample_accurate_params`] says they take
/// offsets (the VST3 and CLAP hosts). `frames` 0 sends the block-start values only: the latency
/// passes run before the block is processed, and processing sends the breakpoints once; with the
/// transport stopped processing passes 0 too (see `strip::timed_span`). In Touch a held
/// parameter's lane is not read and a released one returns over the strip's return ramp (`ramps`, #188):
/// block-start values while it returns, its breakpoints again from the ramp's end.
///
/// With `session`, a member of a rack (ADR 0034) takes a mapped parameter from the rack's macro
/// lane while that lane has points ([`crate::rack::macro_param_value`], at block rate): the macro
/// rules the parameter, as a mapped parameter follows its macro in Ableton's Instrument Rack.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_params(
    session: Option<&Session>,
    slot: &mut PluginSlot,
    ins: &transit_model::Insert,
    t: &Track,
    ramps: &TouchRamps,
    i: usize,
    pos: Samples,
    frames: usize,
) {
    // Automation lanes address insert slots; the instrument (`INSTRUMENT_SLOT`) has its own lane
    // slot, so it never picks up slot A's lanes.
    let slot_u8 = if i == INSTRUMENT_SLOT { AutoParam::INSTRUMENT_SLOT } else { u8::try_from(i).unwrap_or(u8::MAX) };
    if slot_u8 == u8::MAX {
        return apply_values(slot, ins, pos, |_, v| v, |_| &[]);
    }
    let automated = t.mixer.automation_mode.reads()
        && t.automation.iter().any(|l| matches!(&l.param, AutoParam::Plugin { slot, .. } if *slot == slot_u8) && !l.points.is_empty());
    let lane = |k: &str| {
        t.automation
            .iter()
            .filter(|_| automated)
            .find(|l| matches!(&l.param, AutoParam::Plugin { slot, param } if *slot == slot_u8 && param == k))
            .filter(|l| !l.points.is_empty())
    };
    let timed = frames > 0 && slot.plugin.sample_accurate_params();
    let end = pos.saturating_add(i64::try_from(frames).unwrap_or(i64::MAX));
    // The value at the block start, and where the lane's own breakpoints may start: past a hold
    // none, past a return ramp from its end.
    let read = |k: &str, val: f32| {
        lane(k).map(|l| {
            let (v, left, held) = touch::plugin_value(t, ramps, slot_u8, k, l, val, pos);
            (v, if held { end } else { pos.saturating_add(i64::from(left)) })
        })
    };
    let rack = |k: &str| session.filter(|_| t.folder.is_some()).and_then(|s| crate::rack::macro_param_value(s, t, slot_u8, k, pos));
    apply_values(
        slot,
        ins,
        pos,
        |k, val| rack(k).unwrap_or_else(|| read(k, val).map_or(val, |(v, _)| v)),
        |k| match lane(k) {
            _ if rack(k).is_some() => &[],
            // The breakpoints strictly inside the block: one at its start is the start value.
            Some(l) if timed => {
                let after = read(k, 0.0).map_or(pos, |(_, after)| after.max(pos));
                let from = l.points.partition_point(|p| p.at <= after);
                let to = l.points.partition_point(|p| p.at < end);
                l.points.get(from..to.max(from)).unwrap_or_default()
            }
            _ => &[],
        },
    );
}

/// Sends each stored param (as mapped by `value`) to the plugin when it changed since last time,
/// then each of its `points` (breakpoints inside the block at `pos`) at its offset, and then marks
/// the slot [`woken`](PluginSlot::woken).
fn apply_values<'a>(
    slot: &mut PluginSlot,
    ins: &transit_model::Insert,
    pos: Samples,
    value: impl Fn(&str, f32) -> f32,
    points: impl Fn(&str) -> &'a [transit_model::AutomationPoint],
) {
    // First pass over a hosted plugin: it has just loaded its own settings, so a stored value equal to
    // the default it reports is not a choice and is left alone. Older sessions store every parameter
    // at that default, often 0, and pushing them over a restored plugin silenced Rhapsody (volume and
    // pan at minimum). Values that differ from the default, and every later change, are pushed.
    let first = std::mem::take(&mut slot.fresh);
    for ((k, v), last) in ins.params.iter().zip(slot.applied.values(ins.params.len()).iter_mut()) {
        let val = value(k, *v);
        if val.to_bits() != last.to_bits() {
            let untouched = first && slot.plugin.info().param(k).is_some_and(|pi| pi.default.to_bits() == val.to_bits());
            if !untouched {
                slot.plugin.set_param(k, val);
            }
            *last = val;
            slot.woken = true;
        }
        // Allocation-free: the lane's own points, each one call; the host queues are bounded
        // (past them the last point wins and the host counts the miss).
        for p in points(k) {
            let offset = u32::try_from(p.at.saturating_sub(pos)).unwrap_or(u32::MAX);
            slot.plugin.set_param_at(offset, k, p.value);
            *last = p.value;
            slot.woken = true;
        }
    }
}

pub(crate) fn transport_at(s: &Session, pos: Samples, playing: bool, recording: bool) -> Transport {
    let sr = s.sample_rate;
    let ticks = Some(s.tempo.samples_to_ticks_f(pos, sr)).filter(|t| t.is_finite()).unwrap_or(0.0);
    let tick = ticks.floor() as i64;
    let meter = s.tempo.meter_at_tick(tick);
    let bar = s.tempo.bar_beat_at_tick(tick).bar;
    let q = TICKS_PER_QUARTER as f64;
    Transport {
        playing,
        recording,
        tempo: s.tempo.tempo_at_tick(tick),
        numerator: meter.numerator,
        denominator: meter.denominator,
        samples: pos,
        seconds: pos as f64 / sr.as_f64().max(1.0),
        quarters: ticks / q,
        bar_start: s.tempo.bar_start_tick(bar) as f64 / q,
        bar: bar - 1,
    }
}

/// The fader level at `pos`, `offset` frames into the block (`ramps`: the track's own strip's
/// return ramps, #188).
pub(crate) fn volume_db_at(t: &Track, ramps: Option<&TouchRamps>, pos: Samples, offset: usize) -> f32 {
    if t.mixer.automation_mode.reads()
        && t.mixer.automation_mode != transit_model::AutomationMode::Write
        && let Some(l) = t.lane(&AutoParam::Volume)
        && !l.points.is_empty()
    {
        return touch::lane_value(t, ramps, &AutoParam::Volume, l, t.mixer.volume_db, pos, offset);
    }
    t.mixer.volume_db
}

pub(crate) fn pan_at(t: &Track, ramps: &TouchRamps, idx: usize, pos: Samples, offset: usize) -> f32 {
    let base = t.mixer.pan.get(idx).copied().unwrap_or(if idx == 0 { 0.0 } else { 1.0 });
    let param = AutoParam::Pan(u8::try_from(idx).unwrap_or(0));
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&param)
        && !l.points.is_empty()
    {
        return touch::lane_value(t, Some(ramps), &param, l, base, pos, offset);
    }
    base
}

/// Pan automation is read on a grid of [`PAN_STEP`] timeline samples rather than once per block,
/// so playback (callback-sized blocks) and bounces (1024-frame blocks) pan identically.
const PAN_STEP: Samples = 64;

/// The position to read the pan at for the frames from `start` of the block at `pos`, and the end
/// of the frames that share it. Without pan automation the whole block shares one pan.
pub(crate) fn pan_segment(t: &Track, pos: Samples, start: usize, frames: usize) -> (Samples, usize) {
    let automated = t.mixer.automation_mode.reads() && t.automation.iter().any(|l| matches!(l.param, AutoParam::Pan(_)) && !l.points.is_empty());
    if !automated {
        return (pos, frames);
    }
    let here = pos.saturating_add(start as i64);
    let at = here.div_euclid(PAN_STEP).saturating_mul(PAN_STEP);
    let left = usize::try_from(at.saturating_add(PAN_STEP).saturating_sub(here)).unwrap_or(1).max(1);
    (at, start.saturating_add(left).min(frames))
}

/// Send `i`'s level for the block at `pos` (its start value).
pub(crate) fn send_level(t: &Track, ramps: &TouchRamps, i: usize, base: f32, pos: Samples) -> f32 {
    let param = AutoParam::SendLevel(u8::try_from(i).unwrap_or(0));
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&param)
        && !l.points.is_empty()
    {
        return touch::lane_value(t, Some(ramps), &param, l, base, pos, 0);
    }
    base
}

pub(crate) fn lerp_gain(db0: f32, db1: f32, i: usize, n: usize) -> f32 {
    let g0 = db_to_gain(db0);
    let g1 = db_to_gain(db1);
    if n == 0 { g0 } else { g0 + (g1 - g0) * (i as f32 / n as f32) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{PlanRig, render_block, session_with_dc, synth_session};
    use transit_model::Insert;

    #[test]
    fn stored_instrument_params_reach_the_plugin() {
        // #152: a saved instrument level of -12 dB must render quieter than -6 dB.
        // RMS over the held part of the note, past any parameter smoothing.
        let rms = |db: f32| {
            let out = render_range(&synth_session(db), Range::new(0, 12_000), 512);
            let tail = out[0].get(6_000..).unwrap_or_default();
            (tail.iter().map(|x| x * x).sum::<f32>() / tail.len().max(1) as f32).sqrt()
        };
        let (loud, quiet) = (rms(-6.0), rms(-12.0));
        assert!(loud > 0.0, "the synth should sound");
        assert!((quiet / loud - 0.5).abs() < 0.05, "-12 dB vs -6 dB should halve the level: {quiet} / {loud}");
    }

    #[test]
    fn insert_slot_a_automation_does_not_drive_the_instrument() {
        let mut s = synth_session(-6.0);
        let reference = render_range(&s, Range::new(0, 12_000), 512);
        let tr = &mut s.tracks[0];
        let mut lane = transit_model::AutomationLane::new(AutoParam::Plugin { slot: 0, param: "level".into() });
        lane.set_point(0, -48.0);
        tr.automation.push(lane);
        assert_eq!(render_range(&s, Range::new(0, 12_000), 512), reference);
    }

    #[test]
    fn instrument_lane_automation_drives_the_instrument() {
        let mut s = synth_session(-6.0);
        let reference = render_range(&s, Range::new(0, 12_000), 512);
        let tr = &mut s.tracks[0];
        tr.mixer.automation_mode = transit_model::AutomationMode::Read;
        let mut lane = transit_model::AutomationLane::new(AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "level".into() });
        lane.set_point(0, -48.0);
        tr.automation.push(lane);
        assert_ne!(render_range(&s, Range::new(0, 12_000), 512), reference);
        // Not in Read: the lane is ignored.
        s.tracks[0].mixer.automation_mode = transit_model::AutomationMode::Off;
        assert_eq!(render_range(&s, Range::new(0, 12_000), 512), reference);
    }

    #[test]
    fn master_fader_samples_and_ramps_like_a_track_fader() {
        // #151: a block samples its curve at its first and last sample and ramps across
        // `frames - 1`. A breakpoint on the next block's first sample must not move this one.
        const BLOCK: usize = 512;
        let step = |lane: &mut transit_model::AutomationLane| {
            lane.set_point(0, 0.0);
            lane.set_point(BLOCK as i64 - 1, 0.0);
            lane.set_point(BLOCK as i64, -20.0);
            lane.set_point(2 * BLOCK as i64, -20.0);
        };
        let render = |on_master: bool| {
            let (mut s, t) = session_with_dc(0.5, 4 * BLOCK);
            let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
            let id = if on_master { m } else { t };
            let tr = s.track_mut(id).unwrap();
            tr.mixer.automation_mode = transit_model::AutomationMode::Read;
            let mut lane = transit_model::AutomationLane::new(AutoParam::Volume);
            step(&mut lane);
            tr.automation.push(lane);
            let mut rig = PlanRig::new(BLOCK);
            rig.publish(&s);
            let mut out = rig.render(0)[0].clone();
            out.extend_from_slice(&rig.render(BLOCK as i64)[0]);
            out
        };
        let (track, master) = (render(false), render(true));
        assert_eq!(track.len(), master.len());
        for (i, (a, b)) in track.iter().zip(&master).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {i}: track {a} vs master {b}");
        }
    }

    /// A built-in gain behind a third-party id, with a log of parameter sets.
    struct Stateful {
        inner: Box<dyn Plugin>,
        blob: Vec<u8>,
        sets: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Plugin for Stateful {
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
            self.inner.process(io, frames);
        }
        fn state_reader(&mut self) -> Option<Box<dyn transit_dsp::PluginStateReader>> {
            let blob = self.blob.clone();
            Some(Box::new(move || Some(blob.clone())))
        }
    }

    struct Clock {
        inner: Box<dyn Plugin>,
        seen: Arc<std::sync::Mutex<Vec<Transport>>>,
    }

    impl Plugin for Clock {
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
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
        fn set_transport(&mut self, t: &Transport) {
            self.seen.lock().unwrap().push(*t);
        }
    }

    #[test]
    fn plugins_get_the_session_transport() {
        let (mut s, t) = session_with_dc(0.5, 48_000 * 4);
        s.tempo.set_tempo(0, 90.0).unwrap();
        s.tempo.set_meter(0, 3, 4).unwrap();
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.clock"));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let clock = Clock { inner: transit_dsp::create("gain").unwrap(), seen: Arc::clone(&seen) };
        let mut rig = PlanRig::new(512).fake(t, 0, clock);
        rig.publish(&s);
        render_block(&mut rig, 96_000);
        rig.eng.monitor_only = true;
        render_block(&mut rig, 96_000);
        let seen = seen.lock().unwrap();
        let t0 = seen[0];
        assert!(t0.playing && !t0.recording);
        assert_eq!((t0.tempo, t0.numerator, t0.denominator, t0.samples, t0.bar), (90.0, 3, 4, 96_000, 1));
        assert!((t0.seconds - 2.0).abs() < 1e-9 && (t0.quarters - 3.0).abs() < 1e-9 && (t0.bar_start - 3.0).abs() < 1e-9);
        assert!(!seen[1].playing);
    }

    #[test]
    fn stored_state_comes_first_then_session_params() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let mut ins = Insert::new("vst3:00000000000000000000000000000001");
        ins.params.insert("gain".into(), -6.0);
        ins.set_state_bytes(b"stored");
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
        assert_eq!(instance_specs(&s)[0].state.as_deref(), Some(&b"stored"[..]));
        let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fake = Stateful { inner: transit_dsp::create("gain").unwrap(), blob: Vec::new(), sets: Arc::clone(&sets) };
        let mut rig = PlanRig::new(512).fake(t, 0, fake);
        rig.publish(&s);
        render_block(&mut rig, 0);
        assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 1, "session params apply once");
        render_block(&mut rig, 512);
        assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 1);
        s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("gain".into(), -12.0);
        rig.publish(&s);
        render_block(&mut rig, 1024);
        assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 2, "later session edits apply");
        // Offline: third-party plugins are simply absent (tests never load real ones).
        let out = render_range(&s, Range::new(0, 1024), 512);
        assert!(out[0][100] > 0.3);
    }

    #[test]
    fn a_hosted_plugin_keeps_its_own_values_where_the_session_holds_its_defaults() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let id = "vst3:00000000000000000000000000000002";
        let mut ins = Insert::new(id);
        // Older sessions store every parameter at the default the plugin reports (0 for gain).
        ins.params.insert("gain".into(), 0.0);
        ins.set_state_bytes(b"stored");
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
        let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fake = Stateful { inner: transit_dsp::create("gain").unwrap(), blob: Vec::new(), sets: Arc::clone(&sets) };
        let mut rig = PlanRig::new(512).fake(t, 0, fake);
        rig.publish(&s);
        let count = || sets.load(std::sync::atomic::Ordering::Relaxed);
        render_block(&mut rig, 0);
        assert_eq!(count(), 0, "a stored default is not pushed over the restored plugin");
        render_block(&mut rig, 512);
        assert_eq!(count(), 0);
        // Later edits are pushed, including a return to the default.
        s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("gain".into(), -6.0);
        rig.publish(&s);
        render_block(&mut rig, 1024);
        assert_eq!(count(), 1);
        s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("gain".into(), 0.0);
        rig.publish(&s);
        render_block(&mut rig, 1536);
        assert_eq!(count(), 2);
    }

    /// A built-in gain behind a third-party id that takes timed points, with a log of the
    /// parameter calls: (offset, value), `None` for `set_param`.
    struct Timed {
        inner: Box<dyn Plugin>,
        log: Arc<std::sync::Mutex<Vec<(Option<u32>, f32)>>>,
    }

    impl Plugin for Timed {
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
            self.log.lock().unwrap().push((None, v));
            self.inner.set_param(id, v)
        }
        fn set_param_at(&mut self, offset: u32, id: &str, v: f32) -> bool {
            self.log.lock().unwrap().push((Some(offset), v));
            self.inner.set_param(id, v)
        }
        fn sample_accurate_params(&self) -> bool {
            true
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
    }

    /// #117 review C1: stopped, every block renders at the same position. A breakpoint inside
    /// that block must not go out (it would flip the parameter between the playhead's value and
    /// the breakpoint's every block); the playhead's value arrives once. Playing, it goes out at
    /// its offset.
    #[test]
    fn a_stopped_transport_sends_the_playhead_value_once_and_no_breakpoints() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let mut ins = Insert::new("clap:test.timed");
        ins.params.insert("gain".into(), -6.0);
        let tr = s.track_mut(t).unwrap();
        tr.mixer.inserts[0] = Some(ins);
        tr.mixer.automation_mode = transit_model::AutomationMode::Read;
        let mut lane = transit_model::AutomationLane::new(AutoParam::Plugin { slot: 0, param: "gain".into() });
        // The block at 512 holds the breakpoint at 1000 (offset 488).
        lane.set_point(0, -6.0);
        lane.set_point(1000, -12.0);
        tr.automation.push(lane);
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let fake = Timed { inner: transit_dsp::create("gain").unwrap(), log: Arc::clone(&log) };
        let mut rig = PlanRig::new(512).fake(t, 0, fake);
        rig.publish(&s);
        rig.eng.monitor_only = true;
        for _ in 0..3 {
            render_block(&mut rig, 512);
        }
        let stopped = std::mem::take(&mut *log.lock().unwrap());
        let start = -6.0 - 6.0 * (512.0 / 1000.0);
        assert_eq!(stopped.len(), 1, "one call over three stopped blocks: {stopped:?}");
        assert_eq!(stopped[0].0, None, "the playhead value, untimed: {stopped:?}");
        assert!((stopped[0].1 - start).abs() < 1e-4, "the curve at the playhead: {stopped:?}");
        rig.eng.monitor_only = false;
        render_block(&mut rig, 512);
        let playing = std::mem::take(&mut *log.lock().unwrap());
        let timed: Vec<_> = playing.iter().filter(|c| c.0.is_some()).copied().collect();
        assert_eq!(timed, vec![(Some(488), -12.0)], "playing, the breakpoint at its offset: {playing:?}");
    }

    /// A sleeping hosted effect whose host skips the blocks `busy` says (its instance held on the
    /// main thread) and keeps the value it was given until a block it runs, as the VST3 and CLAP
    /// hosts do (#78).
    struct Busy {
        inner: Box<dyn Plugin>,
        busy: Arc<std::sync::atomic::AtomicBool>,
        held: Option<f32>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        got: Arc<std::sync::Mutex<Vec<f32>>>,
    }

    impl Plugin for Busy {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, _: &str, v: f32) -> bool {
            self.held = Some(v);
            true
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.busy.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            if let Some(v) = self.held.take() {
                self.got.lock().unwrap().push(v);
                self.inner.set_param("gain", v);
            }
            self.inner.process(io, frames);
        }
        fn process_status(&self) -> ProcessStatus {
            ProcessStatus::Sleep
        }
        fn params_pending(&self) -> bool {
            self.held.is_some()
        }
    }

    /// #78: a value sent to a sleeping insert in a block its host skipped (the instance busy on its
    /// main thread) keeps the insert awake, so the next block delivers it although the input stays
    /// silent and the lane holds the same value (nothing is sent again); then it sleeps again.
    #[test]
    fn a_value_a_skipped_block_kept_reaches_a_sleeping_insert_without_the_lane_changing() {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut s, t) = session_with_dc(0.0, 48_000);
        let mut ins = Insert::new("clap:test.busy");
        ins.params.insert("gain".into(), 0.0);
        let tr = s.track_mut(t).unwrap();
        tr.mixer.inserts[0] = Some(ins);
        tr.mixer.automation_mode = transit_model::AutomationMode::Read;
        let mut lane = transit_model::AutomationLane::new(AutoParam::Plugin { slot: 0, param: "gain".into() });
        lane.set_point(0, -6.0);
        lane.set_point(48_000, -6.0);
        tr.automation.push(lane);
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let fake = Busy {
            inner: transit_dsp::create("gain").unwrap(),
            busy: Arc::clone(&busy),
            held: None,
            calls: Arc::clone(&calls),
            got: Arc::clone(&got),
        };
        let mut rig = PlanRig::new(512).fake(t, 0, fake);
        rig.publish(&s);
        render_block(&mut rig, 0);
        assert_eq!((calls.load(Relaxed), got.lock().unwrap().clone()), (1, vec![]), "the lane's value went out; the host skipped the block");
        busy.store(false, Relaxed);
        render_block(&mut rig, 512);
        assert_eq!((calls.load(Relaxed), got.lock().unwrap().clone()), (2, vec![-6.0]), "kept awake: the next block delivers it");
        render_block(&mut rig, 1024);
        assert_eq!(calls.load(Relaxed), 2, "delivered, silent input: asleep again");
    }
}
