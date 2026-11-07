//! The device macro strip's pages (ADR 0047 decision 3.8): `mix.device_pages` replaces a device's
//! user pages in one undo step, `mix.device_pages_list` returns both tiers with each slot's state.
//! A device is addressed by an insert letter `a` to `j`, `instrument`, `midi1` to `midi8` (the MIDI
//! effect at that position plus one, when the command runs) or `rack`.

use super::*;
use crate::cmd;
use crate::device_pages::{self as pages, Table};
use serde_json::json;
use transit_model::{DeviceRef, MAX_USER_PAGES, Track, UserPage};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.device_pages",
            "Set Device Pages",
            [],
            None,
            "{track?, device: a..j | instrument | midi1..midi8, pages: [{name, slots: [param id | null; 8]}]} — replaces the device's user pages (at most 16; each page read by the session file's rules), one undo step",
            has_selection,
            set_pages
        ),
        cmd!(
            query "mix.device_pages_list",
            "List Device Pages",
            [],
            None,
            "{track?, device: a..j | instrument | midi1..midi8 | rack} → {track, device, pages: [{tier: factory|user, name, slots: [{id, name, state: ok|empty|missing|pending, enable?}; 8]}]}",
            has_selection,
            list_pages
        ),
    ]
}

fn track_device(e: &Engine, cmd: &str, p: &Value) -> Result<(TrackId, DeviceRef)> {
    let t = tracks_required(e, cmd, p)?.first().copied().ok_or_else(|| bad(cmd, "no track"))?;
    let text = str_param(p, "device").ok_or_else(|| bad(cmd, "`device` required: a..j, instrument, midi1..midi8 or rack"))?;
    let device = DeviceRef::parse(text).ok_or_else(|| bad(cmd, format!("`device` `{text}` is not a..j, instrument, midi1..midi8 or rack")))?;
    Ok((t, device))
}

/// The plug-in id at `device`, and whether it is a MIDI effect.
fn plugin_at(tr: &Track, device: DeviceRef) -> Option<(&str, bool)> {
    match device {
        DeviceRef::Insert(i) => tr.mixer.inserts.get(i).and_then(Option::as_ref).map(|x| (x.plugin.as_str(), false)),
        DeviceRef::Instrument => tr.instrument.as_ref().map(|x| (x.plugin.as_str(), false)),
        DeviceRef::MidiEffect(i) => tr.midi_effects.get(i).map(|x| (x.plugin.as_str(), true)),
        DeviceRef::Rack => None,
    }
}

/// What is known of the parameter table at `device`. A built-in is always known; a hosted
/// plug-in's table comes from the registries (read on the plug-in host thread when no instance
/// has been made yet, waited for at most `clap::INFO_WAIT`), and is pending while that read
/// carries on.
fn table_at(tr: &Track, device: DeviceRef) -> Table<'static> {
    if hardware(tr, device) {
        return Table::NoParameters;
    }
    match plugin_at(tr, device) {
        None => Table::NoParameters,
        Some((id, true)) => pages::classify(transit_dsp::midi_fx::info(id), false, false),
        Some((id, false)) if transit_mix::is_third_party(id) => pages::classify(super::clap::plugin_info(id), true, super::clap::loading(id)),
        Some((id, false)) => pages::classify(transit_dsp::plugin_info(id), false, false),
    }
}

/// A hardware insert or an External Instrument: a slot with no plug-in and so no parameters.
fn hardware(tr: &Track, device: DeviceRef) -> bool {
    match device {
        DeviceRef::Insert(i) => {
            tr.mixer.inserts.get(i).and_then(Option::as_ref).is_some_and(|x| x.hardware.is_some() || x.plugin == transit_model::HARDWARE_PLUGIN)
        }
        DeviceRef::Instrument => tr.instrument.as_ref().is_some_and(|x| x.plugin == transit_model::EXTERNAL_INSTRUMENT_PLUGIN),
        DeviceRef::MidiEffect(_) | DeviceRef::Rack => false,
    }
}

/// Whether the device at `device` has no parameters to put on a page.
fn no_parameters(tr: &Track, device: DeviceRef) -> bool {
    let builtin = match plugin_at(tr, device) {
        Some((id, true)) => transit_dsp::midi_fx::info(id),
        Some((id, false)) => transit_dsp::plugin_info(id),
        None => None,
    };
    hardware(tr, device) || builtin.is_some_and(|i| i.params.is_empty())
}

fn set_pages(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.device_pages";
    let (t, device) = track_device(e, CMD, p)?;
    let raw = p.get("pages").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`pages` must be an array of pages ([] clears them)"))?;
    if raw.len() > MAX_USER_PAGES {
        return Err(bad(CMD, format!("{} pages: a device keeps at most {MAX_USER_PAGES}", raw.len())));
    }
    let new: Vec<UserPage> = raw.iter().map(UserPage::read).collect();
    let tr = e.session().track(t).ok_or_else(|| bad(CMD, "no track"))?;
    if device == DeviceRef::Rack {
        return Err(bad(
            CMD,
            if tr.is_rack() { "a rack card's strip is its eight macros and takes no user pages" } else { "the track is not a rack" },
        ));
    }
    let Some((plugin, _)) = plugin_at(tr, device) else {
        return Err(bad(CMD, format!("no device at `{device}` on `{}`", tr.name)));
    };
    // A hardware insert, an External Instrument or a built-in with no parameters (Invert) has
    // nothing to put on a page. A hosted table is not looked up: user pages may name ids it does
    // not list yet (they read as missing or pending).
    if no_parameters(tr, device) {
        return Err(bad(CMD, format!("`{plugin}` at `{device}` has no parameters to put on a page")));
    }
    if tr.device_pages(device).is_some_and(|old| old == new.as_slice()) {
        return Ok(json!({"track": t, "device": device.to_string(), "pages": new.len(), "changed": false}));
    }
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(CMD, "no track"))?;
    tr.set_device_pages(device, &new).map_err(|why| bad(CMD, why.to_string()))?;
    Ok(json!({"track": t, "device": device.to_string(), "pages": new.len(), "changed": true}))
}

fn list_pages(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.device_pages_list";
    let (t, device) = track_device(e, CMD, p)?;
    let tr = e.session().track(t).ok_or_else(|| bad(CMD, "no track"))?;
    let resolved = if device == DeviceRef::Rack {
        let rack = tr.rack.as_ref().ok_or_else(|| bad(CMD, format!("`{}` is not a rack", tr.name)))?;
        vec![pages::rack_page(rack)]
    } else {
        let user = tr.device_pages(device).ok_or_else(|| bad(CMD, format!("no device at `{device}` on `{}`", tr.name)))?;
        pages::resolve(table_at(tr, device), user)
    };
    Ok(json!({"track": t, "device": device.to_string(), "pages": resolved}))
}

#[cfg(test)]
#[path = "device_pages_tests.rs"]
mod tests;
