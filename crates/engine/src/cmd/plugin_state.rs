//! Third-party plugin state in the session (`mix.insert_state`): the UI reads each live plugin's
//! state right before saving and stores it on its insert (base64), from where the mixer restores
//! it into new instances.

use super::*;
use serde_json::json;
use transit_model::{INSERT_SLOTS, b64};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "mix.insert_state",
        label: "Store Plugin State",
        menu: &[],
        shortcut: None,
        params: "{track, slot: 0..9 | \"a\"..\"j\" | \"instrument\", state: base64 | null} → {bytes}. Not undoable and not journaled (states can be large).",
        enabled: has_tracks,
        run: insert_state,
        journal: false,
        undoable: false,
        view: false,
    }]
}

const CMD: &str = "mix.insert_state";

/// `None` = the instrument slot.
fn slot_param(p: &Value) -> Result<Option<usize>> {
    slot_param_for(CMD, p)
}

/// [`slot_param`] for command `cmd`.
pub(super) fn slot_param_for(cmd: &str, p: &Value) -> Result<Option<usize>> {
    let v = p.get("slot").ok_or_else(|| bad(cmd, "`slot` required"))?;
    if let Some(n) = v.as_u64() {
        return usize::try_from(n).ok().filter(|&n| n < INSERT_SLOTS).map(Some).ok_or_else(|| bad(cmd, "slot out of range"));
    }
    let s = v.as_str().map(|s| s.trim().to_ascii_lowercase()).ok_or_else(|| bad(cmd, "`slot` must be a number or a letter"))?;
    if s == "instrument" {
        return Ok(None);
    }
    match s.as_bytes() {
        [c @ b'a'..=b'z'] if usize::from(c - b'a') < INSERT_SLOTS => Ok(Some(usize::from(c - b'a'))),
        _ => s.parse::<usize>().ok().filter(|&n| n < INSERT_SLOTS).map(Some).ok_or_else(|| bad(cmd, "slot out of range")),
    }
}

fn insert_state(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, CMD, p, "track")?.ok_or_else(|| bad(CMD, "`track` required"))?;
    let slot = slot_param(p)?;
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(b64::decode(s).ok_or_else(|| bad(CMD, "`state` is not valid base64 (or is too large)"))?),
        Some(_) => return Err(bad(CMD, "`state` must be a base64 string or null")),
    };
    let s = e.session_mut();
    let tr = s.track_mut(t).ok_or_else(|| bad(CMD, "no track"))?;
    let ins = match slot {
        Some(i) => tr.mixer.inserts.get_mut(i).and_then(Option::as_mut),
        None => tr.instrument.as_mut(),
    }
    .ok_or_else(|| bad(CMD, "empty slot"))?;
    let n = state.as_ref().map_or(0, Vec::len);
    if ins.plugin == "texture" {
        ins.frozen_recording = match state.filter(|data| !data.is_empty()) {
            Some(data) => {
                let Some(r) = transit_dsp::plugins::texture::freeze_state::Recording::parse(&data) else {
                    let report = format!(
                        "track {:?}, Texture insert {}: invalid frozen capture; previous recording retained",
                        tr.name,
                        slot.map_or_else(|| "instrument".to_string(), |i| (i + 1).to_string())
                    );
                    // With no earlier reference there is nothing to annotate: no empty entry, no false "opens silent" line later.
                    if let Some(saved) = ins.frozen_recording.as_mut() {
                        saved.report.clone_from(&report);
                    }
                    log::warn!("{report}");
                    e.message(report.clone());
                    return Ok(json!({"bytes": 0, "report": report}));
                };
                // A silent device created after a failed restore is not replacement media.
                // Only a device that has actually recorded since reset may replace it.
                if !r.has_recorded() && ins.frozen_recording.as_ref().is_some_and(|saved| saved.restore_failed) {
                    return Ok(json!({"bytes": 0, "retained_missing_recording": true}));
                }
                let mut saved = ins.frozen_recording.clone().unwrap_or_else(|| transit_model::FrozenRecording {
                    quality: r.quality,
                    rate: r.rate,
                    channels: r.channels,
                    span: r.span,
                    buffer_frames: r.buffer_frames,
                    header: r.wav_header(),
                    ..Default::default()
                });
                // Keep the previous on-disk setup and reference until the new media is
                // successfully written. The runtime packet owns its captured setup.
                saved.data = Some(data.into());
                saved.restore_failed = false;
                Some(saved)
            }
            None => None,
        };
        ins.state = None;
        return Ok(json!({"bytes": n}));
    }
    match state {
        Some(bytes) => ins.set_state_bytes(&bytes),
        None => ins.state = None,
    }
    Ok(json!({"bytes": n}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use transit_model::b64;

    #[test]
    fn stores_and_clears_state_without_undo() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "slot": 2})).unwrap();
        let undo_before = e.can_undo();
        let blob = b64::encode(b"\x00\x01plugin state\xff");
        let r = e.execute("mix.insert_state", &json!({"track": t, "slot": "c", "state": blob})).unwrap();
        assert_eq!(r["bytes"], 15);
        let ins = e.session().tracks[0].mixer.inserts[2].clone().unwrap();
        assert_eq!(ins.state_bytes().unwrap(), b"\x00\x01plugin state\xff");
        assert_eq!(e.can_undo(), undo_before, "not undoable");
        assert!(e.journal.iter().all(|(id, _)| id != "mix.insert_state"), "not journaled");
        // Survives save/load.
        let dir = transit_test_support::TestDir::new("insert-state");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.transit");
        e.execute("session.save", &json!({"path": path.to_string_lossy()})).unwrap();
        let mut f = Engine::default();
        f.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(f.session().tracks[0].mixer.inserts[2].as_ref().unwrap().state, ins.state);
        let _ = std::fs::remove_dir_all(&dir);
        e.execute("mix.insert_state", &json!({"track": t, "slot": 2, "state": null})).unwrap();
        assert_eq!(e.session().tracks[0].mixer.inserts[2].as_ref().unwrap().state, None);
    }

    #[test]
    fn copy_after_a_state_capture_carries_the_new_state() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "slot": 0})).unwrap();
        let blob = b64::encode(b"live editor state");
        e.execute("mix.insert_state", &json!({"track": t, "slot": 0, "state": blob})).unwrap();
        e.execute("mix.insert_copy", &json!({"track": t, "from": 0, "to": 1})).unwrap();
        let inserts = &e.session().tracks[0].mixer.inserts;
        assert_eq!(inserts[1].as_ref().unwrap().state_bytes().unwrap(), b"live editor state");
    }

    #[test]
    fn hostile_params_are_errors() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "slot": 0})).unwrap();
        for p in [
            json!({"track": t, "slot": 0, "state": "!!!"}),
            json!({"track": t, "slot": 0, "state": 5}),
            json!({"track": t, "slot": 1, "state": "AAAA"}),
            json!({"track": t, "slot": 99, "state": "AAAA"}),
            json!({"track": t, "slot": "instrument", "state": "AAAA"}),
            json!({"track": t, "slot": -1}),
            json!({"track": t}),
            json!({"track": 999, "slot": 0}),
            json!({"slot": 0}),
        ] {
            assert!(e.execute("mix.insert_state", &p).is_err(), "{p}");
        }
    }
}
