//! Auto mode's detection commands (ADR 0041 and its amendment 2026-11-08, items 5 and 7, #269).
//! Fixtures are built here: generated audio added to the session, and WAV files written with the
//! crate's encoder plus `cue `, `LIST`/`adtl`/`labl` and `acid` chunks written byte by byte. Jobs
//! run on the calling thread (or are held until the test runs them), so nothing waits on time.

use super::*;
use crate::slice_detect::{Run, Stop};
use crate::{Engine, EngineError};
use serde_json::{Value, json};
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_model::sampler_patch::{Marker, Markers, PadEntry, SliceBinding, SliceMode, ZoneEntry};
use transit_model::{SourceId, TrackId};

const SR: u32 = 48_000;

/// A scratch folder per test.
struct Dir(std::path::PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("sc-slice-detect-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An instrument track with an empty Sampler patch.
fn engine() -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    (e, t)
}

/// Adds `channels` at `rate` as a source (resampled to the session's 48 kHz) and makes the loop
/// over all of it. With `file`, the source is that WAV file on disk (cue points and tempo are read
/// from it); without, it is audio only in memory.
fn with_loop(e: &mut Engine, t: TrackId, channels: Vec<Vec<f32>>, rate: u32, file: Option<&str>) -> SourceId {
    let sid = crate::io::add_source(e.session_mut(), "loop", AudioBuffer { sample_rate: rate, channels }, file, FileFormat::Wav);
    if file.is_some() {
        e.session_mut().sources.iter_mut().find(|s| s.id == sid).unwrap().unsaved = false;
    }
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    sid
}

/// One second of quiet noise-free audio (a 48,000-frame loop with no transients).
fn silence(frames: usize) -> Vec<Vec<f32>> {
    vec![vec![0.0; frames]; 2]
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    ready_sampler_patch(e.session(), t).unwrap().clone()
}

fn lp(e: &Engine, t: TrackId) -> SliceLoop {
    patch(e, t).slicing.unwrap()
}

/// Test setup outside the commands.
fn edit(e: &mut Engine, t: TrackId, f: impl FnOnce(&mut PatchV1)) {
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    f(ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap());
}

fn set(list: &[(u32, u64, Option<&str>)]) -> Markers {
    let markers = list.iter().map(|&(id, frame, label)| Marker { id, frame, label: label.map(str::to_string), ..Marker::default() }).collect();
    Markers { markers, ..Markers::default() }
}

/// Binds zone `zone` (0-based) to slice `id` of `set`, on pad `pad` (0-based).
fn bind(e: &mut Engine, t: TrackId, zone: u16, set: SliceSet, id: u32, pad: u8) {
    edit(e, t, |p| {
        let mut z = ZoneEntry::whole(zone, 0, 48_000);
        z.slice = Some(SliceBinding { set, id, ..SliceBinding::default() });
        p.zones.push(z);
        p.zones.sort_by_key(|z| z.id);
        p.set_pad(PadEntry { zone: Some(zone), ..p.pad(pad) });
    });
}

fn detect(e: &mut Engine, t: TrackId, params: Value) -> u64 {
    let mut p = params;
    p["track"] = json!(t);
    let out = e.execute("mix.sampler_slice_detect", &p).unwrap_or_else(|er| panic!("detect {p}: {er}"));
    assert_eq!(out["state"], "pending");
    out["candidate_id"].as_u64().unwrap()
}

fn cand(e: &mut Engine, t: TrackId) -> Value {
    e.execute("mix.sampler_slice_candidate", &json!({"track": t})).unwrap()["candidate"].clone()
}

/// The candidate's marker frames.
fn frames(c: &Value) -> Vec<u64> {
    c["markers"].as_array().unwrap_or_else(|| panic!("no markers: {c}")).iter().map(|m| m["frame"].as_u64().unwrap()).collect()
}

fn auto_frames(e: &Engine, t: TrackId) -> Vec<u64> {
    lp(e, t).auto.map(|m| m.markers.iter().map(|x| x.frame).collect()).unwrap_or_default()
}

fn replace(e: &mut Engine, t: TrackId, cid: u64) -> crate::Result<Value> {
    e.execute("mix.sampler_slice_replace", &json!({"track": t, "candidate_id": cid}))
}

/// The refusal text of a refused Replace, after checking the document and history are unchanged.
fn replace_refused(e: &mut Engine, t: TrackId, cid: u64) -> String {
    let (before, n) = (e.session_arc(), e.undo_history().len());
    let r = replace(e, t, cid);
    assert!(*e.session_arc() == *before, "refused but the document changed");
    assert_eq!(e.undo_history().len(), n, "refused but a step was recorded");
    match r {
        Err(EngineError::BadParams(_, m)) => m,
        other => panic!("not refused: {other:?}"),
    }
}

/// Runs Replace expecting one undo step: undo restores the document whole, redo reproduces it.
fn replace_one_step(e: &mut Engine, t: TrackId, cid: u64) -> Value {
    let (before, n) = (e.session_arc(), e.undo_history().len());
    let out = replace(e, t, cid).unwrap_or_else(|er| panic!("replace {cid}: {er}"));
    let after = e.session_arc();
    assert_eq!(e.undo_history().len(), n + 1, "not exactly one step");
    e.undo();
    assert!(*e.session_arc() == *before, "one undo does not restore the document");
    e.redo();
    assert!(*e.session_arc() == *after, "redo does not reproduce the edit");
    out
}

// ---------------------------------------------------------------------------------------------
// Fixtures: hits and WAV files

fn noise(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed as f32 / u32::MAX as f32 * 2.0 - 1.0
}

/// A noise hit (high-passed, exponential decay), as in the detector's own tests.
fn hit(level: f32, decay: f32, seed: &mut u32) -> Vec<f32> {
    let mut prev = 0.0;
    (0..(decay * 6.0 * SR as f32) as usize)
        .map(|i| {
            let w = noise(seed);
            let hp = w - prev;
            prev = w;
            level * hp * (-(i as f32) / SR as f32 / decay).exp()
        })
        .collect()
}

/// Adds `sig` times `gain` into `x` from frame `at`, with a 30 ms fade at its end.
fn put(x: &mut [f32], at: usize, sig: &[f32], gain: f32) {
    let fade = (SR as usize * 3 / 100).min(sig.len());
    for (i, v) in sig.iter().enumerate() {
        let left = sig.len() - i;
        let g = if left < fade { 0.5 - 0.5 * (std::f32::consts::PI * left as f32 / fade as f32).cos() } else { 1.0 };
        if let Some(o) = x.get_mut(at + i) {
            *o += v * g * gain;
        }
    }
}

/// A RIFF chunk.
fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(body.len() as u32).to_le_bytes());
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

/// A `cue ` chunk of (id, frame) points.
fn cue_chunk(points: &[(u32, u32)]) -> Vec<u8> {
    let mut c = (points.len() as u32).to_le_bytes().to_vec();
    for &(id, frame) in points {
        c.extend_from_slice(&id.to_le_bytes());
        c.extend_from_slice(&frame.to_le_bytes()); // dwPosition
        c.extend_from_slice(b"data");
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&frame.to_le_bytes()); // dwSampleOffset
    }
    chunk(b"cue ", &c)
}

/// A `LIST`/`adtl` of `labl` sub-chunks.
fn labels(list: &[(u32, &str)]) -> Vec<u8> {
    let mut body = b"adtl".to_vec();
    for (id, text) in list {
        let mut l = id.to_le_bytes().to_vec();
        l.extend_from_slice(text.as_bytes());
        l.push(0);
        body.extend_from_slice(&chunk(b"labl", &l));
    }
    chunk(b"LIST", &body)
}

/// An `acid` chunk with `bpm` as its tempo.
fn acid(bpm: f32) -> Vec<u8> {
    let mut body = vec![0u8; 20];
    body.extend_from_slice(&bpm.to_le_bytes());
    chunk(b"acid", &body)
}

/// Writes `channels` at `rate` as a float WAV file with `extra` chunks after the audio.
fn write_wav(path: &str, channels: &[Vec<f32>], rate: u32, extra: &[Vec<u8>]) {
    let buf = AudioBuffer { sample_rate: rate, channels: channels.to_vec() };
    let mut b = transit_audio_io::encode(&buf, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    for c in extra {
        b.extend_from_slice(c);
    }
    let size = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&size.to_le_bytes());
    std::fs::write(path, b).unwrap();
}

/// A loop with a file on disk holding `extra` chunks; `channels` are its audio at `rate`.
fn file_loop(d: &Dir, name: &str, channels: Vec<Vec<f32>>, rate: u32, extra: &[Vec<u8>]) -> (Engine, TrackId) {
    let path = d.path(name);
    write_wav(&path, &channels, rate, extra);
    let (mut e, t) = engine();
    with_loop(&mut e, t, channels, rate, Some(&path));
    (e, t)
}

// ---------------------------------------------------------------------------------------------
// Transients

/// The synthetic stereo loop (2 s at 48 kHz) and its constructed onsets: loud hits, a quiet ghost
/// hit, an opposite-phase hit (left +h, right -h: it cancels when the channels are summed), a flam
/// 40 ms apart (above the detector's 30 ms gap) and one 15 ms apart (below it: merged).
fn hits_loop() -> (Vec<Vec<f32>>, Vec<u64>) {
    let len = 2 * SR as usize;
    let (mut l, mut r) = (vec![0.0f32; len], vec![0.0f32; len]);
    let mut seed = 11;
    let ms = |m: usize| m * SR as usize / 1000;
    let mut onsets = Vec::new();
    for at in [ms(100), ms(600), ms(1100)] {
        let h = hit(0.9, 0.04, &mut seed);
        put(&mut l, at, &h, 1.0);
        put(&mut r, at, &h, 1.0);
        onsets.push(at as u64);
    }
    // The ghost hit.
    let h = hit(0.09, 0.03, &mut seed);
    put(&mut l, ms(350), &h, 1.0);
    put(&mut r, ms(350), &h, 1.0);
    onsets.push(ms(350) as u64);
    // The opposite-phase hit.
    let h = hit(0.9, 0.04, &mut seed);
    put(&mut l, ms(850), &h, 1.0);
    put(&mut r, ms(850), &h, -1.0);
    onsets.push(ms(850) as u64);
    // The flam above the gap: both hits are onsets.
    for at in [ms(1400), ms(1440)] {
        let h = hit(0.9, 0.02, &mut seed);
        put(&mut l, at, &h, 1.0);
        put(&mut r, at, &h, 1.0);
        onsets.push(at as u64);
    }
    // The flam below the gap: merged, only its first hit is an onset.
    for (k, at) in [ms(1700), ms(1715)].into_iter().enumerate() {
        let h = hit(0.9, 0.02, &mut seed);
        put(&mut l, at, &h, 1.0);
        put(&mut r, at, &h, 1.0);
        if k == 0 {
            onsets.push(at as u64);
        }
    }
    onsets.sort_unstable();
    (vec![l, r], onsets)
}

/// Each accepted onset lies within 1 ms of its constructed attack (the bound the detector's own
/// hit tests hold), with no extras. Flams closer than the 30 ms gap are manual-marker territory.
#[test]
fn detects_synthetic_loop_hits_with_ghost_and_opposite_phase() {
    let (ch, onsets) = hits_loop();
    let (mut e, t) = engine();
    with_loop(&mut e, t, ch, SR, None);
    detect(&mut e, t, json!({"source": "transients"}));
    let c = cand(&mut e, t);
    assert_eq!(c["source_used"], "transients", "{c}");
    let f = frames(&c);
    assert_eq!(f.first(), Some(&0), "S first");
    let found = &f[1..];
    let window = u64::from(SR) / 1000;
    assert!(found.len() == onsets.len() && found.iter().zip(&onsets).all(|(a, b)| a.abs_diff(*b) <= window), "{found:?} vs {onsets:?}");
    // Deterministic: the same input gives the same marker array.
    detect(&mut e, t, json!({"source": "transients"}));
    assert_eq!(frames(&cand(&mut e, t)), f);
}

/// Loud hits at `at` in a stereo loop of `frames` at `rate`.
fn hits_at(frames: usize, at: &[usize]) -> Vec<Vec<f32>> {
    let mut x = vec![0.0f32; frames];
    let mut seed = 5;
    for a in at {
        put(&mut x, *a, &hit(0.9, 0.04, &mut seed), 1.0);
    }
    vec![x.clone(), x]
}

/// Grid starts every `step` frames over a loop of `len` frames.
fn grid(step: u64, len: u64) -> Vec<u64> {
    (0..).map(|k| k * step).take_while(|f| *f < len).collect()
}

// ---------------------------------------------------------------------------------------------
// Cue points

#[test]
fn cue_source_wins_over_transients() {
    let d = Dir::new("cues-win");
    let audio = hits_at(48_000, &[12_000, 36_000]);
    let cues = [cue_chunk(&[(1, 30_000), (2, 6_000)]), labels(&[(1, "snare"), (2, "kick")])];
    let (mut e, t) = file_loop(&d, "cues.wav", audio.clone(), SR, &cues);
    detect(&mut e, t, json!({}));
    let c = cand(&mut e, t);
    assert_eq!(c["source_used"], "cues", "{c}");
    assert_eq!(c["markers"], json!([{"frame": 0}, {"frame": 6_000, "label": "kick"}, {"frame": 30_000, "label": "snare"}]));
    assert_eq!((c["count"].clone(), c["current_count"].clone()), (json!(3), json!(1)));
    detect(&mut e, t, json!({"source": "transients"}));
    let c = cand(&mut e, t);
    assert_eq!(c["source_used"], "transients");
    let f = frames(&c);
    assert!(f.len() == 3 && f[0] == 0 && f[1].abs_diff(12_000) <= 48 && f[2].abs_diff(36_000) <= 48, "{f:?}");
    // A file without cue points: explicit cues is empty with a note; Auto goes on to transients.
    let (mut e, t) = file_loop(&d, "plain.wav", audio, SR, &[]);
    detect(&mut e, t, json!({"source": "cues"}));
    let c = cand(&mut e, t);
    assert_eq!((c["source_used"].clone(), frames(&c)), (json!("cues"), vec![0]));
    assert_eq!(c["note"], "No usable cue points in this file.");
    detect(&mut e, t, json!({"source": "auto"}));
    assert_eq!(cand(&mut e, t)["source_used"], "transients");
}

#[test]
fn cue_frames_scale_once_to_the_session_rate() {
    let d = Dir::new("cue-rate");
    // A 44.1 kHz file of 44,100 frames: 48,000 frames once loaded at 48 kHz. Cue id 1 marks two
    // frames (only the first carries its label); 44,100 and 50,000 are past the file's end.
    let cues = [cue_chunk(&[(1, 1_000), (1, 20_000), (2, 44_100), (3, 50_000)]), labels(&[(1, "a")])];
    let (mut e, t) = file_loop(&d, "c44.wav", silence(44_100), 44_100, &cues);
    assert_eq!(lp(&e, t).end, 48_000);
    detect(&mut e, t, json!({"source": "cues"}));
    let c = cand(&mut e, t);
    let want: Vec<u64> = [1_000u64, 20_000].iter().map(|f| crate::cmd::frames_at(*f, 44_100, SR).unwrap()).collect();
    assert_eq!(want, [1_089, 21_769], "frames_at rounds up");
    assert_eq!(c["markers"], json!([{"frame": 0}, {"frame": 1_089, "label": "a"}, {"frame": 21_769}]), "{c}");
    assert_eq!(c["note"], "2 cue points outside the loop left out.");
}

#[test]
fn too_many_cue_points_are_named_and_count_toward_capacity() {
    let d = Dir::new("cue-many");
    let points: Vec<(u32, u32)> = (1..=65_537).map(|i| (i, i)).collect();
    let (mut e, t) = file_loop(&d, "many.wav", silence(72_000), SR, &[cue_chunk(&points)]);
    detect(&mut e, t, json!({"source": "cues"}));
    let c = cand(&mut e, t);
    assert_eq!(c["state"], "refused");
    assert_eq!(c["error"], "over capacity: 65537 slices, at most 64 The file holds more than 65536 cue points; only the first 65536 were read.");
}

// ---------------------------------------------------------------------------------------------
// The grid and its tempo

#[test]
fn grid_tempo_priority_chain() {
    let d = Dir::new("chain");
    // A 1 s loop whose file has an `acid` tempo of 100 BPM; the session runs 120 BPM in 4/4.
    let (mut e, t) = file_loop(&d, "acid.wav", silence(48_000), SR, &[acid(100.0)]);
    let run = |e: &mut Engine, t: TrackId, p: Value| {
        detect(e, t, p);
        let c = cand(e, t);
        assert_eq!(c["state"], "ready", "{c}");
        (c["tempo_origin"].as_str().unwrap().to_string(), c["bpm"].as_f64().unwrap(), frames(&c))
    };
    // 1/16 steps: 720,000 / BPM frames.
    let r = run(&mut e, t, json!({"source": "grid", "bars": 1, "grid_bpm": 96, "at": 0}));
    assert_eq!(r, ("bars".into(), 240.0, grid(3_000, 48_000)), "bars wins over grid_bpm, acid and the session");
    let r = run(&mut e, t, json!({"source": "grid", "grid_bpm": 96, "at": 0}));
    assert_eq!(r, ("grid_bpm".into(), 96.0, grid(7_500, 48_000)), "grid_bpm wins over acid and the session");
    let r = run(&mut e, t, json!({"source": "grid", "at": 0}));
    assert_eq!(r, ("acid".into(), 100.0, grid(7_200, 48_000)), "acid wins over the session");
    let (mut m, mt) = engine();
    with_loop(&mut m, mt, silence(48_000), SR, None);
    let r = run(&mut m, mt, json!({"source": "grid", "at": 0}));
    assert_eq!(r, ("session".into(), 120.0, grid(6_000, 48_000)), "the session tempo last");
    // An invalid `acid` tempo falls through to the session.
    let (mut b, bt) = file_loop(&d, "bad.wav", silence(48_000), SR, &[acid(5_000.0)]);
    let r = run(&mut b, bt, json!({"source": "grid", "at": 0}));
    assert_eq!(r, ("session".into(), 120.0, grid(6_000, 48_000)));
    // grid_bpm round-trips through Replace: stored, then the default for the next Detect.
    let cid = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 96}));
    replace_one_step(&mut e, t, cid);
    assert_eq!(lp(&e, t).grid_bpm, Some(96.0));
    assert_eq!(serde_json::to_value(patch(&e, t)).unwrap()["loop"]["grid_bpm"], json!(96.0));
    let r = run(&mut e, t, json!({}));
    assert_eq!(r.0, "grid_bpm", "the stored source and grid_bpm are the defaults");
    let r = run(&mut e, t, json!({"grid_bpm": null}));
    assert_eq!(r.0, "acid", "null clears it for this detection");
}

#[test]
fn acid_tempo_outside_the_grid_range_falls_back_to_the_session() {
    let d = Dir::new("acid-range");
    for (name, bpm) in [("slow.wav", 0.5f32), ("fast.wav", 5_000.0)] {
        let (mut e, t) = file_loop(&d, name, silence(48_000), SR, &[acid(bpm)]);
        detect(&mut e, t, json!({"source": "grid", "at": 0}));
        let c = cand(&mut e, t);
        assert_eq!((c["tempo_origin"].clone(), c["bpm"].clone()), (json!("session"), json!(120.0)), "{c}");
        assert_eq!(c["note"], format!("The file's acid tempo of {bpm:.2} BPM is outside 5 to 1000 BPM, so the session tempo is used."));
    }
}

#[test]
fn detect_refuses_without_at_when_the_session_tempo_is_the_source_and_when_bars_is_explicit() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    let refusal = |e: &mut Engine, p: Value| {
        detect(e, t, p);
        let c = cand(e, t);
        assert_eq!(c["state"], "refused", "{c}");
        c["error"].as_str().unwrap().to_string()
    };
    let needs = "Detect needs a placement (at) to read the session tempo.";
    assert_eq!(refusal(&mut e, json!({"source": "grid"})), needs, "the session tempo is the source");
    assert_eq!(refusal(&mut e, json!({"source": "grid", "bars": 2, "grid_bpm": 96})), needs, "Bars reads the meter at the placement");
    assert_eq!(refusal(&mut e, json!({})), needs, "Auto reaching the grid (no cue points, no transients)");
    // Without `at` from grid_bpm or a valid acid tempo.
    detect(&mut e, t, json!({"source": "grid", "grid_bpm": 96}));
    assert_eq!(cand(&mut e, t)["tempo_origin"], "grid_bpm");
    let d = Dir::new("no-at");
    let (mut a, at) = file_loop(&d, "acid.wav", silence(48_000), SR, &[acid(100.0)]);
    detect(&mut a, at, json!({"source": "grid"}));
    assert_eq!(cand(&mut a, at)["tempo_origin"], "acid");
}

#[test]
fn grid_bars_override_reports_derived_bpm() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    // 4/4: two bars are 8 quarter notes over 1 s, 480 BPM; 1/16 steps of 1,500 frames.
    detect(&mut e, t, json!({"source": "grid", "bars": 2, "at": 0}));
    let c = cand(&mut e, t);
    assert_eq!((c["tempo_origin"].clone(), c["derived_bpm"].clone()), (json!("bars"), json!(480.0)), "{c}");
    assert_eq!(frames(&c), grid(1_500, 48_000));
    // 3/4: two bars are 6 quarter notes, 360 BPM; steps of 2,000 frames.
    e.session_mut().tempo.set_meter(0, 3, 4).unwrap();
    detect(&mut e, t, json!({"source": "grid", "bars": 2, "at": 0}));
    let c = cand(&mut e, t);
    assert_eq!(c["derived_bpm"], json!(360.0));
    assert_eq!(frames(&c), grid(2_000, 48_000));
    // From tempo again: no derived BPM.
    detect(&mut e, t, json!({"source": "grid", "bars": null, "grid_bpm": 96}));
    let c = cand(&mut e, t);
    assert_eq!((c["tempo_origin"].clone(), c.get("derived_bpm")), (json!("grid_bpm"), None));
}

#[test]
fn over_capacity_candidate_refused_not_truncated() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(&[(1, 0, None), (2, 24_000, None)]));
        l.manual = Some(set(&[(5, 0, None), (6, 9_000, Some("m"))]));
    });
    let before = lp(&e, t);
    // 1/32 at 480 BPM: steps of 750 frames, exactly 64 slices.
    detect(&mut e, t, json!({"source": "grid", "division": "1/32", "grid_bpm": 480}));
    assert_eq!(cand(&mut e, t)["count"], 64);
    // At 485 BPM the grid makes 65 slices: refused whole.
    let cid = detect(&mut e, t, json!({"source": "grid", "division": "1/32", "grid_bpm": 485}));
    let c = cand(&mut e, t);
    assert_eq!((c["state"].clone(), c["error"].clone()), (json!("refused"), json!("over capacity: 65 slices, at most 64")));
    assert_eq!(replace_refused(&mut e, t, cid), "This detection was refused: over capacity: 65 slices, at most 64");
    assert_eq!(lp(&e, t), before, "neither set changes");
}

// ---------------------------------------------------------------------------------------------
// Replace markers

#[test]
fn replace_keeps_ids_at_unchanged_frames_and_reports_freed_bindings() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(Markers { next_id: 4, ..set(&[(1, 0, None), (2, 6_000, None), (3, 9_000, Some("old"))]) });
        l.manual = Some(set(&[(1, 0, None), (3, 9_000, None)]));
    });
    bind(&mut e, t, 0, SliceSet::Auto, 2, 0);
    bind(&mut e, t, 1, SliceSet::Auto, 3, 1);
    bind(&mut e, t, 2, SliceSet::Manual, 3, 2);
    // A 120 BPM 1/16 grid: 0, 6,000 ... 42,000.
    let cid = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    let out = replace_one_step(&mut e, t, cid);
    assert_eq!(out["kept"], json!([1, 2]));
    assert_eq!(out["removed"], json!([3]));
    assert_eq!(out["new"], json!([4, 5, 6, 7, 8, 9]));
    assert_eq!((out["freed_pads"].clone(), out["freed_zones"].clone()), (json!([2]), json!([2])));
    let auto = lp(&e, t).auto.unwrap();
    assert_eq!(
        auto.markers.iter().map(|m| (m.id, m.frame)).collect::<Vec<_>>(),
        [1, 2, 4, 5, 6, 7, 8, 9].into_iter().zip(grid(6_000, 48_000)).collect::<Vec<_>>()
    );
    let p = patch(&e, t);
    let bound = |z: u16| p.zones.iter().find(|x| x.id == z).and_then(|x| x.slice.clone()).map(|b| (b.set, b.id));
    assert_eq!(
        (bound(0), bound(1), bound(2)),
        (Some((SliceSet::Auto, 2)), None, Some((SliceSet::Manual, 3))),
        "Auto id 2 survives; Manual is not touched"
    );
    // A harmless re-detect keeps every id and records no step.
    let n = e.undo_history().len();
    let cid = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    let out = replace(&mut e, t, cid).unwrap();
    assert_eq!((out["new"].clone(), out["removed"].clone(), e.undo_history().len()), (json!([]), json!([]), n));
}

#[test]
fn replace_updates_auto_only() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.manual = Some(Markers { next_id: 9, ..set(&[(7, 0, Some("s")), (8, 10_000, Some("x"))]) });
        l.mode = SliceMode::Manual;
    });
    bind(&mut e, t, 0, SliceSet::Manual, 8, 0);
    let before = patch(&e, t);
    let cid = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    replace_one_step(&mut e, t, cid);
    let after = patch(&e, t);
    let (l0, l1) = (before.slicing.clone().unwrap(), after.slicing.clone().unwrap());
    assert_eq!((l1.manual, l1.mode), (l0.manual, l0.mode), "Manual (ids, labels) and the mode are unchanged");
    assert_eq!((&after.zones, &after.pads), (&before.zones, &before.pads), "Manual's bindings are unchanged");
    assert_eq!(auto_frames(&e, t), grid(6_000, 48_000));
    e.undo();
    assert_eq!(patch(&e, t), before, "one undo restores Auto and the settings exactly");
}

#[test]
fn replace_stores_the_candidate_settings_in_one_step() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    let before = lp(&e, t);
    let cid = detect(&mut e, t, json!({"source": "grid", "sensitivity": 0.25, "division": "1/8", "bars": 2, "grid_bpm": 96, "at": 0}));
    let out = replace_one_step(&mut e, t, cid);
    assert_eq!(out["settings"], json!({"source": "grid", "sensitivity": 0.25, "division": "1/8", "bars": 2, "grid_bpm": 96.0}));
    let l = lp(&e, t);
    assert_eq!(
        (l.detector.source, l.detector.sensitivity, l.division, l.bars, l.grid_bpm),
        (transit_model::sampler_patch::AutoSource::Grid, 0.25, transit_model::sampler_patch::Division::Eighth, Some(2), Some(96.0))
    );
    // Two bars of 4/4 in 1/8 notes: 16 slices of 3,000 frames.
    assert_eq!(auto_frames(&e, t), grid(3_000, 48_000));
    e.undo();
    assert_eq!(lp(&e, t), before, "markers and settings undo together");
}

#[test]
fn settings_change_does_not_stale_its_own_candidate() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    let cid = detect(&mut e, t, json!({"source": "grid", "division": "1/8", "grid_bpm": 120}));
    assert_eq!(cand(&mut e, t)["stale"], false);
    replace(&mut e, t, cid).unwrap();
    assert_eq!(lp(&e, t).division, transit_model::sampler_patch::Division::Eighth);
}

// ---------------------------------------------------------------------------------------------
// The candidate's lifecycle

#[test]
fn each_stale_cause_refuses_replace() {
    let d = Dir::new("stale");
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    let g = json!({"source": "grid", "grid_bpm": 120});
    let snap = |e: &mut Engine, on: bool| {
        e.execute("mix.sampler_slice_set", &json!({"track": t, "snap": on})).unwrap();
    };
    // A patch edit; setting it back does not make the candidate fresh again.
    let cid = detect(&mut e, t, g.clone());
    let detected = patch(&e, t);
    snap(&mut e, false);
    snap(&mut e, true);
    assert_eq!(patch(&e, t), detected);
    assert_eq!(cand(&mut e, t)["stale"], true);
    assert_eq!(replace_refused(&mut e, t, cid), STALE, "patch edit");
    // Undo, then redo.
    snap(&mut e, false);
    let cid = detect(&mut e, t, g.clone());
    e.undo();
    assert_eq!(replace_refused(&mut e, t, cid), STALE, "undo");
    let cid = detect(&mut e, t, g.clone());
    e.redo();
    assert_eq!(replace_refused(&mut e, t, cid), STALE, "redo");
    // A newer candidate.
    let old = detect(&mut e, t, g.clone());
    let new = detect(&mut e, t, g.clone());
    assert_eq!(replace_refused(&mut e, t, old), STALE, "newer candidate");
    assert_eq!(cand(&mut e, t)["candidate_id"], new);
    // The instrument replaced (and the patch brought back by undo): discarded.
    let cid = detect(&mut e, t, g.clone());
    e.execute("mix.instrument", &json!({"track": t, "plugin": "sampler"})).unwrap();
    assert!(ready_sampler_patch(e.session(), t).is_none());
    e.undo();
    assert!(ready_sampler_patch(e.session(), t).is_some());
    assert_eq!(cand(&mut e, t), Value::Null);
    assert_eq!(replace_refused(&mut e, t, cid), STALE, "instrument replaced");
    // The session closed, then opened again: discarded.
    let file = d.path("s.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let cid = detect(&mut e, t, g.clone());
    e.execute("session.close", &json!({})).unwrap();
    e.execute("session.open", &json!({"path": file})).unwrap();
    assert_eq!(cand(&mut e, t), Value::Null);
    assert_eq!(replace_refused(&mut e, t, cid), STALE, "session closed and opened");
    // Pending.
    e.slice_candidates.set_run_for_testing(Run::Held);
    let cid = detect(&mut e, t, g.clone());
    assert_eq!(cand(&mut e, t)["state"], "pending");
    assert_eq!(replace_refused(&mut e, t, cid), PENDING, "pending");
    // An id never handed out.
    assert_eq!(replace_refused(&mut e, t, 999), "No detection 999: detect first.");
}

#[test]
fn stale_candidate_refused() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    let g = json!({"source": "grid", "grid_bpm": 120});
    let first = detect(&mut e, t, g.clone());
    e.execute("mix.sampler_slice_set", &json!({"track": t, "root": 40})).unwrap();
    assert_eq!(replace_refused(&mut e, t, first), STALE);
    let second = detect(&mut e, t, g.clone());
    let third = detect(&mut e, t, g);
    assert_eq!(replace_refused(&mut e, t, second), STALE);
    replace(&mut e, t, third).unwrap();
    assert_eq!(auto_frames(&e, t), grid(6_000, 48_000));
}

#[test]
fn detect_and_discard_are_not_undo_steps_and_the_candidate_is_never_saved() {
    let d = Dir::new("never-saved");
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    let file = d.path("a.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let saved = std::fs::read_to_string(&file).unwrap();
    // The document, history and journal as they are now.
    let mark = |e: &Engine| (e.session_arc(), e.undo_history().len(), e.undo_pushes(), e.journal.len());
    let same = |e: &Engine, m: &(std::sync::Arc<transit_model::Session>, usize, u64, usize)| {
        assert!(std::sync::Arc::ptr_eq(&m.0, &e.session_arc()), "the document is untouched");
        assert_eq!((e.undo_history().len(), e.undo_pushes(), e.journal.len(), e.is_dirty()), (m.1, m.2, m.3, false));
    };
    let doc = mark(&e);
    let cid = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    assert_eq!(cand(&mut e, t)["state"], "ready");
    same(&e, &doc);
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), saved, "the saved session holds no candidate");
    // Discarding another id leaves the candidate; discarding its own drops it.
    let doc = mark(&e);
    let out = e.execute("mix.sampler_slice_discard", &json!({"track": t, "candidate_id": cid + 100})).unwrap();
    assert_eq!((out["discarded"].clone(), cand(&mut e, t)["candidate_id"].clone()), (Value::Null, json!(cid)));
    let out = e.execute("mix.sampler_slice_discard", &json!({"track": t, "candidate_id": cid})).unwrap();
    assert_eq!((out["discarded"].clone(), cand(&mut e, t)), (json!(cid), Value::Null));
    same(&e, &doc);
}

#[test]
fn detection_never_runs_on_reopen() {
    let d = Dir::new("reopen");
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(&[(1, 0, None), (2, 24_000, Some("kick"))]));
        l.manual = Some(set(&[(7, 0, None), (8, 10_000, Some("x"))]));
    });
    let want = lp(&e, t);
    let file = d.path("both.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": file})).unwrap();
    assert_eq!(back.slice_candidates.workers_started(), 0, "opening starts no detection");
    assert_eq!((lp(&back, t).auto, lp(&back, t).manual), (want.auto, want.manual));
    // An engine that has detected starts nothing more by opening.
    detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    let n = e.slice_candidates.workers_started();
    e.execute("session.open", &json!({"path": file})).unwrap();
    assert_eq!(e.slice_candidates.workers_started(), n);
}

#[test]
fn detect_is_cancelled_by_a_newer_detect() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    e.slice_candidates.set_run_for_testing(Run::Held);
    let older = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    let newer = detect(&mut e, t, json!({"source": "grid", "grid_bpm": 96}));
    let held = e.slice_candidates.take_held_for_testing();
    assert_eq!(held.iter().map(|h| h.id).collect::<Vec<_>>(), [older, newer]);
    // The older job sees its cancel flag and its result never lands.
    assert_eq!(e.slice_candidates.run_held_for_testing(&held[0]), Err(Stop::Cancelled));
    assert_eq!(cand(&mut e, t)["state"], "pending");
    assert!(e.slice_candidates.run_held_for_testing(&held[1]).is_ok());
    let c = cand(&mut e, t);
    assert_eq!((c["candidate_id"].clone(), c["state"].clone(), frames(&c)), (json!(newer), json!("ready"), grid(7_500, 48_000)));
    assert_eq!(replace_refused(&mut e, t, older), STALE);
}

#[test]
fn detect_runs_on_a_worker_thread() {
    let (mut e, t) = engine();
    with_loop(&mut e, t, silence(48_000), SR, None);
    e.slice_candidates.set_run_for_testing(Run::Thread);
    detect(&mut e, t, json!({"source": "grid", "grid_bpm": 120}));
    assert_eq!(e.slice_candidates.join_for_testing(), [Some(crate::slice_detect::WORKER.to_string())]);
    assert_eq!(frames(&cand(&mut e, t)), grid(6_000, 48_000));
}
