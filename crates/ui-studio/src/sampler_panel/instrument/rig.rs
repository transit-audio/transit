//! Shared helpers of the Instrument view's tests (on the panel's rig, `super::super::test_rig`).

use super::super::test_rig::{T, mid};
use super::{InstrumentLayout, InstrumentView};
use serde_json::{Value, json};
use transit_model::TrackId;

pub use super::super::slices::rig::{h, moved, press, steps};

/// Loads a generated 48 kHz WAV of `frames` frames (named `name`) into zone `zone` (from 1), no pad,
/// then sets its `playback` fields.
pub fn zone(t: &mut T, tr: TrackId, zone: u16, name: &str, frames: usize, playback: Value) {
    let path = t.sample(name, frames);
    t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": zone, "path": path.to_string_lossy()}));
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": zone, "playback": playback}));
}

/// A kit with two velocity layers over keys 48 to 52 (zone 1 velocities 1 to 63, zone 2 64 to 127
/// labeled `ff`, both rooted at 50) and a sample on pad A1 (zone 3), the Instrument view showing.
pub fn layered() -> (T, TrackId) {
    let (mut t, tr) = T::open();
    zone(&mut t, tr, 1, "Soft.wav", 48_000, json!({"keys": [48, 52], "velocities": [1, 63], "root": 50}));
    zone(&mut t, tr, 2, "Loud.wav", 48_000, json!({"keys": [48, 52], "velocities": [64, 127], "root": 50, "label": "ff"}));
    let path = t.sample("Kick.wav", 4_800);
    t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": 3, "pad": 1, "path": path.to_string_lossy()}));
    show_instrument(&mut t, tr);
    (t, tr)
}

pub fn show_instrument(t: &mut T, tr: TrackId) {
    let switch = t.panel(tr).bar.switch.unwrap();
    t.click(mid(switch.segments[0]), 1);
}

pub fn layout(t: &T, tr: TrackId) -> InstrumentLayout {
    t.panel(tr).instrument.expect("the Instrument view")
}

/// The instance's Instrument view state.
pub fn view(t: &T) -> InstrumentView {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().instrument.clone()
}

/// The commands the view sent since `from` (an index into its log).
pub fn sent(t: &T, from: usize) -> Vec<String> {
    view(t).sent.into_iter().skip(from).collect()
}

/// The center of key `note` on the keyboard (its x) at `y`.
pub fn key_x(t: &T, tr: TrackId, note: u8) -> f32 {
    let k = layout(t, tr).key_map.keys.into_iter().find(|k| k.note == note).unwrap_or_else(|| panic!("key {note} is shown"));
    (k.rect[0] + k.rect[2]) / 2.0
}

/// A drag from `from` to `to`: press, two moves in their own frames, release.
pub fn drag(t: &mut T, from: (f32, f32), to: (f32, f32)) {
    let half = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    steps(t, vec![moved(from), press(from, true), moved(half), moved(to), press(to, false)]);
}

/// Zone `n`'s stored keys, velocities and root.
pub fn stored(t: &T, tr: TrackId, n: u16) -> ([u8; 2], [u8; 2], u8) {
    let p = t.patch(tr);
    let z = p.zone(n - 1).unwrap();
    (z.keys, z.velocities, z.root)
}
