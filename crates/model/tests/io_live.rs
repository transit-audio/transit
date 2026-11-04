//! The live cue declaration is a machine-map preference with a backward-compatible default.
#![allow(clippy::unwrap_used)]
use transit_model::IoMap;

#[test]
fn old_maps_default_to_full_compensation_and_live_maps_round_trip() {
    let map = IoMap::for_device("test", "duplex", 2, 4);
    let mut old = serde_json::to_value(&map).unwrap();
    for p in old["paths"].as_array_mut().unwrap() {
        p.as_object_mut().unwrap().remove("live");
    }
    let mut loaded: IoMap = serde_json::from_value(old).unwrap();
    let saved = serde_json::to_value(&loaded).unwrap();
    assert_eq!(saved["paths"][0]["live"], false);
    assert!(loaded.paths.iter().all(|p| !p.live));
    loaded.set_live("Out 3-4", true).unwrap();
    let saved = serde_json::to_string(&loaded).unwrap();
    assert_eq!(serde_json::from_str::<IoMap>(&saved).unwrap(), loaded);
}

#[test]
fn a_device_shrink_clears_live_when_a_cue_becomes_fallback_main() {
    let mut map = IoMap::for_device("test", "duplex", 2, 4);
    map.set_main("Out 3-4").unwrap();
    map.set_live("Out 1-2", true).unwrap();
    assert!(map.path("Out 1-2").unwrap().live);
    map.reconcile(2, 2);
    assert_eq!(map.main, "Out 1-2");
    assert!(!map.path(&map.main).unwrap().live);
    assert!(map.validate(map.path(&map.main).unwrap()).is_ok());
}
