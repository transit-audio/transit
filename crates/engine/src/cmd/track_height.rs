//! Track height commands (ADR 0044).

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::TrackHeight;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.height",
            "Track Height",
            [],
            None,
            "{tracks?, height: micro|mini|small|medium|large|jumbo|extreme | points}",
            has_selection,
            height
        ),
        cmd!("track.height_up", "Increase Track Height", [], Some("Ctrl+Up"), "{tracks?} — one preset taller", has_selection, |e, p| height_step(
            e, p, true
        )),
        cmd!("track.height_down", "Decrease Track Height", [], Some("Ctrl+Down"), "{tracks?} — one preset shorter", has_selection, |e, p| {
            height_step(e, p, false)
        }),
        cmd!("track.height_zoom", "Zoom All Track Heights", [], None, "{direction: in|out}", always, height_zoom),
        cmd!("track.height_reset", "Reset Track Heights", [], None, "{}", always, |e, _| {
            let changes = e.session().tracks.iter().filter(|t| t.height != TrackHeight::Small).map(|t| (t.id, TrackHeight::Small)).collect();
            Ok(json!({"changed": apply(e, changes)}))
        }),
        cmd!(query "query.track_heights", "Track Heights", [], None, "{}", always, |e, _| {
            let tracks: Vec<_> = e.session().tracks.iter().map(|t| json!({"id": t.id, "height": t.height, "points": t.height.points() as u16})).collect();
            Ok(json!({"tracks": tracks}))
        }),
    ]
}

fn height(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = targets(e, "track.height", p)?;
    let (h, numeric) = match p.get("height") {
        Some(Value::Number(n)) => {
            let points = n.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad("track.height", "height must be finite"))?;
            // Round before narrowing: an f64 just below a half point must not round up as f32.
            (TrackHeight::from_points(points.clamp(16.0, 440.0).round() as f32), true)
        }
        Some(Value::String(name)) => (TrackHeight::from_id(name).ok_or_else(|| bad("track.height", "unknown height"))?, false),
        _ => return Err(bad("track.height", "height must be a finite number or preset name")),
    };
    let changes = tracks
        .iter()
        .filter_map(|id| e.session().track(*id))
        .filter(|t| if numeric { t.height.points() != h.points() } else { t.height != h })
        .map(|t| (t.id, h))
        .collect();
    apply(e, changes);
    Ok(json!({"height": h}))
}

fn height_step(e: &mut Engine, p: &Value, taller: bool) -> Result<Value> {
    let tracks = targets(e, if taller { "track.height_up" } else { "track.height_down" }, p)?;
    let changes = steps(e.session().tracks.iter().filter(|t| tracks.contains(&t.id)), taller);
    Ok(json!({"changed": apply(e, changes)}))
}

fn height_zoom(e: &mut Engine, p: &Value) -> Result<Value> {
    let taller = match str_param(p, "direction") {
        Some("in") => true,
        Some("out") => false,
        _ => return Err(bad("track.height_zoom", "direction must be `in` or `out`")),
    };
    let changes = steps(e.session().tracks.iter(), taller);
    Ok(json!({"changed": apply(e, changes)}))
}

fn targets(e: &Engine, cmd: &str, p: &Value) -> Result<Vec<TrackId>> {
    if p.get("tracks").is_some_and(|v| !v.is_array()) {
        return Err(bad(cmd, "tracks must be an array of track ids or names"));
    }
    let mut tracks = tracks_required(e, cmd, p)?;
    tracks.sort_unstable_by_key(|id| id.0);
    tracks.dedup();
    for id in &tracks {
        if e.session().track(*id).is_none() {
            return Err(bad(cmd, format!("no track with id {}", id.0)));
        }
    }
    Ok(tracks)
}

fn steps<'a>(tracks: impl Iterator<Item = &'a transit_model::Track>, taller: bool) -> Vec<(TrackId, TrackHeight)> {
    tracks
        .filter_map(|t| {
            let h = t.height.step(taller);
            (h != t.height).then_some((t.id, h))
        })
        .collect()
}

/// One borrow and one undo snapshot, including the entire pending toggle restore record.
/// A no-op must leave the document Arc and an active merge group untouched.
fn apply(e: &mut Engine, changes: Vec<(TrackId, TrackHeight)>) -> usize {
    if !changes.is_empty() {
        let s = e.session_mut();
        s.edit.zoom_toggle = None;
        for (id, h) in &changes {
            if let Some(t) = s.track_mut(*id) {
                t.height = *h;
            }
        }
    }
    changes.len()
}

#[cfg(test)]
#[path = "track_height_tests.rs"]
mod tests;
