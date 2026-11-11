//! Native-rate import and playback regressions. All audio is generated in process.
use super::*;

fn wav(rate: u32, data: Vec<f32>) -> Vec<u8> {
    transit_audio_io::encode(
        &AudioBuffer { sample_rate: rate, channels: vec![data] },
        &EncodeOptions { bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() },
    )
    .unwrap()
}

fn loaded(rate: u32, data: Vec<f32>) -> (Engine, TrackId) {
    let mut e = Engine::default();
    let t = instrument(&mut e);
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    super::super::load_zone_bytes(&mut e, t, 0, "native.wav", &wav(rate, data)).unwrap();
    e.execute(
        "mix.sampler_zone_edit",
        &json!({"track": t, "zone": 1, "playback": {
            "mode": "one_shot", "attack_ms": 0, "decay_ms": 0, "release_ms": 0,
            "declick_ms": 0, "sustain": 1, "velocity_sensitivity": 0
        }}),
    )
    .unwrap();
    notes(&mut e, t, &[(60, 0, 1)]);
    (e, t)
}

fn triangle(rate: u32) -> Vec<f32> {
    // Exactly six periods of 300 Hz in 20 ms. No transcendental function or pinned hash.
    let period = rate / 300;
    (0..rate / 50)
        .map(|i| {
            let phase = ((i % period) as f32 / period as f32 + 0.125) % 1.0;
            1.0 - 4.0 * (phase - 0.5).abs()
        })
        .collect()
}

fn compare_rate(rate: u32) {
    let (reference, _) = loaded(48_000, triangle(48_000));
    let reference = render(&reference, 2048);
    let length = |x: &[f32]| x.iter().rposition(|v| *v != 0.0).map_or(0, |n| n + 1);
    let cycles = |x: &[f32]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert_eq!(length(&reference[0]), 960);
    assert_eq!(cycles(&reference[0][..960]), 6);
    {
        let (e, _) = loaded(rate, triangle(rate));
        let out = render(&e, 2048);
        // Cubic interpolation of differently sampled triangles is not bit-identical at corners.
        // Duration and the number of complete periods are exact invariants, with no tolerance.
        assert_eq!(length(&out[0]), length(&reference[0]), "{rate}: output frames");
        assert_eq!(cycles(&out[0][..960]), cycles(&reference[0][..960]), "{rate}: periods");
    }
}

#[test]
fn native_44100_renders_the_session_rate_pitch_and_length() {
    compare_rate(44_100);
}

#[test]
fn native_96000_renders_the_session_rate_pitch_and_length() {
    compare_rate(96_000);
}

#[test]
fn native_media_survives_save_reopen_and_a_shared_timeline_path() {
    let d = Dir::new("native-save");
    let path = d.path("native.wav");
    let data = triangle(44_100);
    std::fs::write(&path, wav(44_100, data.clone())).unwrap();
    let (mut e, t) = loaded(44_100, data.clone());
    // The referenced source and timeline name the same original path, at different admitted rates.
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": s(&path)})).unwrap();
    e.execute("file.import_audio", &json!({"path": s(&path)})).unwrap();
    let sid = patch(&e, t).sources[0].source;
    let timeline = e.session().sources.last().unwrap().id;
    // Both sources keep the path so reopen must key the decode cache by rate as well as path.
    let song = d.path("native.transit");
    let text = e.session().to_json().unwrap();
    std::fs::write(&song, text).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": s(&song)})).unwrap();
    assert_eq!(back.session().pool.get(sid).unwrap().buffer.sample_rate, 44_100);
    assert_eq!(back.session().pool.get(sid).unwrap().buffer.channels[0], data);
    assert_eq!(back.session().pool.get(timeline).unwrap().buffer.sample_rate, 48_000);
    // Managed bytes use the same stored-rate policy through ordinary Save As.
    let (mut e, t) = loaded(96_000, triangle(96_000));
    let before = render(&e, 2048);
    let song = d.path("managed.transit");
    e.execute("session.save_as", &json!({"path": s(&song)})).unwrap();
    back.execute("session.open", &json!({"path": s(&song)})).unwrap();
    let sid = patch(&back, t).sources[0].source;
    assert_eq!(back.session().source(sid).unwrap().sample_rate, 96_000);
    assert_eq!(back.session().pool.get(sid).unwrap().buffer.sample_rate, 96_000);
    assert!(same_bits(&before, &render(&back, 2048)));
}

#[test]
fn kit_pad_drop_and_slice_loop_store_identical_native_pcm() {
    let d = Dir::new("native-kit");
    let data = triangle(44_100);
    let path = d.path("hit.wav");
    std::fs::write(&path, wav(44_100, data.clone())).unwrap();
    let (mut e, t) = loaded(44_100, data.clone());
    e.execute("mix.sampler_pad_bind", &json!({"track":t,"pad":1,"zone":1})).unwrap();
    let preview = e.execute("mix.sampler_load_kit", &json!({"track":t,"folder":s(&d.0),"preview":true})).unwrap();
    assert_eq!(preview["pads"][0]["frames"], 882);
    let kit = e.execute("mix.sampler_load_kit", &json!({"track":t,"folder":s(&d.0)})).unwrap();
    assert_eq!(kit["loaded"], 1);
    e.execute("mix.sampler_zone_source", &json!({"track":t,"zone":3,"pad":3,"path":s(&path)})).unwrap();
    e.execute("mix.sampler_slice_loop", &json!({"track":t,"path":s(&path)})).unwrap();
    for source in &e.session().sources {
        let audio = e.session().pool.get(source.id).unwrap();
        assert_eq!((source.sample_rate, audio.buffer.sample_rate), (44_100, 44_100));
        assert_eq!(audio.buffer.channels[0], data);
    }
    let state = e.execute("mix.sampler_slice_state", &json!({"track":t,"at":0})).unwrap();
    assert_eq!(state["loop"]["rate"], 44_100);
    assert_eq!(state["loop"]["end"], 882);
    assert_eq!(state["loop"]["default_beat_length"], 0.04);
}

#[test]
fn native_loop_regions_read_the_same_audio_as_session_rate_regions() {
    let data: Vec<f32> = (0..480).map(|i| ((i * 17 % 97) as f32 - 48.0) / 64.0).collect();
    let doubled: Vec<f32> = data.iter().flat_map(|x| [*x, *x]).collect();
    for reverse in [false, true] {
        let (mut native, t) = loaded(96_000, doubled.clone());
        let (mut reference, r) = loaded(48_000, data.clone());
        for (e, t, m) in [(&mut native, t, 2), (&mut reference, r, 1)] {
            e.execute(
                "mix.sampler_zone_edit",
                &json!({"track":t,"zone":1,
                "region":{"start":100*m,"end":300*m,"loop_start":120*m,"loop_end":280*m,"loop_mode":"forward"},
                "playback":{"mode":"gate","reverse":reverse}}),
            )
            .unwrap();
        }
        assert!(same_bits(&render(&native, 2048), &render(&reference, 2048)), "reverse={reverse}");
        // The same source-frame region reached through a slice binding and its derived loop.
        for (e, t, m) in [(&mut native, t, 2), (&mut reference, r, 1)] {
            let sid = patch(e, t).sources[0].source;
            e.execute("mix.sampler_slice_loop", &json!({"track":t,"source":sid,"start":120*m,"end":280*m})).unwrap();
            e.execute("mix.sampler_slice_mode", &json!({"track":t,"mode":"manual"})).unwrap();
            e.execute("mix.sampler_slice_marker", &json!({"track":t,"op":"add","frame":200*m,"snap":false})).unwrap();
            e.execute("mix.sampler_zone_clear", &json!({"track":t,"zone":1})).unwrap();
            e.execute("mix.sampler_pad_start", &json!({"track":t,"note":60})).unwrap();
            e.execute("mix.sampler_slices_to_pads", &json!({"track":t})).unwrap();
            e.execute(
                "mix.sampler_zone_edit",
                &json!({"track":t,"zone":1,"region":{"loop_mode":"forward"},
                "playback":{"mode":"gate","reverse":reverse,"attack_ms":0,"decay_ms":0}}),
            )
            .unwrap();
        }
        assert!(same_bits(&render(&native, 2048), &render(&reference, 2048)), "slice reverse={reverse}");
    }
}

#[test]
fn a_converted_staged_import_is_refused_without_consuming_it() {
    let d = Dir::new("native-staged");
    let path = d.path("hit.wav");
    std::fs::write(&path, wav(44_100, triangle(44_100))).unwrap();
    let (mut e, t) = loaded(48_000, triangle(48_000));
    let job = e.imports.prepare_here_for_testing(&s(&path), crate::io::ImportRate::Session(48_000));
    let before = e.session_arc();
    let err = e.execute("mix.sampler_zone_source", &json!({"track":t,"zone":2,"import":job})).unwrap_err();
    assert!(err.to_string().contains("rate: native"));
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert!(e.imports.ready(job).is_ok());
}

#[test]
fn native_slice_positions_keep_their_time_in_both_midi_modes() {
    let (mut e, t) = loaded(44_100, vec![0.25; 44_100]);
    let sid = patch(&e, t).sources[0].source;
    e.execute("mix.sampler_slice_loop", &json!({"track":t,"source":sid})).unwrap();
    for frame in [11_025, 33_075] {
        e.execute("mix.sampler_slice_marker", &json!({"track":t,"op":"add","frame":frame,"snap":false})).unwrap();
    }
    // Remove the loose zone before mapping its source's slices.
    e.execute("mix.sampler_zone_clear", &json!({"track":t,"zone":1})).unwrap();
    e.execute("mix.sampler_slices_to_pads", &json!({"track":t})).unwrap();
    let keep = e.execute("mix.slice_to_midi", &json!({"track":t,"timing":"keep","at":0,"preview":true})).unwrap();
    let fit = e.execute("mix.slice_to_midi", &json!({"track":t,"timing":"fit","beat_length":4,"at":0,"preview":true})).unwrap();
    assert_eq!(keep["ticks"], json!([0, 480, 1440, 1920]));
    assert_eq!(fit["ticks"], json!([0, 960, 2880, 3840]));
    let state = e.execute("mix.sampler_slice_state", &json!({"track":t,"at":0})).unwrap();
    assert_eq!(state["loop"]["rate"], 44_100);
    assert_eq!(state["loop"]["default_beat_length"], 2.0);
    let p = patch(&e, t);
    let starts: Vec<u64> = p.zones.iter().map(|z| z.start).collect();
    assert_eq!(starts, [0, 11_025, 33_075]);
    let err = e
        .execute(
            "mix.sampler_zone_edit",
            &json!({"track":t,"zone":2,
        "region":{"loop_mode":"forward","crossfade":11026},"playback":{"mode":"gate"}}),
        )
        .unwrap_err();
    assert!(err.to_string().contains("250.0 ms (11025 frames)"), "{err}");
    e.session_mut().pool.remove(sid);
    let state = e.execute("mix.sampler_slice_state", &json!({"track":t,"at":0})).unwrap();
    assert_eq!(state["loop"]["rate"], 44_100, "missing audio retains its stored rate readout");
    let zones = e.execute("mix.sampler_state", &json!({"track":t})).unwrap();
    assert_eq!(zones["zones"][0]["rate"], 44_100);
}

#[test]
fn previously_converted_pcm_round_trips_bit_identically() {
    let d = Dir::new("old-rate");
    let original = triangle(44_100);
    let path = d.path("old-original.wav");
    std::fs::write(&path, wav(44_100, original.clone())).unwrap();
    // Reconstruct main's admission: the same offline resampler, then 48 kHz stored metadata.
    // Keep the original external path, as older referenced kits did.
    let converted = transit_dsp::offline::resample(&[original], 44_100, 48_000).remove(0);
    let (mut e, t) = loaded(48_000, converted.clone());
    let sid = patch(&e, t).sources[0].source;
    let src = e.session_mut().sources.iter_mut().find(|s| s.id == sid).unwrap();
    src.path = s(&path);
    src.unsaved = false;
    e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch.as_mut().unwrap().ready_mut().unwrap().sources[0].ownership =
        Ownership::Referenced;
    let mut z = dsp::Zone::new(dsp::Region::whole(0, converted.len() as u64));
    z.playback = dsp::Playback {
        mode: dsp::PlayMode::OneShot,
        attack_ms: 0.0,
        decay_ms: 0.0,
        release_ms: 0.0,
        velocity_sensitivity: 0.0,
        ..dsp::Playback::zone()
    };
    let sample: Arc<dyn transit_dsp::SampleSource> = Arc::new(transit_dsp::SampleBuffer { sample_rate: 48_000.0, channels: vec![converted] });
    let reference = direct(dsp::PatchSpec { sources: vec![sample], zones: vec![Some(z)], ..dsp::PatchSpec::default() }, 1, &Default::default(), 2048);
    assert!(same_bits(&reference, &render(&e, 2048)), "main's converted PCM through the unchanged adapter");
    let song = d.path("old.transit");
    e.execute("session.save_as", &json!({"path":s(&song)})).unwrap();
    let mut reopened = Engine::default();
    reopened.execute("session.open", &json!({"path":s(&song)})).unwrap();
    assert!(same_bits(&reference, &render(&reopened, 2048)), "old referenced PCM is converted to its stored rate exactly as main did");
}
