//! The MIDI effect slot of a track (#201, ADR 0036): built-in MIDI effects ahead of the
//! instrument, in processing order, each bypassable (Ableton's MIDI effects before the
//! instrument, Logic's MIDI FX above it). Every edit is one undo step. The mix ends the notes the
//! chain started when an effect is added, removed, moved or bypassed (`transit_mix` `midi_fx`).

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::{MAX_MIDI_EFFECTS, MidiEffectSlot, Track};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "engine.midi_effects", "List MIDI Effects", [], None, "{}", always, |_, _| {
            Ok(Value::Array(
                transit_dsp::midi_fx::REGISTRY
                    .iter()
                    .map(|i| {
                        json!({
                            "id": i.id,
                            "name": i.name,
                            "params": i.params.iter().map(|p| json!({"id": p.id, "name": p.name, "min": p.min, "max": p.max, "default": p.default})).collect::<Vec<_>>(),
                        })
                    })
                    .collect(),
            ))
        }),
        cmd!(
            "mix.midi_effect",
            "Add MIDI Effect",
            [],
            None,
            "{track?, plugin: id (engine.midi_effects), at?: index (default: the end), params?: {id: value}} — an instrument track or a rack, at most 8",
            has_selection,
            add
        ),
        cmd!("mix.midi_effect_remove", "Remove MIDI Effect", [], None, "{track?, index}", has_selection, remove),
        cmd!("mix.midi_effect_move", "Move MIDI Effect", [], None, "{track?, from, to}", has_selection, move_fx),
        cmd!("mix.midi_effect_bypass", "Bypass MIDI Effect", [], None, "{track?, index, value?: bool (default: toggle)}", has_selection, bypass),
        cmd!(
            "mix.midi_effect_param",
            "Set MIDI Effect Parameter",
            [],
            None,
            "{track?, index, param, value} (clamped into range)",
            has_selection,
            param
        ),
        cmd!(
            "mix.midi_effect_params",
            "Set MIDI Effect Parameters",
            [],
            None,
            "{track?, index, params: {id: value}} (each clamped into range; one undo step, as a preset recall)",
            has_selection,
            params
        ),
    ]
}

fn track_of<'a>(e: &'a mut Engine, cmd: &str, p: &Value) -> Result<&'a mut Track> {
    let t = tracks_required(e, cmd, p)?.first().copied().ok_or_else(|| bad(cmd, "no track"))?;
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(cmd, "no track"))?;
    // The chain feeds the instrument: a MIDI track's external output (#61) does not go through it.
    // A rack's chain feeds every member (ADR 0036).
    if tr.kind != transit_model::TrackKind::Instrument && !tr.is_rack() {
        return Err(bad(cmd, "MIDI effects go on an instrument track or a rack"));
    }
    Ok(tr)
}

fn index_of(p: &Value, key: &str, cmd: &str, len: usize) -> Result<usize> {
    let i = p.get(key).and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("`{key}` must be an index")))?;
    usize::try_from(i).ok().filter(|i| *i < len).ok_or_else(|| bad(cmd, format!("`{key}` {i} is not a MIDI effect of the track ({len})")))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.midi_effect";
    let plugin = str_param(p, "plugin").ok_or_else(|| bad(CMD, "`plugin` required"))?;
    if transit_mix::is_third_party(plugin) {
        return add_hosted(e, p, plugin.to_string());
    }
    let info = transit_dsp::midi_fx::info(plugin).ok_or_else(|| {
        let ids: Vec<&str> = transit_dsp::midi_fx::REGISTRY.iter().map(|i| i.id).collect();
        bad(CMD, format!("`{plugin}` is not a MIDI effect (built-ins: {})", ids.join(", ")))
    })?;
    let mut fx = MidiEffectSlot::new(info.id);
    for pi in info.params {
        fx.params.insert(pi.id.to_string(), pi.default);
    }
    if let Some(obj) = p.get("params").and_then(Value::as_object) {
        for (k, v) in obj {
            let pi = info.param(k).ok_or_else(|| bad(CMD, format!("`{}` has no parameter `{k}`", info.id)))?;
            let v = v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| bad(CMD, format!("`{k}` needs a number")))?;
            fx.params.insert(k.clone(), pi.clamp(v as f32));
        }
    }
    let at = p.get("at").and_then(Value::as_u64).map(|i| usize::try_from(i).unwrap_or(usize::MAX));
    let tr = track_of(e, CMD, p)?;
    let index = tr.add_midi_effect(fx, at).ok_or_else(|| bad(CMD, format!("the track has {MAX_MIDI_EFFECTS} MIDI effects, the most it holds")))?;
    Ok(json!({"track": tr.id, "index": index, "plugin": info.id}))
}

/// A hosted MIDI effect (ADR 0064, 2.6 and decision 12): accepted only when the scan's probe found
/// a note output transit reads (`note_output == Some(true)` in its cached descriptor), never on a
/// rack (10.2). Until the visibility slice this command, through the control channel and headless
/// scripts, is the only way in: no menu or listing offers a hosted id (`engine.midi_effects` lists
/// the built-ins only). Its parameters and state are a later slice's (HM3).
fn add_hosted(e: &mut Engine, p: &Value, plugin: String) -> Result<Value> {
    const CMD: &str = "mix.midi_effect";
    if !hosted_qualifies(&plugin) {
        return Err(bad(CMD, format!("`{plugin}` has no note output transit reads (or the plug-in scan has not probed it)")));
    }
    if p.get("params").is_some() {
        return Err(bad(CMD, "a hosted MIDI effect takes no `params` here"));
    }
    let at = p.get("at").and_then(Value::as_u64).map(|i| usize::try_from(i).unwrap_or(usize::MAX));
    let tr = track_of(e, CMD, p)?;
    if tr.is_rack() {
        return Err(bad(CMD, "Hosted MIDI effects go on an instrument track until racks run one chain for all members"));
    }
    let index = tr
        .add_midi_effect(MidiEffectSlot::new(plugin.clone()), at)
        .ok_or_else(|| bad(CMD, format!("the track has {MAX_MIDI_EFFECTS} MIDI effects, the most it holds")))?;
    Ok(json!({"track": tr.id, "index": index, "plugin": plugin}))
}

/// Whether the scan's probe found a readable note output for the hosted id `id` (2.6).
fn hosted_qualifies(id: &str) -> bool {
    #[cfg(test)]
    {
        tests::QUALIFIED.with(|q| q.borrow().iter().any(|x| x == id))
    }
    #[cfg(not(test))]
    {
        qualifies(id, transit_vst3_host::cached().as_deref(), transit_clap_host::cached().as_deref())
    }
}

/// [`hosted_qualifies`] over the scan's descriptors: the class with that id has a probe outcome
/// that answered with a note output. A class whose probe failed, or that no scan listed, does not.
fn qualifies(id: &str, vst3: Option<&[transit_vst3_host::Vst3Descriptor]>, clap: Option<&[transit_clap_host::ClapDescriptor]>) -> bool {
    let yes = |o: &transit_clap_host::ProbeOutcome| o.note_output == Some(true);
    vst3.is_some_and(|all| all.iter().any(|d| d.id == id && yes(&d.probe))) || clap.is_some_and(|all| all.iter().any(|d| d.id == id && yes(&d.probe)))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.midi_effect_remove";
    let tr = track_of(e, CMD, p)?;
    let i = index_of(p, "index", CMD, tr.midi_effects.len())?;
    let fx = tr.remove_midi_effect(i).ok_or_else(|| bad(CMD, "no such MIDI effect"))?;
    Ok(json!({"track": tr.id, "removed": fx.plugin}))
}

fn move_fx(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.midi_effect_move";
    let tr = track_of(e, CMD, p)?;
    let n = tr.midi_effects.len();
    let (from, to) = (index_of(p, "from", CMD, n)?, index_of(p, "to", CMD, n)?);
    if !tr.move_midi_effect(from, to) {
        return Err(bad(CMD, "`from` and `to` are the same place"));
    }
    Ok(json!({"track": tr.id, "from": from, "to": to}))
}

fn bypass(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.midi_effect_bypass";
    let value = p.get("value").and_then(Value::as_bool);
    let tr = track_of(e, CMD, p)?;
    let i = index_of(p, "index", CMD, tr.midi_effects.len())?;
    let fx = tr.midi_effects.get_mut(i).ok_or_else(|| bad(CMD, "no such MIDI effect"))?;
    fx.bypass = value.unwrap_or(!fx.bypass);
    Ok(json!({"bypass": fx.bypass}))
}

fn param(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.midi_effect_param";
    let id = str_param(p, "param").ok_or_else(|| bad(CMD, "`param` required"))?.to_string();
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(CMD, "`value` must be a number"))? as f32;
    let tr = track_of(e, CMD, p)?;
    let i = index_of(p, "index", CMD, tr.midi_effects.len())?;
    let fx = tr.midi_effects.get_mut(i).ok_or_else(|| bad(CMD, "no such MIDI effect"))?;
    let info = transit_dsp::midi_fx::info(&fx.plugin).ok_or_else(|| bad(CMD, format!("`{}` is not a MIDI effect this build knows", fx.plugin)))?;
    let pi = info.param(&id).ok_or_else(|| bad(CMD, format!("`{}` has no parameter `{id}`", info.id)))?;
    let v = pi.clamp(value);
    fx.params.insert(id, v);
    Ok(json!({"value": v}))
}

fn params(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "mix.midi_effect_params";
    let obj = p.get("params").and_then(Value::as_object).ok_or_else(|| bad(CMD, "`params` must be an object"))?.clone();
    let tr = track_of(e, CMD, p)?;
    let i = index_of(p, "index", CMD, tr.midi_effects.len())?;
    let fx = tr.midi_effects.get_mut(i).ok_or_else(|| bad(CMD, "no such MIDI effect"))?;
    let info = transit_dsp::midi_fx::info(&fx.plugin).ok_or_else(|| bad(CMD, format!("`{}` is not a MIDI effect this build knows", fx.plugin)))?;
    // Check every value before writing any, so a bad one changes nothing.
    let mut values = Vec::with_capacity(obj.len());
    for (k, v) in &obj {
        let pi = info.param(k).ok_or_else(|| bad(CMD, format!("`{}` has no parameter `{k}`", info.id)))?;
        let v = v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| bad(CMD, format!("`{k}` needs a number")))?;
        values.push((k.clone(), pi.clamp(v as f32)));
    }
    for (k, v) in &values {
        fx.params.insert(k.clone(), *v);
    }
    Ok(json!({"set": values.len()}))
}

#[cfg(test)]
#[path = "midi_effect_tests.rs"]
mod tests;
