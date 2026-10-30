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
