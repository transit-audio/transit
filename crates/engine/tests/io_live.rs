//! Machine-only live cues, including every command route that could enable one on Main/input.
#![allow(clippy::unwrap_used)]
use serde_json::json;
use transit_engine::Engine;
use transit_model::IoMap;

#[test]
fn live_cue_commands_persist_only_in_the_map_and_refuse_main_and_inputs() {
    let mut e = Engine::default();
    e.set_io_map(IoMap::for_device("test", "duplex", 2, 4));
    let before = e.session().to_json().unwrap();
    let (revision, undo) = (e.revision, e.can_undo());
    let result = e.execute("setup.io_map", &json!({"action":"set_live", "name":"Out 3-4", "live":true})).unwrap();
    assert_eq!(result["io"]["paths"][1]["live"], true);
    let saved = e.io_map().clone();
    e.execute("setup.io_map", &json!({"action":"set", "map":saved})).unwrap();
    assert_eq!(e.io_map(), &saved);
    e.execute("setup.io_map", &json!({"action":"add_path", "name":"Cue", "channels":[2,3], "live":true})).unwrap();
    assert!(e.io_map().path("Cue").unwrap().live);
    for (params, text) in [
        (json!({"action":"set_live", "name":"OUT 1-2", "live":true}), "Main cannot be a live path; turn off Live first"),
        (json!({"action":"set_live", "name":"In 1", "live":true}), "Live is only available on output paths"),
        (json!({"action":"set_main", "name":"Cue"}), "Main cannot be a live path; turn off Live first"),
        (
            json!({"action":"add_path", "name":"Live input", "direction":"input", "channels":[0], "live":true}),
            "Live is only available on output paths",
        ),
        (json!({"action":"set_live", "name":"Cue", "live":"yes"}), "`live` must be a boolean"),
    ] {
        let table = e.io_map().clone();
        let io_revision = e.io_revision();
        let error = e.execute("setup.io_map", &params).unwrap_err().to_string();
        assert!(error.contains(text), "{error}");
        assert_eq!(e.io_map(), &table);
        assert_eq!(e.io_revision(), io_revision);
    }
    for name in ["Out 1-2", "In 1"] {
        let mut invalid = e.io_map().clone();
        invalid.paths.iter_mut().find(|p| p.name == name).unwrap().live = true;
        assert!(e.execute("setup.io_map", &json!({"action":"set", "map":invalid})).is_err());
    }
    assert_eq!((e.revision, e.can_undo()), (revision, undo));
    assert_eq!(e.session().to_json().unwrap(), before);
    assert_eq!(transit_model::Session::from_json(&before).unwrap().to_json().unwrap(), before);
}
