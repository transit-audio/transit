//! Retry of a plug-in slot that reads "Not responding" (issue #82): its creation ran past the
//! plug-in host's creation budget (ADR 0018). The engine checks the request; the UI's player
//! makes the plug-in again on a responsive host thread with the state the session stores. Not a
//! document change, so no undo step.

use super::plugin_state::slot_param_for;
use super::*;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "plugin.retry",
            label: "Retry Plug-in",
            menu: &[],
            shortcut: None,
            params: "{track, slot: 0..9 | \"a\"..\"j\" | \"instrument\"} → {track, slot, plugin}. Makes the plug-in of a slot that reads \"Not responding\" again, on a responsive plug-in host thread, restoring the state the session stores for it. Not a document change (no undo step); refused by the UI when the slot is not waiting for a retry.",
            enabled: has_tracks,
            run: retry,
            journal: true,
            undoable: false,
            view: false,
        },
        CommandSpec {
            id: "plugin.retry_all",
            label: "Retry All Plug-ins",
            menu: &[],
            shortcut: None,
            params: "{} → {retried}. plugin.retry for every slot that reads \"Not responding\". Not a document change (no undo step).",
            enabled: always,
            run: retry_all,
            journal: true,
            undoable: false,
            view: false,
        },
    ]
}

/// Checks that the slot holds a plug-in; returns where it is and which (the slot as a number,
/// the instrument as `transit_mix::INSTRUMENT_SLOT`).
fn retry(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "plugin.retry";
    let t = track_param(e, CMD, p, "track")?.ok_or_else(|| bad(CMD, "`track` required"))?;
    let slot = slot_param_for(CMD, p)?;
    let tr = e.session().track(t).ok_or_else(|| bad(CMD, "no track"))?;
    let ins = match slot {
        Some(i) => tr.mixer.inserts.get(i).and_then(Option::as_ref),
        None => tr.instrument.as_ref(),
    }
    .ok_or_else(|| bad(CMD, "empty slot"))?;
    Ok(json!({"track": t.0, "slot": slot.unwrap_or(transit_mix::INSTRUMENT_SLOT), "plugin": ins.plugin}))
}

fn retry_all(_: &mut Engine, _: &Value) -> Result<Value> {
    Ok(json!({"retried": 0}))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn a_retry_names_the_slot_and_changes_nothing() {
        let mut e = Engine::default();
        let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = r["tracks"][0].as_u64().unwrap();
        e.execute("mix.instrument", &json!({"track": t, "plugin": "macro_oscillator"})).unwrap();
        let (doc, dirty) = (e.session_arc(), e.is_dirty());
        let undo = e.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len();
        let v = e.execute("plugin.retry", &json!({"track": t, "slot": "instrument"})).unwrap();
        assert_eq!(v, json!({"track": t, "slot": transit_mix::INSTRUMENT_SLOT, "plugin": "macro_oscillator"}));
        assert!(std::sync::Arc::ptr_eq(&doc, &e.session_arc()) && e.is_dirty() == dirty, "not a document change");
        assert_eq!(e.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len(), undo, "no undo step");
        assert!(e.execute("plugin.retry", &json!({"track": t, "slot": 0})).unwrap_err().to_string().contains("empty slot"));
        assert!(e.execute("plugin.retry", &json!({"track": t})).is_err());
        assert_eq!(e.execute("plugin.retry_all", &json!({})).unwrap(), json!({"retried": 0}));
    }
}
