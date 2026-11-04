use super::*;
use std::collections::BTreeSet;
use transit_rt::testing::{CountingAlloc, measure};

#[test]
fn explicit_override_invalidates_unchanged_tables_and_refusal_keeps_the_adopted_set() {
    let mut s = session();
    impulse_track(&mut s, "Audio", Route::Main);
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    let ins = delay(192);
    let uid = ins.uid.get();
    s.track_mut(m).unwrap().mixer.inserts[0] = Some(ins);
    let mut compiler = PlanCompiler::new(target(2), PlanLimits { tracks: 2, ..Default::default() });
    let mut mix = MixEngine::realtime(target(2));
    let mut make = |spec: &InstanceSpec, _| super::make(&s, spec);
    let mut off = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut off), Adoption::Adopted);
    compiler.reclaim(off);
    let mut on = compiler
        .compile_with_bypass(
            Arc::new(s.clone()),
            compiler.active(),
            PlanGeneration::from_raw(2),
            transit_mix::CompileMode::Update,
            BTreeSet::from([uid]),
            &mut make,
        )
        .unwrap();
    assert!(on.plan().is_structural(), "override changed even though the session identity and description did not");
    assert!(compiler.low_latency_bypasses().is_empty(), "publication alone is not adoption");
    let (adopted, allocations) = measure(|| mix.adopt_plan(&mut on));
    assert_eq!(adopted, Adoption::Adopted);
    assert!(allocations.is_zero());
    compiler.reclaim(on);
    assert_eq!(compiler.low_latency_bypasses(), BTreeSet::from([uid]));
    let mut larger = s.clone();
    larger.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    assert!(
        compiler
            .compile_with_bypass(
                Arc::new(larger),
                compiler.active(),
                PlanGeneration::from_raw(3),
                transit_mix::CompileMode::Update,
                BTreeSet::new(),
                &mut make
            )
            .is_err()
    );
    assert_eq!(compiler.low_latency_bypasses(), BTreeSet::from([uid]));
    assert_eq!(mix.session().to_json().unwrap(), s.to_json().unwrap());
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn run(s: &Session, io: &IoMap, limit: Option<f64>) -> (Rendered, BTreeSet<u64>) {
    MixEngine::force_serial(true);
    let mut mix = MixEngine::realtime(target(io.outputs));
    let mut compiler = PlanCompiler::new(target(io.outputs), PlanLimits::default());
    compiler.set_io(Arc::new(io.clone()));
    compiler.set_low_latency(limit);
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut |spec, _| make(s, spec)).unwrap();
    let (adopted, allocation) = measure(|| mix.adopt_plan(&mut env));
    assert_eq!(adopted, Adoption::Adopted);
    assert!(allocation.is_zero(), "{allocation:?}");
    compiler.reclaim(env);
    let width = transit_mix::main_channels(s) + plan_sinks(s, io, io.outputs).channels();
    let mut out = vec![vec![0.0; BLOCK]; width];
    let mut channels = vec![Vec::new(); width];
    for at in (0..LEN).step_by(BLOCK) {
        transit_mix::mark_audio_thread();
        let (_, allocation) = measure(|| mix.render_active(at as i64, BLOCK, &mut out));
        transit_mix::unmark_audio_thread();
        assert!(allocation.is_zero(), "{allocation:?}");
        for (dst, src) in channels.iter_mut().zip(&out) {
            dst.extend_from_slice(src);
        }
    }
    assert_eq!(mix.status().engine_latency as usize, mix.latency);
    (Rendered { channels, latency: mix.latency, main: transit_mix::main_channels(s) }, compiler.low_latency_bypasses())
}

#[test]
fn master_audio_is_early_only_with_mode_and_full_session_save_load_is_unchanged() {
    let mut s = session();
    impulse_track(&mut s, "Live", Route::Main);
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    let ins = delay(192);
    let uid = ins.uid.get();
    s.track_mut(master).unwrap().mixer.inserts[0] = Some(ins);
    let before = s.to_json().unwrap();
    let (off, set) = run(&s, &wide_io(), None);
    assert!(set.is_empty());
    assert_eq!(hits(&off.channels[0]), [AT + 192]);
    let (on, set) = run(&s, &wide_io(), Some(0.0));
    assert_eq!(hits(&on.channels[0]), [AT]);
    assert_eq!(set, BTreeSet::from([uid]));
    assert_eq!(s.to_json().unwrap(), before);
    assert_eq!(Session::from_json(&before).unwrap().to_json().unwrap(), before);
    let (retained, set) = run(&s, &wide_io(), Some(5.0));
    assert!(set.is_empty());
    assert_eq!(hits(&retained.channels[0]), [AT + 192]);
}

#[test]
fn cumulative_limit_uses_numeric_ties_and_manual_bypass_wins() {
    let mut s = session();
    let t = impulse_track(&mut s, "Monitored only", Route::Main);
    let a = delay(192);
    let b = delay(192);
    let first = a.uid.get().min(b.uid.get());
    let tr = s.track_mut(t).unwrap();
    tr.mixer.input_monitor = true;
    tr.mixer.inserts[0] = Some(b);
    tr.mixer.inserts[1] = Some(a);
    let (out, set) = run(&s, &wide_io(), Some(5.0));
    assert_eq!(set, BTreeSet::from([first]));
    assert_eq!(hits(&out.channels[0]), [AT + 192]);
    let (out, set) = run(&s, &wide_io(), Some(0.0));
    assert_eq!(set.len(), 2);
    assert_eq!(hits(&out.channels[0]), [AT]);
    s.track_mut(t).unwrap().mixer.inserts.iter_mut().flatten().find(|i| i.uid.get() == first).unwrap().bypass = true;
    let (_, set) = run(&s, &wide_io(), Some(0.0));
    assert!(!set.contains(&first));
    assert_eq!(set.len(), 1);
    let (off, set) = run(&s, &wide_io(), None);
    assert!(set.is_empty());
    assert_eq!(hits(&off.channels[0]), [AT + 192]);
}

#[test]
fn bus_readers_and_direct_outputs_follow_live_seeds_without_walking_backwards() {
    let mut s = session();
    let bus = bus(&mut s, "Mix bus");
    let a = impulse_track(&mut s, "Live", Route::Bus(bus));
    let b = impulse_track(&mut s, "Other feeder", Route::Bus(bus));
    s.track_mut(a).unwrap().mixer.input_monitor = true;
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(delay(480));
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    let insert = delay(192);
    let uid = insert.uid.get();
    let t = s.track_mut(aux).unwrap();
    t.mixer.input = Route::Bus(bus);
    t.mixer.output = Route::Hardware("Out 3-4".into());
    t.mixer.inserts[0] = Some(insert);
    let (on, set) = run(&s, &wide_io(), Some(0.0));
    assert_eq!(set, BTreeSet::from([uid]));
    assert_eq!(hits(&on.channels[on.main]), [AT + 480]);
    assert!((on.channels[on.main][AT + 480] - 1.0).abs() < 1e-5);
    s.track_mut(b).unwrap().mixer.record_arm = true;
    s.track_mut(a).unwrap().mixer.input_monitor = false;
    assert!(run(&s, &wide_io(), Some(0.0)).1.contains(&uid));
    s.track_mut(b).unwrap().mixer.record_arm = false;
    assert!(run(&s, &wide_io(), Some(0.0)).1.is_empty());
}

#[test]
fn unrelated_tracks_remain_aligned_with_a_dominant_other_sink_and_all_cue_mode_combinations() {
    let mut s = session();
    let a = impulse_track(&mut s, "Dry", Route::Main);
    let b = impulse_track(&mut s, "Playback", Route::Main);
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(delay(128));
    let live = impulse_track(&mut s, "Direct monitored", Route::Hardware("Out 3-4".into()));
    s.track_mut(live).unwrap().mixer.input_monitor = true;
    s.track_mut(live).unwrap().mixer.inserts[0] = Some(delay(192));
    let dominant = impulse_track(&mut s, "Dominant", Route::Hardware("Out 5-6".into()));
    s.track_mut(dominant).unwrap().mixer.inserts[0] = Some(delay(512));
    for cue in [false, true] {
        let mut io = wide_io();
        io.set_live("Out 3-4", cue).unwrap();
        for on in [false, true] {
            let (out, set) = run(&s, &io, on.then_some(0.0));
            assert_eq!(hits(&out.channels[0]), [AT + 512]);
            assert!((out.channels[0][AT + 512] - 1.0).abs() < 1e-5, "unequal playback chains sum at one frame");
            assert_eq!(hits(&out.channels[out.main]), [AT + if cue { if on { 0 } else { 192 } } else { 512 }]);
            assert_eq!(set.len(), usize::from(on));
        }
    }
    assert!(!s.track(a).unwrap().mixer.input_monitor);
}

#[test]
fn frame_limit_conversion_and_offline_compiler_ignore_machine_mode() {
    assert_eq!(transit_mix::low_latency_frames(4.999, SR), 239);
    assert_eq!(transit_mix::low_latency_frames(f64::MAX, SR), 1 << 20);
    let mut s = session();
    impulse_track(&mut s, "Live", Route::Main);
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    let mut i = Insert::new("compressor");
    i.params.insert("lookahead".into(), 4.0);
    s.track_mut(m).unwrap().mixer.inserts[0] = Some(i);
    let offline_before = render_range(&s, Range::new(0, LEN as i64), BLOCK);
    let (_, set) = run(&s, &wide_io(), Some(0.0));
    assert_eq!(set.len(), 1);
    assert_eq!(run(&s, &wide_io(), None).0.latency, 192);
    assert_eq!(render_range(&s, Range::new(0, LEN as i64), BLOCK), offline_before, "headless render ignores the live override");
}

#[test]
fn an_instrument_above_the_limit_keeps_sounding_and_midi_effects_keep_processing() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Voice {
        note: Arc<AtomicUsize>,
        sounding: bool,
    }
    impl Plugin for Voice {
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
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            for c in io {
                c[..frames].fill(if self.sounding { 0.25 } else { 0.0 });
            }
        }
        fn latency(&self) -> usize {
            600
        }
        fn event(&mut self, _: u32, e: &transit_mix::PluginEvent) -> bool {
            if let transit_mix::Message::NoteOn { key, .. } = e.message {
                self.note.store(usize::from(key), Ordering::Relaxed);
                self.sounding = true;
            }
            true
        }
    }
    let mut s = session();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = true;
    tr.instrument = Some(Insert::new("gain"));
    let mut midi = transit_model::MidiEffectSlot::new("midi_transpose");
    midi.params.insert("semitones".into(), 12.0);
    tr.midi_effects.push(midi);
    let mut seq = transit_midi::Sequence::default();
    seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start: 0, length: 24 });
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "Note", 0, 48000, seq));
    let ins = delay(192);
    let uid = ins.uid.get();
    tr.mixer.inserts[0] = Some(ins);
    let note = Arc::new(AtomicUsize::new(0));
    MixEngine::force_serial(true);
    let mut mix = MixEngine::realtime(target(2));
    let mut compiler = PlanCompiler::new(target(2), PlanLimits::default());
    compiler.set_low_latency(Some(0.0));
    let mut env = compiler
        .compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut |spec, _| {
            if spec.slot == transit_mix::INSTRUMENT_SLOT {
                Some(NewInstance { plugin: Box::new(Voice { note: Arc::clone(&note), sounding: false }), refused_state: false })
            } else {
                make(&s, spec)
            }
        })
        .unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let mut out = vec![vec![0.0; BLOCK]; 2];
    mix.render_active(0, BLOCK, &mut out);
    assert_eq!(compiler.low_latency_bypasses(), BTreeSet::from([uid]));
    assert_eq!(note.load(Ordering::Relaxed), 72, "the protected MIDI effect transposes the note");
    assert!(out[0].iter().any(|x| *x > 0.1), "the over-budget instrument sounds");
    assert_eq!(mix.latency, 600);
}
