use super::*;
use TrackHeight::*;
use std::sync::Arc;
use transit_model::{ChannelFormat, Session, TrackKind};

fn engine(heights: &[TrackHeight]) -> Engine {
    let mut s = Session::default();
    for (i, h) in heights.iter().enumerate() {
        let id = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("Track {i}")));
        s.track_mut(id).unwrap().height = *h;
        s.edit.selected_tracks.push(id);
    }
    Engine::new(s)
}

fn heights(e: &Engine) -> Vec<TrackHeight> {
    e.session().tracks.iter().map(|t| t.height).collect()
}

#[test]
fn heights_take_points_and_step_through_the_presets() {
    let mut e = crate::demo::demo_engine();
    let height = |e: &Engine, name: &str| e.session().track_by_name(name).map(|t| t.height);
    e.execute("track.height", &json!({"tracks": ["Kick"], "height": 57})).unwrap();
    assert_eq!(height(&e, "Kick"), Some(Custom(57)));
    e.execute("track.height_up", &json!({"tracks": ["Kick", "Snare"]})).unwrap();
    assert_eq!((height(&e, "Kick"), height(&e, "Snare")), (Some(Medium), Some(Medium)));
    e.execute("track.height", &json!({"tracks": ["Kick"], "height": "extreme"})).unwrap();
    let steps = e.undo_history().len();
    assert_eq!(e.execute("track.height_up", &json!({"tracks": ["Kick"]})).unwrap()["changed"], 0);
    assert_eq!(e.undo_history().len(), steps);
    e.execute("track.height_down", &json!({"tracks": ["Kick"]})).unwrap();
    assert_eq!(height(&e, "Kick"), Some(Jumbo));
}

#[test]
fn refusal_validates_every_target_and_value_before_borrowing() {
    let mut e = engine(&[Small, Medium]);
    let before = e.session_arc();
    let rev = e.revision;
    for params in [
        json!({"tracks":[1,9999],"height":57}),
        json!({"tracks":"bad","height":57}),
        json!({"tracks":[1,2],"height":true}),
        json!({"height":"57"}),
        json!({"height":"NaN"}),
        json!({"height":null}),
        json!({"height":f64::INFINITY}),
        json!({"height":f64::NAN}),
        json!({}),
    ] {
        assert!(e.execute("track.height", &params).is_err(), "{params}");
        assert!(Arc::ptr_eq(&before, &e.session_arc()), "{params}");
        assert_eq!(e.revision, rev);
        assert!(!e.is_dirty());
        assert!(e.undo_history().is_empty());
    }
}

#[test]
fn numeric_and_named_requests_normalize_and_count_one_edit() {
    let mut e = engine(&[Small]);
    for (value, expected) in [
        (json!(56.499999999), Custom(56)),
        (json!(56.5), Custom(57)),
        (json!(45.5), Small),
        (json!(1e300), Extreme),
        (json!(-1e300), Micro),
        (json!("mEdIuM"), Medium),
    ] {
        let count = e.undo_history().len();
        assert_eq!(e.execute("track.height", &json!({"height":value})).unwrap(), json!({"height":expected}));
        assert_eq!(heights(&e), [expected]);
        assert_eq!(e.undo_history().len(), count + 1);
    }
}

#[test]
fn numeric_clamp_no_ops_preserve_payloads_record_and_document() {
    for (stored, value, normalized) in
        [(Fit(65535), 1e300, Extreme), (Custom(65535), 440.0, Extreme), (Fit(0), -1e300, Micro), (Custom(46), 46.0, Small), (Small, 46.0, Small)]
    {
        let mut e = engine(&[stored]);
        e.session_mut().edit.zoom_toggle = Some(transit_model::ZoomToggle { samples_per_px: 90.0, scroll: 123, heights: vec![(TrackId(1), Medium)] });
        let before = e.session_arc();
        let rev = e.revision;
        assert_eq!(e.execute("track.height", &json!({"height":value})).unwrap(), json!({"height":normalized}));
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert_eq!(e.revision, rev);
        assert_eq!(heights(&e), [stored]);
        assert!(e.undo_history().is_empty());
    }
}

#[test]
fn an_explicit_preset_replaces_equal_custom_or_fit_and_undo_restores_it() {
    for stored in [Custom(46), Fit(46)] {
        let mut e = engine(&[stored]);
        assert_eq!(e.execute("track.height", &json!({"height":"small"})).unwrap(), json!({"height":"Small"}));
        assert_eq!(heights(&e), [Small]);
        assert_eq!(e.undo_history().len(), 1);
        assert!(e.undo());
        assert_eq!(heights(&e), [stored]);
        assert!(e.redo());
        assert_eq!(heights(&e), [Small]);
    }
}

#[test]
fn steps_at_both_ends_do_not_borrow_and_duplicate_ids_count_once() {
    for (h, cmd) in [(Fit(65535), "track.height_up"), (Fit(0), "track.height_down"), (Extreme, "track.height_up"), (Micro, "track.height_down")] {
        let mut e = engine(&[h]);
        let before = e.session_arc();
        assert_eq!(e.execute(cmd, &json!({})).unwrap(), json!({"changed":0}));
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
    }
    let mut e = engine(&[Small]);
    assert_eq!(e.execute("track.height_up", &json!({"tracks":[1,1]})).unwrap(), json!({"changed":1}));
    assert_eq!(heights(&e), [Medium]);
}

#[test]
fn all_track_zoom_and_reset_include_hidden_tracks_and_preserve_other_state() {
    let mut e = engine(&[Micro, Small, Fit(65535)]);
    e.session_mut().tracks[1].hidden = true;
    e.session_mut().tracks[2].inactive = true;
    e.session_mut().tracks[2].kind = TrackKind::Master;
    e.session_mut().edit.selected_tracks.clear();
    let zoom = e.session().edit.zoom;
    assert_eq!(e.execute("track.height_zoom", &json!({"direction":"in"})).unwrap(), json!({"changed":2}));
    assert_eq!(heights(&e), [Mini, Medium, Fit(65535)]);
    assert_eq!(e.execute("track.height_zoom", &json!({"direction":"out"})).unwrap(), json!({"changed":3}));
    assert_eq!(heights(&e), [Micro, Small, Jumbo]);
    assert_eq!(e.execute("track.height_reset", &json!({})).unwrap(), json!({"changed":2}));
    assert_eq!(heights(&e), [Small; 3]);
    let before = e.session_arc();
    assert_eq!(e.execute("track.height_reset", &json!({})).unwrap(), json!({"changed":0}));
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert!(e.session().tracks[1].hidden && e.session().tracks[2].inactive);
    assert_eq!(e.session().edit.zoom, zoom);
    assert!(e.session().edit.selected_tracks.is_empty());
    assert!(e.undo());
    assert_eq!(heights(&e), [Micro, Small, Jumbo]);
    let mut e = engine(&[Custom(46), Fit(46)]);
    assert_eq!(e.execute("track.height_reset", &json!({})).unwrap(), json!({"changed":2}));
    assert!(e.undo());
    assert_eq!(heights(&e), [Custom(46), Fit(46)]);
}

#[test]
fn empty_sessions_and_invalid_directions_do_not_edit() {
    let mut e = Engine::default();
    let before = e.session_arc();
    for direction in ["in", "out"] {
        assert_eq!(e.execute("track.height_zoom", &json!({"direction":direction})).unwrap(), json!({"changed":0}));
    }
    assert_eq!(e.execute("track.height_reset", &json!({})).unwrap(), json!({"changed":0}));
    for direction in [json!(null), json!(1), json!("IN"), json!("sideways")] {
        assert!(e.execute("track.height_zoom", &json!({"direction":direction})).is_err());
    }
    assert!(e.execute("track.height_zoom", &json!({})).is_err());
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert!(e.undo_history().is_empty());
}

#[test]
fn height_query_is_read_only_in_session_order_including_hidden() {
    let mut e = engine(&[Small, Fit(65535), Custom(57)]);
    e.session_mut().tracks[1].hidden = true;
    e.session_mut().tracks.swap(0, 2);
    let before = e.session_arc();
    let journal = e.journal.len();
    let rev = e.revision;
    assert_eq!(
        e.execute("query.track_heights", &json!({})).unwrap(),
        json!({"tracks":[
            {"id":3,"height":{"Custom":57},"points":57},
            {"id":2,"height":{"Fit":65535},"points":440},
            {"id":1,"height":"Small","points":46}
        ]})
    );
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert_eq!((e.revision, e.journal.len(), e.undo_history().len()), (rev, journal, 0));
}

#[test]
fn loading_legacy_fit_keeps_its_payload_and_does_not_dirty_the_session() {
    let e = engine(&[Fit(65535), Medium]);
    let bytes = e.session().to_json().unwrap();
    let loaded = Engine::new(Session::from_json(&bytes).unwrap());
    assert_eq!(loaded.session().tracks[0].height.points(), 440.0);
    assert_eq!(loaded.session().to_json().unwrap(), bytes);
    assert!(!loaded.is_dirty());
    assert!(loaded.session().load_notes.is_empty());
}

#[test]
fn ordinary_height_splits_a_held_merge_only_when_changed() {
    let mut e = engine(&[Fit(65535)]);
    e.hold_merge(Some("automation-pass"));
    e.execute_merged("mix.volume", &json!({"track":1,"db":-1}), "fader").unwrap();
    let key = e.merge_key.clone();
    e.execute("track.height", &json!({"height":1e300})).unwrap();
    assert_eq!(e.undo_history().len(), 1);
    assert_eq!(e.merge_key, key);
    assert!(e.execute("track.height", &json!({"tracks":[1,999],"height":57})).is_err());
    assert_eq!(e.merge_key, key);
    e.execute("track.height", &json!({"height":57})).unwrap();
    assert_eq!(e.undo_history().len(), 2);
    assert!(e.merge_key.is_none());
    assert_eq!(e.merge_hold.as_deref(), Some("automation-pass"));
    e.execute_merged("mix.volume", &json!({"track":1,"db":-2}), "fader").unwrap();
    e.execute_merged("mix.pan", &json!({"track":1,"pan":0.5}), "pan").unwrap();
    assert_eq!(e.undo_history().len(), 3);
    assert!(e.undo());
    assert_eq!(e.session().tracks[0].mixer.volume_db, -1.0);
    assert_eq!(heights(&e), [Custom(57)]);
    assert!(e.undo());
    assert_eq!(heights(&e), [Fit(65535)]);
}
