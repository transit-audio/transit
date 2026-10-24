//! The External Instrument (ADR 0020): an instrument track's instrument slot playing a hardware
//! synth. The MIDI goes to the track's `midi_output` port (#61), the synth's audio comes back on
//! an input path, and its latency (set by hand in this slice) is compensated like a hardware
//! insert's round trip. One command, `instrument.external`, writes the device and the MIDI
//! destination together as one undo step.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{EXTERNAL_GAIN_DB, Insert, MAX_EXTERNAL_LATENCY_MS, MidiOutput, TrackKind};

const ID: &str = "instrument.external";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "instrument.external",
        "External Instrument",
        [],
        None,
        "{track?, port?: {backend,name,ordinal?,id?}, channel?: as_recorded|1..16, return?: input path, gain_db?: -60..24, manual_ms?: 0..500|null} — puts an External Instrument in an instrument track's instrument slot, or changes it; the port and channel are the track's MIDI output, whose offset is held at 0 (the latency is set here); omitted fields keep their values",
        has_selection,
        set_external
    )]
}

/// Fields `instrument.external` reads; anything else is refused, so a misspelled field is not
/// silently ignored.
const FIELDS: [&str; 8] = ["track", "tracks", "port", "channel", "return", "gain_db", "manual_ms", "offset_ms"];

fn gain_param(v: &Value) -> Result<f32> {
    v.as_f64()
        .filter(|g| g.is_finite() && (f64::from(EXTERNAL_GAIN_DB.0)..=f64::from(EXTERNAL_GAIN_DB.1)).contains(g))
        .map(|g| g as f32)
        .ok_or_else(|| bad(ID, format!("`gain_db` must be a number in {}..{}", EXTERNAL_GAIN_DB.0, EXTERNAL_GAIN_DB.1)))
}

/// `Some(None)` clears the manual value.
fn manual_param(v: &Value) -> Result<Option<f32>> {
    if v.is_null() {
        return Ok(None);
    }
    v.as_f64()
        .filter(|ms| ms.is_finite() && (0.0..=f64::from(MAX_EXTERNAL_LATENCY_MS)).contains(ms))
        .map(|ms| Some(ms as f32))
        .ok_or_else(|| bad(ID, format!("`manual_ms` must be a number in 0..{MAX_EXTERNAL_LATENCY_MS}, or null to clear it")))
}

fn return_param(e: &Engine, v: &Value) -> Result<String> {
    let name = v.as_str().ok_or_else(|| bad(ID, "`return` must be an input path name"))?;
    if name.len() > 1024 {
        return Err(bad(ID, "`return` has an invalid length"));
    }
    // An empty name leaves the return unchosen. A name this machine's table does not have is
    // kept (the session may come from another interface) and reported missing.
    if !name.is_empty()
        && let Some(path) = e.io_map().path(name)
        && path.direction != soundcraft_model::IoDirection::Input
    {
        return Err(bad(ID, format!("`{name}` is an output path; the return is an input path")));
    }
    Ok(name.to_owned())
}

fn set_external(e: &mut Engine, p: &Value) -> Result<Value> {
    if let Some(k) = p.as_object().and_then(|o| o.keys().find(|k| !FIELDS.contains(&k.as_str()))) {
        return Err(bad(ID, format!("unknown field `{k}`")));
    }
    if p.get("offset_ms").is_some() {
        return Err(bad(ID, "Latency is set on the External Instrument (`manual_ms`); the MIDI offset is held at 0"));
    }
    let t = tracks_required(e, ID, p)?.first().copied().ok_or_else(|| bad(ID, "no track"))?;
    let track = e.session().track(t).ok_or_else(|| bad(ID, format!("no track {}", t.0)))?;
    if track.kind != TrackKind::Instrument {
        return Err(bad(ID, "an External Instrument goes in an instrument track's instrument slot"));
    }
    // Validate everything before changing anything, so a refusal changes nothing.
    let port = match p.get("port") {
        Some(v) => Some(super::midi_output::port_param(v)?.ok_or_else(|| bad(ID, "`port` must be a MIDI output port object"))?),
        None => None,
    };
    let channel = p.get("channel").map(super::midi_output::channel_param).transpose()?;
    let ret = p.get("return").map(|v| return_param(e, v)).transpose()?;
    let gain = p.get("gain_db").map(gain_param).transpose()?;
    let manual = p.get("manual_ms").map(manual_param).transpose()?;
    let current = track.external_instrument().cloned();
    let output = match (port, track.midi_output.clone()) {
        (Some(port), MidiOutput::External { channel: c, .. }) => MidiOutput::External { port, channel: channel.unwrap_or(c), offset_ms: 0.0 },
        (Some(port), MidiOutput::Internal) => MidiOutput::External { port, channel: channel.unwrap_or_default(), offset_ms: 0.0 },
        (None, MidiOutput::External { port, channel: c, .. }) => MidiOutput::External { port, channel: channel.unwrap_or(c), offset_ms: 0.0 },
        (None, MidiOutput::Internal) if channel.is_some() => return Err(bad(ID, "choose a MIDI port before its channel")),
        (None, MidiOutput::Internal) => MidiOutput::Internal,
    };
    let mut device = current.clone().unwrap_or_default();
    if let Some(ret) = ret {
        device.ret = ret;
    }
    if let Some(gain) = gain {
        device.gain_db = gain;
    }
    if let Some(manual) = manual {
        device.manual_ms = manual;
    }
    let keep = e.session().track(t).and_then(|tr| tr.instrument.clone()).filter(|_| current.is_some());
    let mut ins = keep.unwrap_or_else(|| Insert::external(device.ret.clone()));
    ins.external = Some(device.clone());
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument = Some(ins);
        tr.midi_output = output;
    }
    Ok(json!({"track": t, "return": device.ret, "gain_db": device.gain_db, "manual_ms": device.manual_ms}))
}

/// The tracks an offline render of `s` would leave out: every active, unfrozen External
/// Instrument track, by name. Its sound exists only while the synth plays in real time (ADR 0020
/// decision 9), so a render of the session with one is refused unless the caller chose to render
/// without it.
#[must_use]
pub fn external_in_render(s: &soundcraft_model::Session) -> Vec<String> {
    s.tracks.iter().filter(|t| !t.inactive && t.freeze.is_none() && t.external_instrument().is_some()).map(|t| t.name.clone()).collect()
}

/// The refusal of an offline render that includes External Instruments (decision 9), or the
/// tracks it leaves out when `without_external: true` asks for that explicitly. Never a silent skip.
pub(crate) fn render_guard(e: &Engine, id: &str, p: &Value) -> Result<Vec<String>> {
    let names = external_in_render(e.session());
    if names.is_empty() || bool_or(p, "without_external", false) {
        return Ok(names);
    }
    Err(bad(
        id,
        format!(
            "This range has an External Instrument ({}), which can only render in real time: pass `without_external: true` to render without it",
            names.join(", ")
        ),
    ))
}

/// Freeze, flatten and bounce in place of an External Instrument track itself: it has nothing
/// to render offline, and the real-time render (ADR 0020 slice 3) is not here yet.
pub(crate) fn refuse_offline_print(id: &str, t: &soundcraft_model::Track) -> Result<()> {
    if t.external_instrument().is_some() && t.freeze.is_none() {
        return Err(bad(
            id,
            format!("`{}` is an External Instrument, which can only render in real time; real-time freeze and bounce are not available yet", t.name),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{EXTERNAL_INSTRUMENT_PLUGIN, MidiOutChannel, MidiOutPort, MidiOutput, TrackId};

    fn instrument_track(e: &mut Engine) -> TrackId {
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        e.session().tracks.last().unwrap().id
    }

    fn port() -> serde_json::Value {
        json!({"backend": "winmm", "name": "MODEL D"})
    }

    #[test]
    fn one_command_writes_the_device_and_the_destination_as_one_undo_step() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        let before = e.session().track(t).unwrap().clone();
        e.execute("instrument.external", &json!({"track": t.0, "port": port(), "channel": 1, "return": "In 1", "gain_db": -6.0, "manual_ms": 19.5}))
            .unwrap();
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.instrument.as_ref().unwrap().plugin, EXTERNAL_INSTRUMENT_PLUGIN);
        let ei = tr.external_instrument().unwrap();
        assert_eq!((ei.ret.as_str(), ei.gain_db, ei.manual_ms), ("In 1", -6.0, Some(19.5)));
        assert_eq!(
            tr.midi_output,
            MidiOutput::External {
                port: MidiOutPort { backend: "winmm".into(), name: "MODEL D".into(), ordinal: 0, id: String::new() },
                channel: MidiOutChannel::Channel(0),
                offset_ms: 0.0
            }
        );
        e.execute("edit.undo", &json!({})).unwrap();
        let tr = e.session().track(t).unwrap();
        assert_eq!((&tr.instrument, &tr.midi_output), (&before.instrument, &before.midi_output), "one undo step takes both back");
    }

    #[test]
    fn omitted_fields_keep_their_values_and_null_clears_the_manual_latency() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        e.execute("instrument.external", &json!({"track": t.0, "port": port(), "return": "In 1", "manual_ms": 20.0})).unwrap();
        e.execute("instrument.external", &json!({"track": t.0, "gain_db": 3.0})).unwrap();
        let ei = e.session().track(t).unwrap().external_instrument().unwrap().clone();
        assert_eq!((ei.ret.as_str(), ei.gain_db, ei.manual_ms), ("In 1", 3.0, Some(20.0)));
        e.execute("instrument.external", &json!({"track": t.0, "manual_ms": null})).unwrap();
        assert_eq!(e.session().track(t).unwrap().external_instrument().unwrap().manual_ms, None);
        assert!(e.session().track(t).unwrap().midi_output.is_external(), "the port stays");
    }

    #[test]
    fn the_destination_offset_is_held_at_zero() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        e.execute("midi.output", &json!({"track": t.0, "port": port(), "offset_ms": 7.0})).unwrap();
        e.execute("instrument.external", &json!({"track": t.0, "return": "In 1"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().midi_output.offset_ms(), 0.0, "adding the device zeroes the offset");
        let err = e.execute("midi.output", &json!({"track": t.0, "offset_ms": 5.0})).unwrap_err().to_string();
        assert!(err.contains("Latency is set on the External Instrument"), "{err}");
        assert!(e.execute("instrument.external", &json!({"track": t.0, "offset_ms": 5.0})).is_err());
        // The port and channel are still chosen through midi.output, which keeps the offset at 0.
        e.execute("midi.output", &json!({"track": t.0, "channel": 3})).unwrap();
        assert_eq!(e.session().track(t).unwrap().midi_output.offset_ms(), 0.0);
        e.execute("midi.output", &json!({"track": t.0, "offset_ms": 0.0})).unwrap();
    }

    #[test]
    fn refuses_other_tracks_bad_values_and_unknown_fields_changing_nothing() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        e.execute("track.new", &json!({"kind": "midi"})).unwrap();
        let midi = e.session().tracks.last().unwrap().id;
        assert!(e.execute("instrument.external", &json!({"track": midi.0, "return": "In 1"})).is_err());
        let before = e.session().track(t).unwrap().clone();
        for bad in [
            json!({"track": t.0, "manual_ms": 501.0}),
            json!({"track": t.0, "manual_ms": -1.0}),
            json!({"track": t.0, "gain_db": 30.0}),
            json!({"track": t.0, "return": 3}),
            json!({"track": t.0, "channel": 2}),
            json!({"track": t.0, "port": "internal"}),
            json!({"track": t.0, "retrun": "In 1"}),
        ] {
            assert!(e.execute("instrument.external", &bad).is_err(), "{bad}");
            assert_eq!(e.session().track(t).unwrap().instrument, before.instrument, "{bad}");
        }
        assert!(e.session().track(t).unwrap().external_instrument().is_none());
    }
    #[test]
    fn an_offline_render_with_an_external_instrument_is_refused_unless_it_is_left_out_explicitly() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        e.session_mut().track_mut(t).unwrap().name = "Model D".into();
        let dir = std::env::temp_dir().join(format!("transit-ext-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mix.wav").to_string_lossy().into_owned();
        // Without the device, the bounce runs as before.
        e.execute("file.bounce_mix", &json!({"path": path, "start": 0, "end": 4800})).unwrap();
        e.execute("instrument.external", &json!({"track": t.0, "port": port(), "return": "In 1"})).unwrap();
        for (id, p) in [
            ("file.bounce_mix", json!({"path": path, "start": 0, "end": 4800})),
            ("file.bounce_stems", json!({"dir": dir.to_string_lossy(), "start": 0, "end": 4800})),
            ("file.bounce_paths", json!({"dir": dir.to_string_lossy(), "start": 0, "end": 4800})),
        ] {
            let err = e.execute(id, &p).unwrap_err().to_string();
            assert!(err.contains("External Instrument (Model D)") && err.contains("real time"), "{id}: {err}");
            let mut p = p;
            p["without_external"] = json!(true);
            let done = e.execute(id, &p).unwrap();
            assert_eq!(done["left_out"], json!(["Model D"]), "{id}: the result names the tracks left out");
        }
        // An inactive device track is not in the render.
        e.session_mut().track_mut(t).unwrap().inactive = true;
        assert!(super::external_in_render(e.session()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn freeze_flatten_and_bounce_in_place_of_the_device_track_are_refused() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        e.execute("instrument.external", &json!({"track": t.0, "return": "In 1"})).unwrap();
        e.execute("midi.new_clip", &json!({"track": t.0})).unwrap();
        for id in ["track.freeze", "track.flatten", "track.bounce", "track.commit"] {
            let err = e.execute(id, &json!({"tracks": [t.0]})).unwrap_err().to_string();
            assert!(err.contains("can only render in real time"), "{id}: {err}");
            assert!(e.session().track(t).unwrap().freeze.is_none());
        }
        assert_eq!(e.session().tracks.len(), 1, "bounce in place made no track");
    }
}
