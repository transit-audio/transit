//! Rule: tests never call the real plug-in hosts. Third-party ids are not created here
//! (create_plugin returns None under cfg(test)); a test that needs a hosted plug-in
//! hands a fake to `PlanRig::fake`.

use super::*;

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_model::{Insert, SourceAudio, SourceId};

/// A plan engine for tests. A `fake` stands in for the instance of its (track, slot); every
/// other plug-in is built as playback builds it.
pub(crate) struct PlanRig {
    pub(crate) eng: MixEngine,
    comp: PlanCompiler,
    pub(crate) fakes: Vec<(TrackId, usize, Box<dyn Plugin>)>,
    issued: u64,
    block: usize,
}

impl PlanRig {
    pub(crate) fn new(block: usize) -> Self {
        let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: block as u32, out_channels: 0, in_channels: 0 };
        PlanRig { eng: MixEngine::realtime(target), comp: PlanCompiler::new(target, PlanLimits::default()), fakes: Vec::new(), issued: 0, block }
    }

    pub(crate) fn fake(mut self, track: TrackId, slot: usize, plugin: impl Plugin + 'static) -> Self {
        self.fakes.push((track, slot, Box::new(plugin)));
        self
    }

    /// Compiles `s` and adopts it.
    pub(crate) fn publish(&mut self, s: &Session) {
        assert_eq!(self.try_publish(s), Adoption::Adopted);
    }

    /// Compiles `s` and offers it to the engine: what the adoption said.
    pub(crate) fn try_publish(&mut self, s: &Session) -> Adoption {
        self.issued += 1;
        let (sr, block, fakes) = (s.sample_rate.as_f64() as f32, self.block, &mut self.fakes);
        let mut make = |spec: &InstanceSpec, _: SlotGen| match fakes.iter().position(|(t, slot, _)| (*t, *slot) == (spec.track, spec.slot)) {
            Some(i) => Some(NewInstance { plugin: fakes.swap_remove(i).2, refused_state: false }),
            None => create_instance(&spec.id, spec.state.as_deref(), sr, block, spec.ch),
        };
        let mut env = self.comp.compile(Arc::new(s.clone()), self.comp.active(), PlanGeneration::from_raw(self.issued), &mut make).unwrap();
        let adopted = self.eng.adopt_plan(&mut env);
        self.comp.reclaim(env);
        adopted
    }

    /// Renders one block at `pos` and returns it.
    pub(crate) fn render(&mut self, pos: i64) -> Vec<Vec<f32>> {
        let mut out = vec![vec![0.0f32; self.block]; self.eng.main_channels().max(2)];
        self.eng.render_active(pos, self.block, &mut out);
        out
    }
}

pub(crate) fn synth_session(level_db: f32) -> Session {
    use transit_midi::{Note, Sequence};
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 24_000, seq));
    let mut ins = Insert::new("macro_oscillator");
    ins.params.insert("level".into(), level_db);
    tr.instrument = Some(ins);
    s
}

#[test]
fn live_notes_sound_while_stopped_without_the_clips() {
    let s = synth_session(-6.0);
    let t = s.tracks[0].id;
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.monitor_only = true;
    let peak = |out: &[Vec<f32>]| out.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
    let mut loudest = 0.0f32;
    for _ in 0..8 {
        loudest = loudest.max(peak(&rig.render(0)));
    }
    assert_eq!(loudest, 0.0, "stopped: the clip's note must not play");
    rig.eng.live_note(t, 72, 100);
    for _ in 0..8 {
        loudest = loudest.max(peak(&rig.render(0)));
    }
    assert!(loudest > 0.01, "the auditioned note should sound: {loudest}");
    rig.eng.live_note(t, 72, 0);
    let mut last = Vec::new();
    for _ in 0..400 {
        last = rig.render(0);
    }
    assert!(peak(&last) < 1e-4, "the released note should die away: {}", peak(&last));
}

/// `live_note` goes through the event host's `give` (#246), so an insert that reads notes hears the
/// auditioned note: the Filter's amp stage passes nothing until a note opens it.
#[test]
fn live_notes_reach_the_inserts_that_read_notes() {
    let mut s = synth_session(-6.0);
    let t = s.tracks[0].id;
    let mut f = Insert::new("filter");
    f.params.insert("amp_on".into(), 1.0);
    f.params.insert("cutoff".into(), 20_000.0);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(f);
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.monitor_only = true;
    let peak = |out: &[Vec<f32>]| out.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
    let mut loudest = 0.0f32;
    for _ in 0..8 {
        loudest = loudest.max(peak(&rig.render(0)));
    }
    assert_eq!(loudest, 0.0, "no note: the amp stage is closed");
    rig.eng.live_note(t, 72, 100);
    for _ in 0..8 {
        loudest = loudest.max(peak(&rig.render(0)));
    }
    assert!(loudest > 0.01, "the Filter heard the auditioned note: {loudest}");
}

pub(crate) fn session_with_dc(level: f32, frames: usize) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = add_dc_track(&mut s, level, frames);
    (s, t)
}

pub(crate) fn add_dc_track(s: &mut Session, level: f32, frames: usize) -> TrackId {
    // A mono strip carrying an instrument (the one-channel buffer is what takes the scratch path).
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; frames]] };
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(buf)));
    let id = s.new_clip_id();
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips.push(Clip::audio(id, "dc", SourceId(900), 0, 0, frames as i64));
    }
    t
}

#[test]
fn every_builtin_parameter_map_is_tracked_inline() {
    for info in transit_dsp::plugins() {
        assert!(info.params.len() <= APPLIED_INLINE, "{}: {} parameters", info.id, info.params.len());
    }
    let mut a = Applied::new();
    assert!(a.values(3).iter().all(|x| x.is_nan()));
    a.values(3).fill(1.0);
    assert_eq!(a.values(3), [1.0; 3], "kept while the map's length stays");
    assert!(a.values(APPLIED_INLINE + 1).is_empty(), "unprepared long maps never allocate");
    let mut a = Applied::prepared(vec![f32::NAN; APPLIED_INLINE + 1]);
    assert_eq!(a.values(APPLIED_INLINE + 1).len(), APPLIED_INLINE + 1);
}

#[test]
fn clip_effect_slots_round_trip_and_never_collide() {
    for clip in [0u64, 1, 7, 1 << 20] {
        for comp in [false, true] {
            let slot = clip_fx_slot(clip, comp).unwrap();
            assert_eq!(clip_fx_of_slot(slot), Some((clip, comp)));
            assert_ne!(slot, INSTRUMENT_SLOT);
        }
    }
    assert_eq!(clip_fx_of_slot(INSTRUMENT_SLOT), None);
    assert_eq!(clip_fx_of_slot(3), None, "insert slots are not clip-effect slots");
    assert_eq!(clip_fx_slot(u64::MAX, false), None);
}

#[test]
fn mono_center_pan_is_minus_3db() {
    let (s, _) = session_with_dc(0.5, 4800);
    let out = render_range(&s, Range::new(0, 4800), 512);
    let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((out[0][1000] - expect).abs() < 1e-3, "{}", out[0][1000]);
    assert!((out[1][1000] - expect).abs() < 1e-3);
}

#[test]
fn fader_mute_and_solo() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.track_mut(t).unwrap().mixer.volume_db = -6.0;
    let out = render_range(&s, Range::new(0, 4800), 512);
    let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
    assert!((out[0][2000] - expect).abs() < 1e-3);
    s.track_mut(t).unwrap().mixer.mute = true;
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][2000].abs() < 1e-6);
    // Soloing another track mutes this one.
    s.track_mut(t).unwrap().mixer.mute = false;
    let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.track_mut(other).unwrap().mixer.solo = true;
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][2000].abs() < 1e-6);
}

#[test]
fn hard_pans() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
    let out = render_range(&s, Range::new(0, 4800), 256);
    assert!(out[0][100] > 0.49 && out[1][100].abs() < 1e-4);
}

#[test]
fn fade_curvature_reaches_the_rendered_audio() {
    for fade_in in [true, false] {
        for curvature in [-0.5, 0.0, 0.5] {
            let (mut s, t) = session_with_dc(1.0, 8192);
            s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
            let fade = transit_model::Fade { len: 8192, shape: transit_model::FadeShape::SCurve, curvature };
            let c = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
            if fade_in {
                c.fade_in = fade;
            } else {
                c.fade_out = fade;
            }
            let out = render_range(&s, Range::new(0, 8192), 512);
            let expected = if curvature < 0.0 {
                0.2
            } else if curvature > 0.0 {
                0.8
            } else {
                0.5
            };
            assert!((out[0][4096] - expected).abs() < 1e-5, "{fade_in}, {curvature}: {} vs {expected}", out[0][4096]);
            assert!(out[1][4096].abs() < 1e-6);
            assert!(out[0][if fade_in { 0 } else { 8191 }].abs() < 1e-4);
        }
    }
}

#[test]
fn pan_automation_does_not_depend_on_the_block_size() {
    // Bounces render in 1024-frame blocks, playback in whatever its callback asks for (#244).
    let (mut s, t) = session_with_dc(0.5, 4800);
    {
        let l = s.track_mut(t).unwrap().lane_mut(&AutoParam::Pan(0));
        l.set_point(0, -1.0);
        l.set_point(480, 1.0);
    }
    let bounce = render_range(&s, Range::new(0, 4800), 1024);
    assert!(bounce[0][600].abs() < 1e-4 && bounce[1][600] > 0.49, "{} {}", bounce[0][600], bounce[1][600]);
    for block in [512, 100, 37] {
        assert_eq!(render_range(&s, Range::new(0, 4800), block), bounce, "block {block}");
    }
}

#[test]
fn fades_and_clip_gain_apply() {
    let (mut s, t) = session_with_dc(1.0, 48_000);
    {
        let c = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
        c.fade_in = transit_model::Fade { len: 1000, shape: transit_model::FadeShape::Linear, curvature: 0.0 };
        c.gain_db = -6.0;
    }
    let out = render_range(&s, Range::new(0, 2000), 512);
    let g = std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
    assert!(out[0][0].abs() < 1e-3);
    assert!((out[0][500] - 0.5 * g).abs() < 0.02, "{}", out[0][500]);
    assert!((out[0][1500] - g).abs() < 1e-3);
}

#[test]
fn sends_feed_aux_via_bus() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let bus = s.add_bus("Verb", ChannelFormat::Stereo);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    {
        let tr = s.track_mut(t).unwrap();
        tr.mixer.output = Route::None;
        let mut snd = transit_model::SendSlot::new(Route::Bus(bus));
        snd.level_db = 0.0;
        tr.mixer.sends[0] = Some(snd);
    }
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][1000] > 0.1, "aux should carry the send: {}", out[0][1000]);
}

#[test]
fn render_bus_is_the_bus_not_the_main_mix() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let stem = s.add_bus("Stem", ChannelFormat::Stereo);
    let empty = s.add_bus("Empty", ChannelFormat::Stereo);
    s.track_mut(t).unwrap().mixer.output = Route::Bus(stem);
    let main = render_range(&s, Range::new(0, 4800), 512);
    let bus = render_bus(&s, stem, Range::new(0, 4800), 512);
    let silent = render_bus(&s, empty, Range::new(0, 4800), 512);
    assert!(main[0][1000].abs() < 1e-4, "main should not carry a track routed to the bus: {}", main[0][1000]);
    assert!(bus[0][1000].abs() > 0.1, "bus should carry the track: {}", bus[0][1000]);
    assert!(silent.iter().all(|c| c.iter().all(|x| x.abs() < 1e-6)));
}

#[test]
fn volume_automation_ramps() {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    {
        let tr = s.track_mut(t).unwrap();
        let l = tr.lane_mut(&AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(24_000, -144.0);
    }
    let out = render_range(&s, Range::new(0, 48_000), 512);
    assert!(out[0][100] > 0.3);
    assert!(out[0][30_000].abs() < 1e-4);
}

#[test]
fn inserts_process_and_survive_unknown_plugins() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("does-not-exist"));
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][100] > 0.3);
    let id = transit_dsp::plugins().iter().find(|p| p.id.contains("gain")).map(|p| p.id).unwrap_or("gain");
    let mut ins = Insert::new(id);
    ins.params.insert("gain".into(), -6.0);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
    let _ = render_range(&s, Range::new(0, 4800), 512);
}

#[derive(Default)]
struct Calls {
    on: Vec<u8>,
    all_off: usize,
    resets: usize,
    processed: usize,
    /// `(frames, shortest channel length)` of every `process` call.
    shapes: Vec<(usize, usize)>,
}

struct Spy {
    inner: Box<dyn Plugin>,
    calls: Arc<std::sync::Mutex<Calls>>,
}

impl Plugin for Spy {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.calls.lock().unwrap().resets += 1;
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        {
            let mut c = self.calls.lock().unwrap();
            c.processed += 1;
            c.shapes.push((frames, io.iter().map(Vec::len).min().unwrap_or(0)));
        }
        self.inner.process(io, frames);
    }
    fn note_on(&mut self, _offset: usize, note: u8, _velocity: u8) {
        self.calls.lock().unwrap().on.push(note);
    }
    fn all_notes_off(&mut self) {
        self.calls.lock().unwrap().all_off += 1;
    }
}

#[test]
fn all_notes_off_releases_every_held_note_without_resetting() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.spy"));
    let mut seq = transit_midi::Sequence::default();
    seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 48_000 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000 * 4, seq));
    let calls = Arc::new(std::sync::Mutex::new(Calls::default()));
    let spy = Spy { inner: transit_dsp::create("gain").unwrap(), calls: Arc::clone(&calls) };
    let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, spy);
    rig.publish(&s);
    render_block(&mut rig, 0);
    let (offs, resets) = {
        let c = calls.lock().unwrap();
        assert_eq!(c.on, vec![60]);
        (c.all_off, c.resets)
    };
    rig.eng.all_notes_off();
    let c = calls.lock().unwrap();
    assert_eq!(c.all_off, offs + 1, "the instrument was told to release");
    assert_eq!(c.resets, resets, "reset would discard the queued releases");
}

#[test]
fn flush_releases_processes_every_instrument_once_without_scheduling() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.spy"));
    let mut seq = transit_midi::Sequence::default();
    seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000, seq));
    let calls = Arc::new(std::sync::Mutex::new(Calls::default()));
    let spy = Spy { inner: transit_dsp::create("gain").unwrap(), calls: Arc::clone(&calls) };
    let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, spy);
    rig.publish(&s);
    rig.eng.flush_releases(512);
    {
        let c = calls.lock().unwrap();
        assert_eq!(c.processed, 1, "one pass");
        assert!(c.on.is_empty(), "nothing was scheduled");
    }
    render_block(&mut rig, 0);
    assert_eq!(calls.lock().unwrap().on, vec![60], "the position was untouched, so the first note still schedules");
}

#[test]
fn flush_releases_hands_a_mono_instrument_full_length_buffers_before_any_render() {
    let mut s = Session::default();
    // A mono strip carrying an instrument (the one-channel buffer is what takes the scratch path).
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.spy"));
    let calls = Arc::new(std::sync::Mutex::new(Calls::default()));
    let spy = Spy { inner: transit_dsp::create("gain").unwrap(), calls: Arc::clone(&calls) };
    let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, spy);
    rig.publish(&s);
    let plan = rig.eng.plan.as_deref().unwrap();
    assert_eq!(plan.tables.strips[plan.tables.strip_of(t).unwrap()].buf.len(), 1, "a mono strip");
    rig.eng.flush_releases(512);
    let c = calls.lock().unwrap();
    assert_eq!(c.shapes.len(), 1, "one pass");
    assert_eq!(c.shapes[0].0, 512);
    assert!(c.shapes[0].1 >= 512, "shortest channel of {} samples", c.shapes[0].1);
}

pub(crate) fn render_block(rig: &mut PlanRig, pos: i64) -> f32 {
    rig.render(pos)[0][500]
}

#[test]
fn input_monitoring_replaces_clips() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.input = vec![vec![0.25; 512]];
    // Not monitored: the clip plays.
    let out = rig.render(0);
    assert!((out[0][100] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    // Monitored: the input replaces it, and it still sounds when stopped.
    s.track_mut(t).unwrap().mixer.input_monitor = true;
    rig.publish(&s);
    let out = rig.render(512);
    assert!((out[0][100] - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{}", out[0][100]);
    rig.eng.monitor_only = true;
    let out = rig.render(1024);
    assert!(out[0][100] > 0.1);
    s.track_mut(t).unwrap().mixer.input_monitor = false;
    rig.publish(&s);
    let out = rig.render(1024);
    assert!(out[0][100].abs() < 1e-6, "stopped, unmonitored: silence");
}

#[test]
fn empty_session_is_silent() {
    let s = Session::default();
    let out = render_range(&s, Range::new(0, 1000), 128);
    assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)));
}

#[test]
fn stopped_monitoring_does_not_click() {
    let mut s = Session::default();
    s.edit.click = true;
    let mut rig = PlanRig::new(512);
    rig.publish(&s);
    rig.eng.metronome = true;
    rig.eng.monitor_only = true;
    for _ in 0..4 {
        assert!(rig.render(0).iter().all(|c| c.iter().all(|x| *x == 0.0)));
    }
}
