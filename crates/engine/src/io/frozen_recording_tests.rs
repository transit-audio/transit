use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use transit_dsp::{Plugin, plugins::texture::TextureFx};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("transit-frozen-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn file(&self) -> PathBuf {
        self.0.join("pad.transit")
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(quality: u32, freeze: bool) -> (Engine, TextureFx) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"name":"Pad"})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track":t,"slot":0,"plugin":"texture"})).unwrap();
    let mut device = TextureFx::new();
    let params = [("quality", quality as f32), ("time", 0.0), ("size", 70.0), ("dry_wet", 100.0), ("reverb", 20.0), ("input_gain", 1.0)];
    for (id, v) in params {
        e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap().params.insert(id.into(), v);
        device.set_param(id, v);
    }
    device.prepare(e.session().sample_rate.hz() as f32, 250, 2);
    let mut io = vec![vec![0.0; 250]; 2];
    for block in 0..240 {
        for (c, channel) in io.iter_mut().enumerate() {
            for (n, x) in channel.iter_mut().enumerate() {
                *x = ((block * 250 + n) as f32 * (0.017 + c as f32 * 0.003)).sin() * 0.4;
            }
        }
        device.process(&mut io, 250);
    }
    device.set_param("freeze", f32::from(freeze));
    e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap().params.insert("freeze".into(), f32::from(freeze));
    for _ in 0..5 {
        device.process(&mut io, 250);
    }
    let state = device.save_state().map(|data| transit_model::b64::encode(&data));
    e.execute("mix.insert_state", &json!({"track":t,"slot":0,"state":state})).unwrap();
    (e, device)
}

fn opened(folder: &Folder) -> (Engine, Vec<String>) {
    let mut e = Engine::default();
    let notes = super::super::session::open_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    (e, notes)
}

fn device(e: &Engine) -> Box<dyn Plugin> {
    let spec = transit_mix::realtime_instance_specs(e.session()).into_iter().find(|s| s.id == "texture").unwrap();
    let mut made = transit_mix::create_instance(&spec.id, spec.state.as_deref(), e.session().sample_rate.hz() as f32, 250, 2).unwrap();
    assert!(!made.refused_state);
    for (id, v) in &e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().params {
        made.plugin.set_param(id, *v);
    }
    made.plugin
}

#[test]
fn frozen_session_writes_float_wav_and_resumes_the_next_blocks_bit_exactly() {
    for quality in 0..4 {
        let folder = Folder::new();
        let (mut e, mut live) = fixture(quality, true);
        super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
        let saved = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
        assert!(Path::new(&saved.path).is_relative());
        let bytes = std::fs::read(folder.0.join(&saved.path)).unwrap();
        assert_eq!(&bytes[20..22], &3u16.to_le_bytes());
        assert_eq!(&bytes[34..36], &32u16.to_le_bytes());
        assert_eq!(bytes.len(), 44 + saved.span as usize * saved.channels as usize * 4);
        let (_, audio) = transit_audio_io::decode(&bytes, Some("wav")).unwrap();
        assert_eq!(audio.sample_rate, saved.rate);
        assert_eq!(audio.frames(), saved.span as usize);
        assert_eq!(audio.num_channels(), saved.channels as usize);
        let text = std::fs::read_to_string(folder.file()).unwrap();
        assert!(text.len() < 20_000, "audio must live outside session JSON");
        let (opened, notes) = opened(&folder);
        assert!(notes.is_empty(), "{notes:?}");
        let mut restored = device(&opened);
        let mut heard = false;
        for block in 0..20 {
            let mut a = vec![vec![0.0; 250]; 2];
            let mut b = a.clone();
            live.process(&mut a, 250);
            restored.process(&mut b, 250);
            heard |= a.iter().flatten().any(|x| *x != 0.0);
            assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()), "quality {quality} block {block}");
        }
        assert!(heard);
        let (bundle, _) = super::super::session::session_bundle(e.session()).unwrap();
        let mut bundled = Engine::default();
        assert!(super::super::session::open_session_bytes(&mut bundled, "pad", &bundle).unwrap().is_empty());
        let original = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
        let unpacked = bundled.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
        assert_eq!(original.data, unpacked.data);
        // Saving a copy brings both files along, even to a new media folder.
        let copy = Folder::new();
        super::super::session::save_session_copy(&e, copy.file().to_str().unwrap()).unwrap();
        assert!(super::tests::opened(&copy).1.is_empty());
    }
}

#[test]
fn missing_and_wrong_quality_media_open_silent_and_name_track_and_insert() {
    for missing in [true, false] {
        let folder = Folder::new();
        let (mut e, _) = fixture(0, true);
        super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
        let ins = e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap();
        if missing {
            std::fs::remove_file(folder.0.join(&ins.frozen_recording.as_ref().unwrap().path)).unwrap();
        } else {
            ins.frozen_recording.as_mut().unwrap().quality = 3;
            std::fs::write(folder.file(), e.session().to_json().unwrap()).unwrap();
        }
        let (loaded, notes) = opened(&folder);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("Pad") && notes[0].contains("Texture insert 1"), "{notes:?}");
        if !missing {
            assert!(notes[0].contains("mismatch"));
        }
        let mut p = device(&loaded);
        let mut io = vec![vec![0.0; 250]; 2];
        for _ in 0..20 {
            p.process(&mut io, 250);
        }
        assert!(io.iter().flatten().all(|x| *x == 0.0));
    }
}

#[test]
fn freeze_off_clears_the_reference_keeps_old_media_and_round_trips_as_before() {
    let folder = Folder::new();
    let (mut e, _) = fixture(3, true);
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    let ins = e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap();
    let old = ins.frozen_recording.as_ref().unwrap().path.clone();
    ins.params.insert("freeze".into(), 0.0);
    let track = e.session().tracks[0].id.0;
    e.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": null})).unwrap();
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    assert!(e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.is_none());
    assert!(folder.0.join(old).is_file(), "immutable media remains for backups");
    let (loaded, notes) = opened(&folder);
    assert!(notes.is_empty());
    let mut a = device(&e);
    let mut b = device(&loaded);
    for _ in 0..20 {
        let mut x = vec![vec![0.25; 250]; 2];
        let mut y = x.clone();
        a.process(&mut x, 250);
        b.process(&mut y, 250);
        assert_eq!(x, y);
    }
}

#[test]
fn importing_a_frozen_track_at_another_host_rate_reports_the_mismatch() {
    let folder = Folder::new();
    let (mut donor, _) = fixture(3, true);
    super::super::session::save_session(&mut donor, folder.file().to_str().unwrap()).unwrap();
    let mut receiver = Engine::default();
    receiver.session_mut().sample_rate = transit_time::SampleRate::new(44_100).unwrap();
    let result = super::super::session_data::import_session_data(&mut receiver, folder.file().to_str().unwrap(), None).unwrap();
    assert_eq!(result.missing.len(), 1);
    assert!(result.missing[0].contains("Texture insert 1") && result.missing[0].contains("mismatch"));
    let ins = receiver.session().track(result.tracks[0]).unwrap().mixer.inserts[0].as_ref().unwrap();
    assert!(ins.frozen_recording.as_ref().unwrap().data.is_none());
}

#[test]
fn captured_setup_and_live_freeze_win_over_stored_base_values() {
    let folder = Folder::new();
    let (mut e, _) = fixture(2, true);
    let ins = e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap();
    ins.params.insert("quality".into(), 0.0);
    ins.params.insert("buffer".into(), 4.0);
    ins.params.insert("mono_input".into(), 1.0);
    ins.params.insert("freeze".into(), 0.0);
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    let (loaded, notes) = opened(&folder);
    assert!(notes.is_empty(), "{notes:?}");
    let saved = loaded.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
    assert_eq!((saved.quality, saved.buffer_frames, saved.channels), (2, 192_000, 2));
    assert!(saved.data.is_some());
    // Live Freeze off clears the captured reference even if base Freeze remains on.
    let track = e.session().tracks[0].id.0;
    e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap().params.insert("freeze".into(), 1.0);
    e.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": null})).unwrap();
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    assert!(opened(&folder).0.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.is_none());
}

#[test]
fn wav_alone_restores_the_recording_and_reopens_deterministically() {
    for quality in 0..4 {
        for corrupt in [false, true] {
            let folder = Folder::new();
            let (mut e, _) = fixture(quality, true);
            super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
            let saved = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
            let original_pcm = Recording::parse(saved.data.as_ref().unwrap()).unwrap().pcm.to_vec();
            let companion = folder.0.join(&saved.continuation);
            if corrupt {
                std::fs::write(companion, b"future continuation version").unwrap();
            } else {
                std::fs::remove_file(companion).unwrap();
            }
            let (a, notes) = opened(&folder);
            assert_eq!(notes.len(), 1);
            assert!(notes[0].contains("WAV restored") && notes[0].contains("continuation not restored"), "{notes:?}");
            let data = a.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap().data.as_ref().unwrap();
            let r = Recording::parse(data).unwrap();
            assert_eq!(r.pcm, original_pcm);
            assert!(r.continuation.is_empty());
            let (b, _) = opened(&folder);
            let (mut a, mut b) = (device(&a), device(&b));
            let mut heard = false;
            for _ in 0..64 {
                let mut x = vec![vec![0.0; 250]; 2];
                let mut y = x.clone();
                a.process(&mut x, 250);
                b.process(&mut y, 250);
                heard |= x.iter().flatten().any(|v| *v != 0.0);
                assert_eq!(x, y);
            }
            assert!(heard, "quality {quality}");
        }
    }
}

#[test]
fn failed_restore_retains_missing_reference_through_capture_save_and_copy() {
    let folder = Folder::new();
    let (mut e, _) = fixture(0, true);
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    let saved = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
    let missing = folder.0.join(&saved.path);
    std::fs::remove_file(&missing).unwrap();
    let (mut loaded, _) = opened(&folder);
    let mut silent = device(&loaded);
    let state = silent.save_state().map(|data| transit_model::b64::encode(&data));
    let track = loaded.session().tracks[0].id.0;
    loaded.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": state})).unwrap();
    super::super::session::save_session(&mut loaded, folder.file().to_str().unwrap()).unwrap();
    assert!(!missing.exists(), "saving must never replace missing audio with silence");
    let copy = Folder::new();
    super::super::session::save_session_copy(&loaded, copy.file().to_str().unwrap()).unwrap();
    for f in [&folder, &copy] {
        let (again, notes) = opened(f);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("opens silent"));
        assert!(again.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap().restore_failed);
    }
    // Recording after unfreezing permits replacement even without saving while Freeze is off.
    silent.set_param("freeze", 0.0);
    let mut audio = vec![vec![0.25; 250]; 2];
    silent.process(&mut audio, 250);
    silent.set_param("freeze", 1.0);
    silent.process(&mut audio, 250);
    let new = silent.save_state().unwrap();
    loaded.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": transit_model::b64::encode(&new)})).unwrap();
    assert!(!loaded.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap().restore_failed);
    // A live Freeze-off capture also ends the missing recording.
    loaded.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": null})).unwrap();
    assert!(loaded.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.is_none());
}

#[test]
fn legacy_wav_without_a_companion_uses_its_saved_setup_metadata() {
    let folder = Folder::new();
    let (mut e, _) = fixture(1, true);
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    let saved = e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap().frozen_recording.as_mut().unwrap();
    std::fs::remove_file(folder.0.join(&saved.continuation)).unwrap();
    saved.header.clear();
    std::fs::write(folder.file(), e.session().to_json().unwrap()).unwrap();
    let (loaded, notes) = opened(&folder);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("WAV restored"));
    assert!(loaded.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap().data.is_some());
}

#[test]
fn frozen_media_write_failure_does_not_refuse_the_session() {
    let (mut e, _) = fixture(0, true);
    store(e.session_mut(), &mut |_, _, _| Err(EngineError::Io("disk refused media".into()))).unwrap();
    let saved = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
    assert!(saved.report.contains("disk refused media"));
    assert!(e.session().to_json().unwrap().contains("disk refused media"));
    // Invalid runtime packets also cannot refuse unrelated session state.
    e.session_mut().tracks[0].mixer.inserts[0].as_mut().unwrap().frozen_recording.as_mut().unwrap().data = Some(vec![0; 20].into());
    store(e.session_mut(), &mut |_, _, _| panic!("invalid capture must not be written")).unwrap();
}

#[test]
fn failed_media_write_retains_the_previous_file_and_its_setup() {
    let folder = Folder::new();
    let (mut e, _) = fixture(0, true);
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    let before = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.clone().unwrap();
    let (_, mut newer) = fixture(2, true);
    let packet = newer.save_state().unwrap();
    let track = e.session().tracks[0].id.0;
    e.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": transit_model::b64::encode(&packet)})).unwrap();
    store(e.session_mut(), &mut |_, _, _| Err(EngineError::Io("disk refused media".into()))).unwrap();
    let saved = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
    assert_eq!(
        (&saved.path, &saved.continuation, &saved.header, saved.quality),
        (&before.path, &before.continuation, &before.header, before.quality)
    );
    std::fs::write(folder.file(), e.session().to_json().unwrap()).unwrap();
    let (loaded, notes) = opened(&folder);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(loaded.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap().data, before.data);
}

#[test]
fn invalid_capture_keeps_previous_frozen_media_and_does_not_abort_save() {
    let folder = Folder::new();
    let (mut e, _) = fixture(0, true);
    super::super::session::save_session(&mut e, folder.file().to_str().unwrap()).unwrap();
    let before = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.clone().unwrap();
    let track = e.session().tracks[0].id.0;
    let response =
        e.execute("mix.insert_state", &json!({"track": track, "slot": 0, "state": transit_model::b64::encode(b"invalid frozen packet")})).unwrap();
    assert!(response["report"].as_str().unwrap().contains("invalid frozen capture"));
    let retained = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap();
    assert_eq!((&retained.path, &retained.header, &retained.data), (&before.path, &before.header, &before.data));
    assert!(retained.report.contains("invalid frozen capture"));
    e.execute("session.save", &json!({"path": folder.file().to_string_lossy()})).unwrap();
    let (loaded, notes) = opened(&folder);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(loaded.session().tracks[0].mixer.inserts[0].as_ref().unwrap().frozen_recording.as_ref().unwrap().data, before.data);
}
