#![allow(clippy::unwrap_used)]

use transit_model::Session;

#[test]
fn an_old_session_defaults_to_200_ms() {
    let mut old = serde_json::to_value(Session::default()).unwrap();
    assert!(old.get("touch_return_ms").is_none(), "the default preserves old session file bytes");
    old.as_object_mut().unwrap().remove("touch_return_ms");
    let loaded = Session::from_json(&old.to_string()).unwrap();
    assert_eq!(loaded.touch_return_ms, 200);
    assert_eq!(Session::default().touch_return_ms, 200);
}

#[test]
fn non_default_touch_return_round_trips_in_the_session_file() {
    for ms in [0, 50, 1000, 2000] {
        let s = Session { touch_return_ms: ms, ..Session::default() };
        let saved = s.to_json().unwrap();
        assert_eq!(Session::from_json(&saved).unwrap().touch_return_ms, ms);
    }
}

#[test]
fn a_loaded_out_of_range_duration_is_clamped() {
    let mut file = serde_json::to_value(Session::default()).unwrap();
    file["touch_return_ms"] = serde_json::json!(65535);
    assert_eq!(Session::from_json(&file.to_string()).unwrap().touch_return_ms, 2000);
}
