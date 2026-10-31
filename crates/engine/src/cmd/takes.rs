//! Take stacks (#25, ADR 0039): list a track's stacked takes, choose the take that plays, flatten
//! and delete a take. Each change is one undo step; see [`crate::takes`] for what they do.

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::Session;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "takes.list", "List Takes", [], None, "{track} -> {stacks: [{index, start, end, active, takes: [{number, active, explicit, clips: [{clip, playlist}]}]}]}", has_selection, list),
        cmd!(
            "takes.set_active",
            "Choose Take",
            [],
            None,
            "{track, clip} (that clip's take plays over its range) | {track, take, stack?} (that take plays in that stack, or in every stack it has material in)",
            has_selection,
            set_active
        ),
        cmd!(
            "takes.flatten",
            "Flatten Takes",
            [],
            None,
            "{track?, tracks?, stack?} (keep what plays, remove the other takes; every stack when `stack` is left out)",
            has_selection,
            flatten
        ),
        cmd!("takes.delete", "Delete Take", [], None, "{track, take, stack?} (refused where that take plays)", has_selection, delete),
    ]
}

fn the_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    track_param(e, id, p, "track")?.ok_or_else(|| bad(id, "`track` required"))
}

fn u32_param(p: &Value, key: &str) -> Option<u32> {
    p.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())
}

fn index_param(p: &Value) -> Option<usize> {
    p.get("stack").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok())
}

/// Take commands change no frozen track at all, not even material that does not play: unfreeze
/// first (checked before any change).
fn refuse_frozen(e: &Engine, tracks: &[TrackId]) -> Result<()> {
    if tracks.iter().any(|t| e.session().track(*t).is_some_and(|tr| tr.freeze.is_some())) {
        return Err(EngineError::TrackFrozen);
    }
    Ok(())
}

/// Run `op` on a copy of the session and keep the copy only when it changed, so a take edit
/// that changes nothing leaves no undo step.
fn edit<T>(e: &mut Engine, id: &str, op: impl FnOnce(&mut Session) -> std::result::Result<T, String>) -> Result<T> {
    let mut work = e.session().clone();
    let out = op(&mut work).map_err(|m| bad(id, m))?;
    if work != *e.session() {
        *e.session_mut() = work;
    }
    Ok(out)
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = the_track(e, "takes.list", p)?;
    let tr = e.session().track(t).ok_or_else(|| bad("takes.list", "no track"))?;
    let stacks: Vec<Value> = tr
        .take_stacks()
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let takes: Vec<Value> = st
                .takes
                .iter()
                .map(|k| {
                    let clips: Vec<Value> = k.clips.iter().map(|(pl, c)| json!({"clip": c.0, "playlist": pl})).collect();
                    json!({"number": k.number, "active": k.is_active(), "explicit": k.explicit, "clips": clips})
                })
                .collect();
            json!({"index": i, "start": st.range.start, "end": st.range.end, "active": st.active(), "takes": takes})
        })
        .collect();
    Ok(json!({"track": t.0, "stacks": stacks}))
}

fn set_active(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "takes.set_active";
    let t = the_track(e, id, p)?;
    refuse_frozen(e, &[t])?;
    if let Some(clip) = p.get("clip").and_then(Value::as_u64).map(ClipId) {
        let tr = e.session().track(t).ok_or_else(|| bad(id, "no track"))?;
        let (_, stack) = tr.take_stack_of(clip).ok_or_else(|| bad(id, format!("clip {} is not in a take stack", clip.0)))?;
        let changed = edit(e, id, |s| {
            crate::takes::materialize(s, t, &stack);
            crate::takes::activate_clip(s, t, clip)
        })?;
        let number = e.session().track(t).and_then(|tr| tr.playlists.iter().find_map(|pl| pl.clip(clip))).and_then(|c| c.take);
        return Ok(json!({"moved": usize::from(changed), "take": number}));
    }
    let number = u32_param(p, "take").ok_or_else(|| bad(id, "give `clip`, or `take` (and `stack`)"))?;
    let stacks = e.session().track(t).map(transit_model::Track::take_stacks).unwrap_or_default();
    let which: Vec<usize> = match index_param(p) {
        Some(i) => vec![i],
        None => stacks.iter().enumerate().filter(|(_, st)| st.take(number).is_some()).map(|(i, _)| i).collect(),
    };
    if which.is_empty() {
        return Err(bad(id, format!("there is no take {number}")));
    }
    // Later stacks first: choosing a take can only change the stacks from the one it is in on,
    // so the earlier indices still name the same stacks.
    let moved = edit(e, id, |s| {
        let mut n = 0;
        for i in which.into_iter().rev() {
            n += crate::takes::set_active(s, t, i, number)?;
        }
        Ok(n)
    })?;
    Ok(json!({"moved": moved, "take": number}))
}

fn flatten(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "takes.flatten";
    let tracks = tracks_required(e, id, p)?;
    refuse_frozen(e, &tracks)?;
    let index = index_param(p);
    let removed = edit(e, id, |s| {
        let mut n = 0;
        let mut flattened = false;
        let mut last = None;
        for t in &tracks {
            match crate::takes::flatten(s, *t, index) {
                Ok(k) => {
                    n += k;
                    flattened = true;
                }
                Err(m) => last = Some(m),
            }
        }
        if flattened { Ok(n) } else { Err(last.unwrap_or_else(|| "nothing to flatten".into())) }
    })?;
    Ok(json!({"removed": removed}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "takes.delete";
    let t = the_track(e, id, p)?;
    refuse_frozen(e, &[t])?;
    let number = u32_param(p, "take").ok_or_else(|| bad(id, "`take` required"))?;
    let index = index_param(p);
    let removed = edit(e, id, |s| crate::takes::delete_take(s, t, number, index))?;
    Ok(json!({"removed": removed, "take": number}))
}
