//! The Sampler patch commands, load and save (ADR 0041, job C2). Files are generated tones in a
//! scratch folder per test; renders go through the offline mix (the plan compiler's path). No
//! sleeps: the import is prepared on the test's own thread.

use crate::Engine;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_dsp::Plugin;
use transit_dsp::plugins::sampler as dsp;
use transit_midi::{Note, Sequence};
use transit_model::sampler_patch::{AutoSource, Detector, Division, Marker, Markers, Ownership, PatchV1, PlayMode, SliceLoop, SliceMode};
use transit_model::{Clip, SourceId, TrackId};
use transit_time::{Range, TICKS_PER_QUARTER};

struct Dir(transit_test_support::TestDir);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = transit_test_support::TestDir::new(&format!("sampler-patch-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

/// A generated tone (no recorded audio) as a 16-bit WAV of `secs` seconds.
fn tone(path: &Path, freq: f32, rate: u32, secs: f32, channels: usize) {
    let n = (rate as f32 * secs) as usize;
    let one: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect();
    let buf = AudioBuffer { sample_rate: rate, channels: vec![one; channels] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None, bitrate_kbps: None, title: None };
    std::fs::write(path, transit_audio_io::encode(&buf, &opts).unwrap()).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn instrument(e: &mut Engine) -> TrackId {
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    e.session().tracks.last().unwrap().id
}

fn notes(e: &mut Engine, t: TrackId, list: &[(u8, i64, i64)]) {
    let q = TICKS_PER_QUARTER;
    let mut seq = Sequence::default();
    for &(pitch, start, len) in list {
        seq.notes.push(Note { pitch, velocity: 127, release_velocity: 64, channel: 0, start: start * q, length: len * q });
    }
    let s = e.session_mut();
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 8, seq));
}

fn render(e: &Engine, frames: i64) -> Vec<Vec<f32>> {
    transit_mix::render_range(e.session(), Range::new(0, frames), 512)
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

fn same_bits(a: &[Vec<f32>], b: &[Vec<f32>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()))
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// A two-zone patch from a file: zone 1 on pad 1 (note 60 at the stated pad start note 60,
/// one-shot), zone 2 unbound over keys 70..80 (Gate), with controls on both, and two notes to play
/// them.
fn kit(e: &mut Engine, d: &Dir) -> TrackId {
    let wav = d.path("tone.wav");
    tone(&wav, 220.0, 48_000, 0.5, 2);
    let t = instrument(e);
    e.execute("mix.sampler_new_patch", &json!({"track": t, "seed": 9})).unwrap();
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": s(&wav)})).unwrap();
    let src = patch(e, t).sources[0].source.0;
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "source": src, "playback": {"keys": [70, 80], "root": 72}})).unwrap();
    for (id, v) in [("zone001_level", -6.0), ("zone001_pan", 30.0), ("zone002_shaper", 1.0), ("zone002_cutoff", 900.0), ("zone002_resonance", 40.0)] {
        e.execute("mix.sampler_control", &json!({"track": t, "id": id, "value": v})).unwrap();
    }
    notes(e, t, &[(60, 0, 1), (72, 1, 1)]);
    t
}

#[test]
fn a_new_patch_leaves_the_one_sample_field_unset() {
    let mut e = Engine::default();
    let t = instrument(&mut e);
    // A new patch on another instrument's track: a Sampler with an empty kit, no one-sample field
    // and no parameter values (the 19 legacy ids are retired, #455); undo brings the instrument back.
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
    assert!(ins.plugin == "sampler" && ins.sample.is_none() && ins.sampler_patch.is_some() && ins.params.is_empty(), "{ins:?}");
    assert!(e.execute("mix.sampler_new_patch", &json!({"track": t})).is_err(), "one patch per Sampler");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session().track(t).unwrap().instrument.as_ref().unwrap().plugin, "macro_oscillator");
}

#[test]
fn the_commands_edit_the_patch_atomically_and_refuse_bad_edits_whole() {
    let d = Dir::new("edit");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    let p = patch(&e, t);
    assert_eq!(p.seed, 9);
    assert_eq!(p.zones.len(), 2);
    assert_eq!(p.pad(0).zone, Some(0));
    assert_eq!((p.zone(0).unwrap().mode, p.zone(0).unwrap().key_tracking), (PlayMode::OneShot, false), "pad defaults");
    assert_eq!(p.sources.len(), 1, "both zones share one source");
    assert_eq!(p.sources[0].ownership, Ownership::Referenced, "a file is referenced by default");
    assert_eq!(p.controls.len(), 5);
    let before = e.session_arc();
    for r in [json!({"track": t, "pad": 2, "note": 60}), json!({"track": t, "pad": 2, "choke": 17}), json!({"track": t, "pad": 65})] {
        assert!(e.execute("mix.sampler_pad_set", &r).is_err(), "{r}");
    }
    assert!(e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "region": {"end": 999_999_999}})).is_err(), "past the source");
    assert!(e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"attack_ms": 9e9}})).is_err());
    assert!(e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"bogus": 1}})).is_err());
    assert!(e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 3})).is_err(), "an empty zone");
    assert!(e.execute("mix.sampler_pad_bind", &json!({"track": t, "pad": 2, "zone": 1})).is_err(), "zone 1 is pad 1's");
    assert!(e.execute("mix.sampler_control", &json!({"track": t, "id": "zone001_nope", "value": 1})).is_err());
    assert!(e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 3, "source": 424_242})).is_err());
    assert!(Arc::ptr_eq(&before, &e.session_arc()), "refused edits change nothing");
    // Valid edits: a pad's note and choke, a binding, a clear (the pad unbinds, the file stays).
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 2, "note": 40, "choke": 3})).unwrap();
    e.execute("mix.sampler_pad_bind", &json!({"track": t, "pad": 2, "zone": 2})).unwrap();
    assert_eq!(patch(&e, t).pad(1).zone, Some(1));
    e.execute("mix.sampler_control", &json!({"track": t, "id": "zone001_level", "value": 99})).unwrap();
    assert_eq!(patch(&e, t).controls.get("zone001_level"), Some(&12.0), "clamped");
    e.execute("mix.sampler_control", &json!({"track": t, "id": "zone001_level", "value": 0})).unwrap();
    assert!(!patch(&e, t).controls.contains_key("zone001_level"), "a default is not stored");
    e.execute("mix.sampler_zone_clear", &json!({"track": t, "zone": 2})).unwrap();
    assert_eq!(patch(&e, t).pad(1).zone, None);
    assert!(d.path("tone.wav").exists(), "clearing never deletes a file");
    e.execute("mix.sampler_seed", &json!({"track": t, "seed": 5})).unwrap();
    assert_eq!(patch(&e, t).seed, 5);
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(patch(&e, t).zones.len(), 2, "each edit is one undo step");
}

/// Plays `spec` directly through a Sampler: one publication, then `set_param` for each control,
/// with the notes of [`kit`] at their frames, in 512-frame blocks with the transport playing.
fn direct(spec: dsp::PatchSpec, seed: u32, controls: &std::collections::BTreeMap<String, f32>, frames: usize) -> Vec<Vec<f32>> {
    use transit_dsp::event::{Message, NoteId, PluginEvent};
    let mut s = dsp::Sampler::new();
    s.prepare(48_000.0, 512, 2);
    let mut publisher = s.enable_sampler_core(seed).unwrap();
    publisher.publish(spec).unwrap();
    for (id, v) in controls {
        assert!(s.set_param(id, *v), "{id}");
    }
    let mut out = vec![Vec::new(), Vec::new()];
    let mut start = 0;
    while start < frames {
        let n = 512.min(frames - start);
        for (frame, key, on) in [(0usize, 60u8, true), (24_000, 60, false), (24_000, 72, true), (48_000, 72, false)] {
            if (start..start + n).contains(&frame) {
                let message = if on {
                    Message::NoteOn { key, velocity: 127, id: NoteId::NONE }
                } else {
                    Message::NoteOff { key, velocity: 64, id: NoteId::NONE }
                };
                s.event((frame - start) as u32, &PluginEvent { channel: 0, note_id: -1, message });
            }
        }
        let t = transit_dsp::Transport {
            playing: true,
            tempo: 120.0,
            numerator: 4,
            denominator: 4,
            samples: start as i64,
            quarters: start as f64 / 24_000.0,
            ..transit_dsp::Transport::default()
        };
        s.set_transport(&t);
        let mut io = vec![vec![0.0f32; n]; 2];
        s.process(&mut io, n);
        for (o, c) in out.iter_mut().zip(io) {
            o.extend(c);
        }
        start += n;
    }
    out
}

#[test]
fn a_load_restores_structure_and_controls_bit_for_bit_with_a_direct_publication() {
    let d = Dir::new("load");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    let path = d.path("song.transit");
    e.execute("session.save_as", &json!({"path": s(&path)})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": s(&path)})).unwrap();
    let p = patch(&back, t);
    assert_eq!(p, patch(&e, t), "the patch comes back as saved");
    let loaded = render(&back, 96_000);
    assert!(peak(&loaded[0][..24_000]) > 0.05 && peak(&loaded[0][24_000..48_000]) > 0.01, "both zones sound");
    assert!(same_bits(&loaded, &render(&e, 96_000)), "a reopened session plays as the one saved");
    // The same structure published directly, then the controls through set_param.
    let audio = back.session().pool.get(p.sources[0].source).unwrap().clone();
    let sample: Arc<dyn transit_dsp::SampleSource> =
        Arc::new(transit_dsp::SampleBuffer { sample_rate: 48_000.0, channels: audio.buffer.channels.clone() });
    let frames = audio.frames() as u64;
    let mut z1 = dsp::Zone::new(dsp::Region::whole(0, frames));
    z1.playback = dsp::Playback { mode: dsp::PlayMode::OneShot, key_tracking: false, root: 60, ..dsp::Playback::zone() };
    let mut z2 = dsp::Zone::new(dsp::Region::whole(0, frames));
    z2.keys = (70, 80);
    z2.playback.root = 72;
    let mut spec = dsp::PatchSpec { sources: vec![sample], zones: vec![Some(z1), Some(z2)], ..dsp::PatchSpec::default() };
    spec.pads[0].zone = Some(0);
    let reference = direct(spec, 9, &p.controls, 96_000);
    assert!(same_bits(&loaded, &reference), "the loaded instance plays as a direct publication plus set_param calls");
}

#[test]
fn admission_refuses_each_bound_and_the_old_patch_keeps_playing() {
    let d = Dir::new("admit");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    let before_render = render(&e, 48_000);
    let sources = e.session().sources.len();
    // 30 s: a 31 s file (at 8 kHz, refused from its header before it is decoded).
    let long = d.path("long.wav");
    tone(&long, 100.0, 8_000, 31.0, 1);
    let err = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 5, "path": s(&long)})).unwrap_err().to_string();
    assert!(err.contains("30 s"), "{err}");
    // Mono or stereo only.
    let wide = d.path("wide.wav");
    tone(&wide, 100.0, 48_000, 0.1, 3);
    let err = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 5, "path": s(&wide)})).unwrap_err().to_string();
    assert!(err.contains("mono or stereo"), "{err}");
    // The session's streaming threshold (here 6 s) is the minimum with the 30 s bound.
    let mid = d.path("mid.wav");
    tone(&mid, 100.0, 8_000, 7.0, 1);
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.1);
    let before = e.session_arc();
    let err = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 5, "path": s(&mid)})).unwrap_err().to_string();
    assert!(err.contains("streaming threshold"), "{err}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()), "the refused edit changed nothing");
    e.session_mut().edit.values.remove(transit_model::disk::STREAM_MINUTES_KEY);
    assert_eq!(e.session().sources.len(), sources, "nothing was added");
    assert!(same_bits(&render(&e, 48_000), &before_render), "the old patch keeps playing");
}

#[test]
fn the_per_source_bounds_hold_at_their_edges() {
    let s = transit_model::Session::default();
    let at = |frames: u64, ch: u64, rate: u32| super::admit_source(&s, "x", frames, ch, rate);
    assert!(at(30 * 48_000, 1, 48_000).is_ok(), "exactly 30 s");
    assert!(at(30 * 48_000 + 1, 1, 48_000).unwrap_err().contains("30 s"));
    // At 192 kHz stereo, 32 MiB is 4,194,304 frames (21.8 s): the byte bound binds first.
    assert!(at(4_194_304, 2, 192_000).is_ok());
    assert!(at(4_194_305, 2, 192_000).unwrap_err().contains("32 MiB"));
    assert!(at(10, 3, 48_000).unwrap_err().contains("mono or stereo"));
    assert!(at(u64::MAX, 2, 48_000).is_err(), "checked arithmetic, no overflow");
}

#[test]
fn the_patch_bound_counts_distinct_audio_once() {
    let mut e = Engine::default();
    let t = instrument(&mut e);
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    // Twelve sources of 29.9 s stereo at 48 kHz (10.95 MiB each): eleven fit in 128 MiB, the twelfth
    // does not.
    let frames = 1_435_200usize;
    let mut ids = Vec::new();
    for i in 0..12 {
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; frames]; 2] };
        ids.push(crate::io::add_source(e.session_mut(), &format!("s{i}"), buf, None, FileFormat::Wav));
    }
    // One key each: at most eight zones may sound on one note (ADR 0041).
    for (z, sid) in ids.iter().take(11).enumerate() {
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": z + 1, "source": sid.0, "playback": {"keys": [z, z]}})).unwrap();
    }
    let before = e.session_arc();
    let err = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 12, "source": ids[11].0})).unwrap_err().to_string();
    assert!(err.contains("128 MiB"), "{err}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    // A sample already in the patch adds nothing: another zone on it is admitted.
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 12, "source": ids[0].0, "playback": {"keys": [11, 11]}})).unwrap();
}

#[test]
fn a_zone_source_comes_through_the_import_transaction_and_dropped_bytes_are_managed() {
    let d = Dir::new("import");
    let wav = d.path("hit.wav");
    tone(&wav, 330.0, 44_100, 0.2, 1);
    let mut e = Engine::default();
    let t = instrument(&mut e);
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    let job = e.imports.prepare_here_for_testing(&s(&wav), crate::io::ImportRate::Native);
    let r = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "import": job, "copy": true})).unwrap();
    assert_eq!(r["frames"], 8_820, "native frames retained by the import (ADR 0041)");
    assert_eq!(e.session().sources.last().unwrap().sample_rate, 44_100);
    assert_eq!(patch(&e, t).sources[0].ownership, Ownership::Managed, "copy: the session's own audio");
    assert!(e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "import": job})).is_err(), "an import is taken once");
    // Dropped bytes have no path: always managed, written into Audio Files on save.
    let bytes = std::fs::read(&wav).unwrap();
    let steps = e.undo_history().len();
    super::load_zone_bytes(&mut e, t, 1, "drop.wav", &bytes).unwrap();
    assert_eq!(e.undo_history().len(), steps + 1);
    let p = patch(&e, t);
    let sid = p.sources[usize::from(p.zone(1).unwrap().source)].source;
    let src = e.session().source(sid).unwrap().clone();
    assert!(src.unsaved && src.path.starts_with("Audio Files/"), "{src:?}");
    let count = e.session().sources.len();
    assert!(super::load_zone_bytes(&mut e, t, 300, "drop.wav", &bytes).is_err());
    assert_eq!(e.session().sources.len(), count, "a refused drop adds nothing");
}

#[test]
fn save_save_as_and_autosave_carry_the_patch_slicing_and_media() {
    let d = Dir::new("save");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    // A managed source too, both slice sets with the mode and every loop setting off its
    // default, as Detect and Replace markers will write them.
    let bytes = std::fs::read(d.path("tone.wav")).unwrap();
    super::load_zone_bytes(&mut e, t, 2, "managed.wav", &bytes).unwrap();
    {
        let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
        let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        let set = |ids: &[(u32, u64)]| Markers {
            markers: ids.iter().map(|&(id, frame)| Marker { id, frame, ..Marker::default() }).collect(),
            ..Markers::default()
        };
        p.slicing = Some(SliceLoop {
            source: 0,
            end: 20_000,
            mode: SliceMode::Manual,
            snap: false,
            detector: Detector { source: AutoSource::Grid, sensitivity: 0.25, ..Detector::default() },
            division: Division::Eighth,
            bars: Some(4),
            grid_bpm: Some(92.5),
            root: Some(36),
            beat_length: Some(8.0),
            auto: Some(set(&[(1, 0), (2, 10_000)])),
            manual: Some(set(&[(7, 0), (8, 5_000), (9, 15_000)])),
            ..SliceLoop::default()
        });
    }
    let want = patch(&e, t);
    let referenced = s(&d.path("tone.wav"));
    let (first, second, auto) = (d.path("a/song.transit"), d.path("b/song.transit"), d.path("auto/song.transit"));
    e.execute("session.save_as", &json!({"path": s(&first)})).unwrap();
    e.execute("session.save_as", &json!({"path": s(&second)})).unwrap();
    crate::io::save_copy(&e, &s(&auto)).unwrap();
    for (file, dir) in [(&first, "a"), (&second, "b"), (&auto, "auto")] {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(v["needs"], json!(["sampler_patch", "sampler_slicing_modes"]), "{dir}");
        let mut back = Engine::default();
        back.execute("session.open", &json!({"path": s(file)})).unwrap();
        let p = patch(&back, t);
        assert_eq!((&p.slicing, &p.zones, &p.pads, &p.controls), (&want.slicing, &want.zones, &want.pads, &want.controls), "{dir}");
        // The referenced file keeps its own path; the managed one is a file in this folder.
        let paths: Vec<String> = p.sources.iter().map(|r| back.session().source(r.source).unwrap().path.clone()).collect();
        assert!(paths.contains(&referenced), "{dir}: {paths:?}");
        let managed = paths.iter().find(|x| x.starts_with("Audio Files/")).unwrap_or_else(|| panic!("{dir}: {paths:?}"));
        assert!(d.path(dir).join(managed).exists(), "{dir}");
        assert!(p.sources.iter().all(|r| back.session().pool.get(r.source).is_some()), "{dir}: every dependency decoded");
    }
}

#[test]
fn missing_media_keeps_its_reference_and_reports_silence_per_zone() {
    let d = Dir::new("missing");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    let path = d.path("song.transit");
    e.execute("session.save_as", &json!({"path": s(&path)})).unwrap();
    std::fs::remove_file(d.path("tone.wav")).unwrap();
    let mut back = Engine::default();
    let r = back.execute("session.open", &json!({"path": s(&path)})).unwrap();
    assert_eq!(r["missing"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(back.messages.iter().any(|m| m.contains("Sampler zones 1, 2 play silence")), "{:?}", back.messages);
    assert_eq!(patch(&back, t), patch(&e, t), "the references stay");
    assert_eq!(peak(&render(&back, 48_000)[0]), 0.0, "silence, no error");
    back.execute("session.save", &json!({})).unwrap();
    let mut again = Engine::default();
    again.execute("session.open", &json!({"path": s(&path)})).unwrap();
    assert_eq!(patch(&again, t).sources, patch(&e, t).sources, "saved again, the reference is still there");
}

#[test]
fn importing_a_track_brings_its_patch_media_and_never_aliases_a_source() {
    let d = Dir::new("donor");
    let mut donor = Engine::default();
    let t = kit(&mut donor, &d);
    let file = d.path("donor.transit");
    donor.execute("session.save_as", &json!({"path": s(&file)})).unwrap();
    let mut e = Engine::default();
    // Sources here take ids the donor's patch also uses.
    for i in 0..4 {
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 100]] };
        crate::io::add_source(e.session_mut(), &format!("x{i}"), buf, None, FileFormat::Wav);
    }
    let name = donor.session().track(t).unwrap().name.clone();
    e.execute("file.import_session_data", &json!({"path": s(&file), "tracks": [name.clone()]})).unwrap();
    let nt = e.session().track_by_name(&name).unwrap().id;
    let sid: SourceId = patch(&e, nt).sources[0].source;
    assert_eq!(e.session().source(sid).unwrap().name, "tone", "the patch names the imported copy, not a source of this session");
    assert!(e.session().pool.get(sid).is_some());
}

#[test]
fn a_file_over_a_bound_is_refused_from_its_header_without_reading_its_body() {
    let d = Dir::new("header");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    let long = d.path("long.wav");
    tone(&long, 100.0, 8_000, 31.0, 1);
    let reads = crate::io::WHOLE_READS.with(std::cell::Cell::get);
    let err = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 5, "path": s(&long)})).unwrap_err().to_string();
    assert!(err.contains("30 s"), "{err}");
    assert_eq!(crate::io::WHOLE_READS.with(std::cell::Cell::get), reads, "refused before the body was read");
    // A file within the bounds is read whole (the counter sees it).
    let short = d.path("short.wav");
    tone(&short, 100.0, 8_000, 0.5, 1);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 5, "path": s(&short)})).unwrap();
    assert_eq!(crate::io::WHOLE_READS.with(std::cell::Cell::get), reads + 1);
}

#[test]
fn a_pad_or_zone_holding_only_unknown_fields_survives_load_edit_and_save() {
    let d = Dir::new("unknown");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    {
        let p = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        let mut pad = transit_model::sampler_patch::PadEntry { index: 3, ..Default::default() };
        pad.extra.insert("color".into(), json!("red"));
        p.pads.push(pad);
        p.zones[0].extra.insert("lfo3".into(), json!({"rate": 2}));
    }
    let path = d.path("song.transit");
    e.execute("session.save_as", &json!({"path": s(&path)})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": s(&path)})).unwrap();
    // Edits that store that pad and that zone again.
    back.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 4, "mute": false})).unwrap();
    back.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"transpose": 1}})).unwrap();
    back.execute("session.save", &json!({})).unwrap();
    let mut again = Engine::default();
    again.execute("session.open", &json!({"path": s(&path)})).unwrap();
    let p = patch(&again, t);
    assert_eq!(p.pad(3).extra.get("color"), Some(&json!("red")), "the pad with only an unknown field is kept: {:?}", p.pads);
    assert_eq!(p.zone(0).unwrap().extra.get("lfo3"), Some(&json!({"rate": 2})));
    assert_eq!(p.zone(0).unwrap().transpose, 1.0);
}

#[path = "tests_sampler_native.rs"]
mod native;

/// #489: a patch this build cannot read is kept exactly as saved, and a save never rewrites the
/// path of a source it marks referenced: no copy in Audio Files, the session still points at the
/// file where it is.
#[test]
fn a_save_keeps_the_referenced_path_of_an_opaque_patch() {
    let d = Dir::new("opaque");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    let wav = s(&d.path("tone.wav"));
    let mut raw = serde_json::to_value(patch(&e, t)).unwrap();
    raw["schema"] = json!(2);
    assert_eq!(raw["sources"][0]["ownership"], "referenced", "{raw}");
    let opaque = transit_model::SamplerPatch::from_value(raw.clone());
    assert!(opaque.ready().is_none(), "opaque");
    e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch = Some(opaque);
    let sid = SourceId(raw["sources"][0]["source"].as_u64().unwrap());
    let song = d.path("song/song.transit");
    e.execute("session.save_as", &json!({"path": s(&song)})).unwrap();
    assert_eq!(e.session().source(sid).unwrap().path, wav, "the session still points at the file where it is");
    assert!(!d.path("song/Audio Files").exists(), "nothing copied");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&song).unwrap()).unwrap();
    let saved = v["tracks"].as_array().unwrap().iter().find(|x| x["id"] == json!(t.0)).unwrap();
    assert_eq!(saved["instrument"]["sampler_patch"], raw, "kept exactly as saved");
}

/// #493: a template keeps the files its Sampler patches name (by path) and drops every freeze, so
/// a session opened from it plays its kits and claims no freeze it does not have.
#[test]
fn a_template_keeps_its_sampler_samples_and_drops_freezes() {
    let d = Dir::new("template");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    // Dropped bytes (managed): once the session is saved they are a file in its Audio Files, which
    // the template names by its full path.
    let song = d.path("song/song.transit");
    let own = d.path("own.wav");
    tone(&own, 330.0, 48_000, 0.25, 1);
    super::load_zone_bytes(&mut e, t, 2, "own.wav", &std::fs::read(&own).unwrap()).unwrap();
    std::fs::remove_file(&own).unwrap();
    e.execute("session.save_as", &json!({"path": s(&song)})).unwrap();
    let frozen = patch(&e, t).sources[0].source;
    e.session_mut().track_mut(t).unwrap().freeze =
        Some(transit_model::Frozen { source: frozen, start: 0, length: 48, hash: 0, revision: 0, latency: 0 });
    let template = d.path("templates/Drums.transit");
    std::fs::create_dir_all(d.path("templates")).unwrap();
    e.execute("session.save_template", &json!({"path": s(&template)})).unwrap();
    let mut back = Engine::default();
    let r = back.execute("session.open", &json!({"path": s(&template)})).unwrap();
    assert_eq!(r["missing"].as_array().map(Vec::len), Some(0), "{r}");
    assert!(back.session().track(t).unwrap().freeze.is_none(), "no freeze claimed");
    assert!(back.session().tracks.iter().all(|x| x.playlists.iter().all(|p| p.clips.is_empty())), "no clips");
    let p = patch(&back, t);
    assert_eq!(p.sources.len(), 2);
    for r in &p.sources {
        let src = back.session().source(r.source).unwrap_or_else(|| panic!("{:?} kept", r.source));
        assert!(Path::new(&src.path).is_absolute(), "by its full path: {}", src.path);
        assert!(back.session().pool.get(r.source).is_some(), "{} decoded", src.path);
    }
    notes(&mut back, t, &[(60, 0, 1)]);
    assert!(peak(&render(&back, 48_000)[0]) > 0.1, "the kit plays");
}

#[test]
fn a_template_is_refused_while_a_kit_sample_has_no_file() {
    let d = Dir::new("template-unsaved");
    let mut e = Engine::default();
    let t = kit(&mut e, &d);
    super::load_zone_bytes(&mut e, t, 2, "drop.wav", &std::fs::read(d.path("tone.wav")).unwrap()).unwrap();
    let template = d.path("Drums.transit");
    let err = e.execute("session.save_template", &json!({"path": s(&template)})).unwrap_err().to_string();
    assert!(err.contains("has no file yet"), "{err}");
    assert!(!template.exists(), "nothing written");
}

/// #491: a refused zone or slice-loop command leaves its prepared import staged, so the same call
/// with corrected values succeeds; a command that succeeds takes it.
#[test]
fn a_refused_command_leaves_the_prepared_import_for_the_corrected_call() {
    let d = Dir::new("import-refused");
    let wav = d.path("loop.wav");
    tone(&wav, 330.0, 48_000, 0.1, 1);
    let mut e = Engine::default();
    let t = instrument(&mut e);
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    let job = e.imports.prepare_here_for_testing(&s(&wav), crate::io::ImportRate::Native);
    // A slice loop with an end past the file, then the same call with a valid end.
    assert!(e.execute("mix.sampler_slice_loop", &json!({"track": t, "import": job, "end": 1_000_000})).is_err());
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "import": job, "end": 2_400})).unwrap();
    assert!(e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 9, "import": job})).is_err(), "taken by the success");
    // A zone the group check refuses once its source is read (a second key range in group 0),
    // then the same call on the group's key.
    let member = |keys: [u8; 2], alternative: u8| json!({"keys": keys, "membership": {"group": 0, "layer": 0, "alternative": alternative}});
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 3, "path": s(&wav), "playback": member([60, 60], 0)})).unwrap();
    let job = e.imports.prepare_here_for_testing(&s(&wav), crate::io::ImportRate::Native);
    let refused = e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "import": job, "playback": member([61, 61], 1)}));
    assert!(refused.is_err(), "{refused:?}");
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "import": job, "playback": member([60, 60], 1)})).unwrap();
    assert!(e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "import": job})).is_err(), "taken by the success");
}
