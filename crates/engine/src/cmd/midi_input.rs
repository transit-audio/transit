//! MIDI input routing. One command changes any number of selected MIDI tracks atomically.

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::{MidiFrom, MidiPort, TrackId};

const ID: &str = "midi.input";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "midi.input",
        "MIDI Input",
        [],
        None,
        "{track?|tracks?, port?: all|none|{backend,name,ordinal,id?}, channel?: all|1..16} — omitted fields keep their current values",
        has_tracks,
        set_input
    )]
}

fn device_text(p: &Value, key: &str, max: usize, allow_empty: bool) -> Result<String> {
    let value = p.get(key).and_then(Value::as_str).ok_or_else(|| bad(ID, format!("device `{key}` must be a string")))?;
    if (!allow_empty && value.is_empty()) || value.len() > max {
        return Err(bad(ID, format!("device `{key}` has an invalid length")));
    }
    Ok(value.to_owned())
}

fn port_param(value: &Value) -> Result<MidiPort> {
    match value.as_str() {
        Some("all") => Ok(MidiPort::All),
        Some("none") => Ok(MidiPort::None),
        _ if value.is_object() => {
            let backend = device_text(value, "backend", 64, false)?;
            let name = device_text(value, "name", 1024, false)?;
            let ordinal = value
                .get("ordinal")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| bad(ID, "device `ordinal` must be an integer in 0..4294967295"))?;
            let id = if value.get("id").is_some() { device_text(value, "id", 4096, true)? } else { String::new() };
            Ok(MidiPort::Device { backend, name, ordinal, id })
        }
        _ => Err(bad(ID, "`port` must be all, none, or a device object")),
    }
}

/// `from`: absent, `null` (clear the route) or a source track by id or name (ADR 0065 1.7).
fn from_param(e: &Engine, p: &Value) -> Result<Option<Option<TrackId>>> {
    match p.get("from") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => Ok(Some(track_param(e, ID, p, "from")?)),
    }
}

/// Refuses a route from `src` to each of `tracks` before any change (ADR 0065 1.5, 1.7): the
/// track itself, a destination or source outside decision 1.5, and a route that closes a loop
/// with any edge of the route graph.
fn refuse_route(e: &Engine, tracks: &[TrackId], src: TrackId) -> Result<()> {
    let s = e.session();
    let source = s.track(src).ok_or_else(|| bad(ID, format!("no track {}", src.0)))?;
    for id in tracks {
        let Some(t) = s.track(*id) else { continue };
        if *id == src {
            return Err(bad(ID, format!("that route would create a routing loop: {}", transit_model::describe_cycle(s, &[src]))));
        }
        if let Some(why) = transit_model::destination_problem(t) {
            return Err(bad(ID, transit_model::destination_refusal(&t.name, why)));
        }
    }
    if let Some(why) = transit_model::source_problem(source) {
        return Err(bad(ID, transit_model::source_refusal(&source.name, why)));
    }
    let changes: Vec<_> = tracks.iter().map(|t| transit_model::RouteChange::Midi { track: *t, from: Some(src) }).collect();
    super::track::refuse_loop(s, ID, &changes)
}

fn set_input(e: &mut Engine, p: &Value) -> Result<Value> {
    if p.get("tracks").is_some_and(|v| !v.is_array()) {
        return Err(bad(ID, "`tracks` must be an array of track ids or names"));
    }
    let mut tracks = tracks_required(e, ID, p)?;
    tracks.sort_by_key(|t| t.0);
    tracks.dedup();
    let from = from_param(e, p)?;
    let listen = p.get("listen").map(|v| v.as_bool().ok_or_else(|| bad(ID, "`listen` must be true or false"))).transpose()?;
    let port = p.get("port").map(port_param).transpose()?;
    let channel = p
        .get("channel")
        .map(|value| {
            if value.as_str() == Some("all") {
                Ok(None)
            } else {
                value
                    .as_u64()
                    .filter(|v| (1..=16).contains(v))
                    .map(|v| Some((v - 1) as u8))
                    .ok_or_else(|| bad(ID, "`channel` must be all or an integer in 1..16"))
            }
        })
        .transpose()?;
    if port.is_none() && channel.is_none() && from.is_none() && listen.is_none() {
        return Err(bad(ID, "supply `port`, `channel`, `from` or `listen`"));
    }
    if matches!(from, Some(Some(_))) && port.is_some() {
        return Err(bad(ID, "supply `from` or `port`, not both: a port clears the route"));
    }
    match from {
        // The route checks name the destination's own reason (ADR 0065 1.7).
        Some(Some(src)) => refuse_route(e, &tracks, src)?,
        // Clearing a route or setting its Listen needs no hardware input.
        _ if port.is_none() && channel.is_none() => {}
        _ => {
            if tracks.iter().any(|id| e.session().track(*id).is_none_or(|t| !t.kind.is_midi())) {
                return Err(bad(ID, "MIDI input is available only on MIDI and instrument tracks"));
            }
        }
    }
    if from.is_none()
        && listen.is_some()
        && let Some(t) = tracks.iter().filter_map(|id| e.session().track(*id)).find(|t| t.midi_from.is_none())
    {
        return Err(bad(ID, format!("{} takes no MIDI from another track", t.name)));
    }
    for id in &tracks {
        if let Some(track) = e.session_mut().track_mut(*id) {
            if let Some(port) = &port {
                track.midi_input.port = port.clone();
                track.midi_from = None;
            }
            if let Some(channel) = channel {
                track.midi_input.channel = channel;
            }
            match from {
                Some(Some(src)) => track.midi_from = Some(MidiFrom { track: src, listen: listen.unwrap_or(true) }),
                Some(None) => track.midi_from = None,
                None => {
                    if let (Some(on), Some(route)) = (listen, track.midi_from.as_mut()) {
                        route.listen = on;
                    }
                }
            }
        }
    }
    Ok(json!({"tracks": tracks}))
}

#[cfg(test)]
#[path = "tests_midi_routes.rs"]
mod tests;
