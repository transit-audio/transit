//! Zoom Toggle: zoom in on the edit selection, then back out to the view before.

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::{TrackHeight, ZoomToggle};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!("view.zoom_toggle", "Zoom Toggle", [], None, "{width_px?: 1200, height_px?: 600}", always, toggle)]
}

fn toggle(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(back) = s.edit.zoom_toggle.take() {
        s.edit.zoom.samples_per_px = back.samples_per_px.clamp(0.25, 1_000_000.0);
        s.edit.zoom.scroll = back.scroll.clamp(0, MAX_POSITION);
        for t in &mut s.tracks {
            t.height = back.heights.iter().find(|(id, _)| *id == t.id).map_or(t.height, |(_, h)| *h);
        }
        return Ok(json!({"on": false}));
    }
    let heights: Vec<_> = s.tracks.iter().filter(|t| !t.hidden && s.edit.selected_tracks.contains(&t.id)).map(|t| (t.id, t.height)).collect();
    // Rows sit 1 point apart.
    let each = f64_or(p, "height_px", 600.0) / heights.len().max(1) as f64 - 1.0;
    let fit = TrackHeight::Fit(each.clamp(16.0, 440.0).round() as u16);
    for t in s.tracks.iter_mut().filter(|t| heights.iter().any(|(id, _)| *id == t.id)) {
        t.height = fit;
    }
    let (r, z) = (s.edit.selection, &mut s.edit.zoom);
    s.edit.zoom_toggle = Some(ZoomToggle { samples_per_px: z.samples_per_px, scroll: z.scroll, heights });
    if !r.is_empty() {
        z.samples_per_px = (r.len() as f64 / f64_or(p, "width_px", 1200.0).max(100.0)).clamp(0.25, 1_000_000.0);
        z.scroll = r.start;
    }
    Ok(json!({"on": true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrackHeight::{Fit, Small};

    fn engine(select: Value) -> Engine {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 3})).unwrap();
        e.execute("view.zoom_set", &json!({"samples_per_px": 500.0})).unwrap();
        e.execute("view.scroll", &json!({"to": 1000})).unwrap();
        e.execute("edit.select", &select).unwrap();
        e
    }

    fn view(e: &Engine) -> (f64, Samples, Vec<TrackHeight>) {
        let s = e.session();
        (s.edit.zoom.samples_per_px, s.edit.zoom.scroll, s.tracks.iter().map(|t| t.height).collect())
    }

    #[test]
    fn the_selection_fills_the_window_and_toggling_again_goes_back() {
        let mut e = engine(json!({"tracks": ["Audio 1", "Audio 3"], "start": 48_000, "end": 96_000}));
        let steps = e.undo_history().len();
        assert_eq!(e.execute("view.zoom_toggle", &json!({"width_px": 1000.0, "height_px": 500.0})).unwrap(), json!({"on": true}));
        assert_eq!(view(&e), (48.0, 48_000, vec![Fit(249), Small, Fit(249)]));
        e.execute("view.zoom_in", &json!({})).unwrap();
        assert_eq!(e.execute("view.zoom_toggle", &json!({})).unwrap(), json!({"on": false}));
        assert_eq!(view(&e), (500.0, 1000, vec![Small, Small, Small]));
        assert_eq!(e.undo_history().len(), steps + 2);
        e.execute("edit.select", &json!({"tracks": ["Audio 2"], "start": 5000})).unwrap();
        e.execute("view.zoom_toggle", &json!({"height_px": 300.0})).unwrap();
        assert_eq!(view(&e), (500.0, 1000, vec![Small, Fit(299), Small]));
    }

    #[test]
    fn hostile_sizes_and_stored_views_stay_in_range() {
        let mut e = engine(json!({"tracks": ["Audio 1"], "start": 0, "end": MAX_POSITION}));
        e.execute("view.zoom_toggle", &json!({"width_px": -5.0, "height_px": 1e300})).unwrap();
        assert_eq!((view(&e).0, view(&e).2[0]), (1_000_000.0, Fit(440)));
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        e.execute("view.zoom_toggle", &json!({"width_px": "wide", "height_px": -1.0})).unwrap();
        assert_eq!(view(&e).2[0], Fit(16));
        let audio_1 = e.session().tracks[0].id;
        let heights = vec![(TrackId(9999), TrackHeight::Jumbo), (audio_1, Fit(0))];
        e.session_mut().edit.zoom_toggle = Some(ZoomToggle { samples_per_px: -1.0, scroll: Samples::MIN, heights });
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        assert_eq!(view(&e), (0.25, 0, vec![Fit(0), Small, Small]));
        assert_eq!(Fit(0).points(), 16.0);
    }

    #[test]
    fn each_toggle_is_one_undo_step_restoring_heights_and_the_record() {
        let mut e = engine(json!({"tracks": ["Audio 1", "Audio 3"], "start":48000, "end":96000}));
        e.session_mut().tracks[2].hidden = true;
        let before = e.session_arc();
        let count = e.undo_history().len();
        e.execute("view.zoom_toggle", &json!({"height_px":1000})).unwrap();
        assert_eq!(e.undo_history().len(), count + 1);
        assert_eq!(view(&e).2, [Fit(440), Small, Small]);
        let fitted = e.session_arc();
        assert!(e.undo());
        assert_eq!(e.session(), &*before);
        assert!(e.redo());
        assert_eq!(e.session(), &*fitted);
        assert_eq!(e.execute("view.zoom_toggle", &json!({})).unwrap(), json!({"on":false}));
        assert_eq!(view(&e).2, [Small; 3]);
        assert!(e.session().edit.zoom_toggle.is_none());
        assert!(e.undo());
        assert_eq!(e.session(), &*fitted);
    }

    #[test]
    fn explicit_changes_invalidate_the_whole_record_but_no_ops_do_not() {
        for (cmd, params) in [
            ("track.height", json!({"tracks":[2],"height":57})),
            ("track.height_up", json!({"tracks":[2]})),
            ("track.height_down", json!({"tracks":[2]})),
            ("track.height_zoom", json!({"direction":"in"})),
            ("track.height_reset", json!({})),
        ] {
            let mut e = engine(json!({"tracks":["Audio 1", "Audio 3"],"start":48000,"end":96000}));
            e.execute("view.zoom_toggle", &json!({"height_px":10000})).unwrap();
            let before = e.session_arc();
            let count = e.undo_history().len();
            e.execute("track.height", &json!({"tracks":[1,3],"height":1e300})).unwrap();
            assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
            assert_eq!(e.execute("track.height_up", &json!({"tracks":[1,3]})).unwrap(), json!({"changed":0}));
            assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
            e.execute(cmd, &params).unwrap();
            assert_eq!(e.undo_history().len(), count + 1);
            assert!(e.session().edit.zoom_toggle.is_none(), "{cmd}");
            assert_eq!(e.session().edit.zoom, before.edit.zoom);
            assert!(e.undo());
            assert_eq!(e.session(), &*before);
            assert_eq!(e.execute("view.zoom_toggle", &json!({})).unwrap(), json!({"on":false}));
            assert_eq!(view(&e), (500.0, 1000, vec![Small; 3]));
        }
    }

    #[test]
    fn a_toggle_after_an_explicit_change_starts_fresh() {
        let mut e = engine(json!({"tracks":["Audio 1"]}));
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        e.execute("track.height", &json!({"height":57})).unwrap();
        assert_eq!(e.execute("view.zoom_toggle", &json!({})).unwrap(), json!({"on":true}));
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        assert_eq!(view(&e).2[0], TrackHeight::Custom(57));
    }

    #[test]
    fn toggling_off_restores_only_surviving_ids() {
        let mut e = engine(json!({"tracks":["Audio 1", "Audio 3"]}));
        e.session_mut().tracks[2].height = TrackHeight::Custom(57);
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        e.execute("track.delete", &json!({"tracks":[1]})).unwrap();
        assert_eq!(e.execute("view.zoom_toggle", &json!({})).unwrap(), json!({"on":false}));
        assert_eq!(e.session().tracks.iter().map(|t| t.id.0).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(view(&e).2, [Small, TrackHeight::Custom(57)]);
    }
}
