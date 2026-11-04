//! Track height commands.

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
    ]
}

fn height(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.height", p)?;
    let h = match p.get("height").and_then(Value::as_f64) {
        Some(points) => TrackHeight::from_points(points as f32),
        None => str_param(p, "height").and_then(TrackHeight::from_id).ok_or_else(|| bad("track.height", "unknown height"))?,
    };
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.height = h;
        }
    }
    Ok(json!({"height": h}))
}

fn height_step(e: &mut Engine, p: &Value, taller: bool) -> Result<Value> {
    let tracks = tracks_required(e, if taller { "track.height_up" } else { "track.height_down" }, p)?;
    let mut steps = Vec::new();
    for tr in tracks.iter().filter_map(|t| e.session().track(*t)) {
        let h = tr.height.step(taller);
        if h != tr.height {
            steps.push((tr.id, h));
        }
    }
    if !steps.is_empty() {
        let s = e.session_mut();
        for (t, h) in &steps {
            if let Some(tr) = s.track_mut(*t) {
                tr.height = *h;
            }
        }
    }
    Ok(json!({"changed": steps.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_take_points_and_step_through_the_presets() {
        let mut e = crate::demo::demo_engine();
        let height = |e: &Engine, name: &str| e.session().track_by_name(name).map(|t| t.height);
        e.execute("track.height", &json!({"tracks": ["Kick"], "height": 57})).unwrap();
        assert_eq!(height(&e, "Kick"), Some(TrackHeight::Custom(57)));
        e.execute("track.height_up", &json!({"tracks": ["Kick", "Snare"]})).unwrap();
        assert_eq!((height(&e, "Kick"), height(&e, "Snare")), (Some(TrackHeight::Medium), Some(TrackHeight::Medium)));
        e.execute("track.height", &json!({"tracks": ["Kick"], "height": "extreme"})).unwrap();
        let steps = e.undo_history().len();
        assert_eq!(e.execute("track.height_up", &json!({"tracks": ["Kick"]})).unwrap()["changed"], 0);
        assert_eq!(e.undo_history().len(), steps);
        e.execute("track.height_down", &json!({"tracks": ["Kick"]})).unwrap();
        assert_eq!(height(&e, "Kick"), Some(TrackHeight::Jumbo));
    }
}
