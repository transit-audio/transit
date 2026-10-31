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
            ins.params.insert("quality".into(), 3.0);
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
