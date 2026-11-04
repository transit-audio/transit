//! The audio preferences merge persists cue opt-outs in the per-device table.
#![allow(clippy::unwrap_used)]
use super::persist_audio;
use crate::{UiState, io_settings};
use std::sync::atomic::{AtomicUsize, Ordering};
use transit_model::IoMap;

#[test]
fn live_cue_survives_the_atomic_ui_json_merge_and_device_adoption() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("transit-live-cue-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ui.json");
    std::fs::write(&file, r#"{"unrelated_preference":42}"#).unwrap();
    let mut ui = UiState::default();
    let mut map = IoMap::for_device("test", "duplex", 2, 4);
    map.set_live("Out 3-4", true).unwrap();
    io_settings::remember(&mut ui.io_maps, &map);
    persist_audio(&file, &ui).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["io_maps"][0]["paths"][1]["live"], true);
    assert_eq!(value["unrelated_preference"], 42);
    let loaded: UiState = serde_json::from_slice(&bytes).unwrap();
    let adopted = io_settings::adopt(&loaded.io_maps, IoMap::for_device("test", "duplex", 2, 4));
    assert_eq!(adopted, map);
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
