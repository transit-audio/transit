//! The real-time render (ADR 0027, ADR 0020 slice 3): `file.bounce_mix {mode: "realtime"}`.
//!
//! A render that includes an External Instrument plays the session through the live device: the
//! engine checks what it can see without the device (the transport, each device's port, return
//! and latency) and names the files; the app runs the render on the live Player, which sends the
//! MIDI as playback does and keeps Main and the returns ([`RealtimePlan`]). Run here, the command
//! refuses after its checks, as `instrument.measure_external` does: the device is the app's.

use super::*;
use serde_json::Value;
use soundcraft_model::{IoDirection, TrackId};
use std::path::{Path, PathBuf};

pub(crate) const ID: &str = "file.bounce_mix";

/// How a render with External Instruments runs (`mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// No mode given: refused when the range has an External Instrument.
    Unset,
    /// Through the live device, in real time.
    Realtime,
    /// Offline, leaving the External Instruments out (also `without_external: true`).
    OfflineWithoutExternal,
}

/// `mode?: realtime|offline_without_external`, with the older `without_external: true`.
pub(crate) fn mode_param(id: &str, p: &Value) -> Result<RenderMode> {
    let mode = match str_param(p, "mode").map(str::trim) {
        None | Some("") => RenderMode::Unset,
        Some(m) if m.eq_ignore_ascii_case("realtime") => RenderMode::Realtime,
        Some(m) if m.eq_ignore_ascii_case("offline_without_external") => RenderMode::OfflineWithoutExternal,
        Some(m) => return Err(bad(id, format!("unknown mode `{m}` (use \"realtime\" or \"offline_without_external\")"))),
    };
    Ok(if mode == RenderMode::Unset && bool_or(p, "without_external", false) { RenderMode::OfflineWithoutExternal } else { mode })
}

/// The render mode `file.bounce_mix` reads from `p`.
pub fn realtime_plan_mode(p: &Value) -> Result<RenderMode> {
    mode_param(ID, p)
}

/// One External Instrument of a real-time render.
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeStem {
    pub track: TrackId,
    pub name: String,
    pub ret: String,
    /// The device input channels of the return, from this machine's table.
    pub channels: Vec<u16>,
    /// C from this machine's table (decision 4's lookup); `None` when not measured and the render
    /// was accepted uncompensated.
    pub latency: Option<u32>,
    pub gain_db: f32,
    /// Where its captured return is written, when stems were asked for.
    pub path: Option<PathBuf>,
}

/// A checked real-time render, for the app to run on the live Player.
#[derive(Debug, Clone)]
pub struct RealtimePlan {
    pub path: PathBuf,
    pub range: soundcraft_time::Range,
    pub opts: soundcraft_audio_io::EncodeOptions,
    /// Main's channels and the file's WAV speaker mask.
    pub channels: usize,
    pub mask: u32,
    pub stems: Vec<RealtimeStem>,
}

impl RealtimePlan {
    /// The External Instruments rendered without compensation (accepted by `uncompensated`).
    #[must_use]
    pub fn uncompensated(&self) -> Vec<String> {
        self.stems.iter().filter(|s| s.latency.is_none()).map(|s| s.name.clone()).collect()
    }
}

/// The stem file of `track` next to the mix at `path`: "mix - Model D.wav".
fn stem_path(path: &Path, track: &str) -> PathBuf {
    let stem = path.file_stem().map_or_else(|| "render".into(), |s| s.to_string_lossy().into_owned());
    let ext = path.extension().map_or_else(|| "wav".into(), |s| s.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem} - {}.{ext}", crate::io::sanitize_name(track)))
}

/// Why one External Instrument cannot render in real time, in ADR 0020 decision 7's words.
fn stem_blocker(e: &Engine, t: &soundcraft_model::Track, uncompensated: bool) -> std::result::Result<RealtimeStem, String> {
    let device = t.external_instrument().ok_or("no External Instrument")?;
    if !t.midi_output.is_external() {
        return Err(format!("No MIDI port on {}: choose one on the Out button", t.name));
    }
    if device.ret.is_empty() {
        return Err(format!("Choose the return input of {}", t.name));
    }
    let channels = e
        .io_map()
        .path(&device.ret)
        .filter(|p| p.direction == IoDirection::Input && !p.channels.is_empty())
        .map(|p| p.channels.clone())
        .ok_or_else(|| format!("Return {} of {} is not on this interface", device.ret, t.name))?;
    let latency = e.io_map().external_latency_of(t, e.session().sample_rate.hz()).map(|l| l.frames);
    if latency.is_none() && !uncompensated {
        return Err(format!("{} is not measured: measure it, or render uncompensated (its notes land late by the round trip)", t.name));
    }
    Ok(RealtimeStem { track: t.id, name: t.name.clone(), ret: device.ret.clone(), channels, latency, gain_db: device.gain_db, path: None })
}

/// Check `file.bounce_mix {mode: "realtime", ...}` as far as the engine can see, and plan it.
/// Refuses: the transport running; a range with no External Instrument (it renders offline); a
/// bus source, normalize or a fold-down (not in real time yet); and, per External Instrument, no
/// MIDI port, no return or one not on this interface, and an unknown latency unless
/// `uncompensated: true`. `stems: true` also writes each return to its own file.
pub fn realtime_plan(e: &Engine, p: &Value) -> Result<RealtimePlan> {
    let path = PathBuf::from(str_param(p, "path").ok_or_else(|| bad(ID, "`path` required"))?);
    if e.transport.playing || e.transport.recording {
        return Err(bad(ID, "Stop the transport to render in real time."));
    }
    if str_param(p, "source").map(str::trim).is_some_and(|s| !s.is_empty() && !s.eq_ignore_ascii_case("main")) {
        return Err(bad(ID, "A real-time render writes the main mix; render a bus offline"));
    }
    if bool_or(p, "normalize", false) {
        return Err(bad(ID, "Normalize is not available in a real-time render"));
    }
    if str_param(p, "fold_down").map(str::trim).is_some_and(|f| !f.is_empty() && !f.eq_ignore_ascii_case("none")) {
        return Err(bad(ID, "A fold-down is not available in a real-time render"));
    }
    let s = e.session();
    let externals: Vec<&soundcraft_model::Track> =
        s.tracks.iter().filter(|t| !t.inactive && t.freeze.is_none() && t.external_instrument().is_some()).collect();
    if externals.is_empty() {
        return Err(bad(ID, "Nothing in this session needs real time: render it offline"));
    }
    let uncompensated = bool_or(p, "uncompensated", false);
    let with_stems = bool_or(p, "stems", false);
    let mut stems = Vec::new();
    for t in externals {
        let mut stem = stem_blocker(e, t, uncompensated).map_err(|why| bad(ID, why))?;
        if with_stems {
            stem.path = Some(stem_path(&path, &t.name));
        }
        stems.push(stem);
    }
    let r = range_param(e, ID, p)?;
    let range = if r.is_empty() { soundcraft_time::Range::new(0, s.content_end().max(1)) } else { r };
    crate::io::check_bounce_range(s, range, ID)?;
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("wav").to_ascii_lowercase();
    let format_name = str_param(p, "format").unwrap_or(ext.as_str());
    let format = soundcraft_audio_io::encode_format_for(format_name)
        .ok_or_else(|| bad(ID, format!("cannot write `{format_name}` files (supported: {})", soundcraft_audio_io::ENCODE_EXTENSIONS)))?;
    let bit_depth = match p.get("bit_depth").map(|b| b.to_string().trim_matches('"').to_string()).as_deref() {
        Some("16") => soundcraft_audio_io::BitDepth::Int16,
        Some("32") | Some("32f") => soundcraft_audio_io::BitDepth::Float32,
        _ => soundcraft_audio_io::BitDepth::Int24,
    };
    let opts = soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: bool_or(p, "dither", true), bwf: None };
    for file in std::iter::once(&path).chain(stems.iter().filter_map(|s| s.path.as_ref())) {
        crate::file_access::check_write(file).map_err(EngineError::Io)?;
    }
    let (fmt, channels, _) = crate::io::bounce_layout(s, None, false);
    Ok(RealtimePlan { path, range, opts, channels, mask: fmt.channel_mask(), stems })
}

/// `file.bounce_mix {mode: "realtime"}` run in the engine: checked, then refused, because the
/// render needs the live audio device and MIDI, which are the app's (`StudioApp::run` runs it).
pub(crate) fn realtime(e: &Engine, p: &Value) -> Result<Value> {
    realtime_plan(e, p)?;
    Err(bad(ID, "rendering in real time needs the live audio device and MIDI: run it from the app"))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::TrackId;

    fn engine() -> (Engine, TrackId, String) {
        let mut e = Engine::default();
        e.set_io_map(soundcraft_model::IoMap::for_device("ASIO", "Scarlett", 2, 2));
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        e.session_mut().track_mut(t).unwrap().name = "Model D".into();
        let dir = std::env::temp_dir().join(format!("transit-rt-plan-{}", std::process::id()));
        let path = dir.join("mix.wav").to_string_lossy().into_owned();
        (e, t, path)
    }

    fn port() -> serde_json::Value {
        json!({"backend": "winmm", "name": "MODEL D"})
    }

    /// ADR 0027 decision 2: each refusal the engine can see, in decision 7's words, before
    /// anything is written; an accepted plan names the files and C.
    #[test]
    fn each_refusal_is_said_and_a_good_request_is_planned() {
        let (mut e, t, path) = engine();
        let rt = |e: &mut Engine, extra: serde_json::Value| {
            let mut p = json!({"path": path, "start": 0, "end": 48_000, "mode": "realtime"});
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            e.execute("file.bounce_mix", &p).unwrap_err().to_string()
        };
        assert!(rt(&mut e, json!({})).contains("Nothing in this session needs real time"));
        e.execute("instrument.external", &json!({"track": t.0, "return": "In 1"})).unwrap();
        assert!(rt(&mut e, json!({})).contains("No MIDI port on Model D: choose one on the Out button"));
        e.execute("instrument.external", &json!({"track": t.0, "port": port(), "return": ""})).unwrap();
        assert!(rt(&mut e, json!({})).contains("Choose the return input of Model D"));
        e.execute("instrument.external", &json!({"track": t.0, "return": "In 7"})).unwrap();
        assert!(rt(&mut e, json!({})).contains("Return In 7 of Model D is not on this interface"));
        e.execute("instrument.external", &json!({"track": t.0, "return": "In 1"})).unwrap();
        let unmeasured = rt(&mut e, json!({}));
        assert!(unmeasured.contains("Model D is not measured: measure it, or render uncompensated"), "{unmeasured}");
        // Accepted uncompensated, the request is good: only the live device is missing here.
        assert!(rt(&mut e, json!({"uncompensated": true})).contains("run it from the app"));
        e.transport.playing = true;
        assert!(rt(&mut e, json!({"uncompensated": true})).contains("Stop the transport to render in real time."));
        e.transport.playing = false;
        e.transport.recording = true;
        assert!(rt(&mut e, json!({"uncompensated": true})).contains("Stop the transport to render in real time."));
        e.transport.recording = false;
        assert!(rt(&mut e, json!({"uncompensated": true, "normalize": true})).contains("Normalize is not available"));
        assert!(rt(&mut e, json!({"uncompensated": true, "fold_down": "stereo"})).contains("fold-down"));
        assert!(rt(&mut e, json!({"uncompensated": true, "mode": "live"})).contains("unknown mode"));
        e.execute("instrument.external", &json!({"track": t.0, "manual_ms": 20.0})).unwrap();
        let plan = super::realtime_plan(&e, &json!({"path": path, "start": 0, "end": 48_000, "stems": true})).unwrap();
        assert_eq!(plan.range, soundcraft_time::Range::new(0, 48_000));
        assert_eq!(plan.channels, 2);
        assert_eq!(plan.stems.len(), 1);
        let stem = &plan.stems[0];
        assert_eq!((stem.track, stem.channels.as_slice(), stem.latency), (t, &[0u16][..], Some(960)));
        assert!(stem.path.as_ref().unwrap().ends_with("mix - Model D.wav"), "{:?}", stem.path);
        assert!(plan.uncompensated().is_empty());
        assert!(!std::path::Path::new(&path).exists(), "a refusal writes nothing");
    }

    /// ADR 0027 decision 2: the offline alternative keeps working under its old and new names, and
    /// the refusal now names both ways.
    #[test]
    fn the_offline_alternative_keeps_its_old_name_and_the_refusal_names_both_modes() {
        let (mut e, t, path) = engine();
        e.execute("instrument.external", &json!({"track": t.0, "port": port(), "return": "In 1"})).unwrap();
        let err = e.execute("file.bounce_mix", &json!({"path": path, "start": 0, "end": 4800})).unwrap_err().to_string();
        assert!(err.contains("mode: \"realtime\"") && err.contains("offline_without_external"), "{err}");
        let dir = std::path::Path::new(&path).parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let done = e.execute("file.bounce_mix", &json!({"path": path, "start": 0, "end": 4800, "mode": "offline_without_external"})).unwrap();
        assert_eq!(done["left_out"], json!(["Model D"]));
        let err = e.execute("file.bounce_stems", &json!({"dir": dir.to_string_lossy(), "start": 0, "end": 4800, "mode": "realtime"})).unwrap_err();
        assert!(err.to_string().contains("real time"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
