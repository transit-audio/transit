//! Every edit that empties a zone resets that slot's `zoneNNN_*` controls in the same undo step
//! (#478): Clear pad and Delete zone (`mix.sampler_zone_clear`), a slice binding freed by a marker
//! edit, and a pad Slices to pads unbinds. Generated audio in scratch folders; no devices, no
//! clock outcome, no pinned hash.

use super::*;
use std::collections::BTreeMap;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_model::sampler_patch::{Marker, Markers, SliceBinding, SliceMode, SliceSet};

struct Dir(transit_test_support::TestDir);

impl Dir {
    fn new() -> Self {
        Self(transit_test_support::TestDir::created("zone-reset"))
    }

    fn audio(&self, name: &str, frames: usize) -> PathBuf {
        let path = self.0.join(name);
        let samples = (0..frames).map(|i| ((i.wrapping_mul(1664525).wrapping_add(1013904223) >> 16) & 255) as f32 / 256.0 - 0.5).collect();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![samples] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, ..EncodeOptions::default() };
        std::fs::write(&path, transit_audio_io::encode(&audio, &opts).unwrap()).unwrap();
        path
    }
}

fn setup() -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    (e, t)
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    patch_of(e.session(), "test", t).unwrap()
}

fn control(e: &mut Engine, t: TrackId, id: &str, value: f64) {
    e.execute("mix.sampler_control", &json!({"track": t, "id": id, "value": value})).unwrap();
}

/// The stored controls of zone slot `n` (1-based).
fn slot(p: &PatchV1, n: u16) -> BTreeMap<String, f32> {
    let prefix = format!("zone{n:03}_");
    p.controls.iter().filter(|(id, _)| id.starts_with(&prefix)).map(|(id, v)| (id.clone(), *v)).collect()
}

/// What `mix.sampler_state` would play for slot `n`: each control's stored value or its default.
fn effective(p: &PatchV1, n: u16, field: &str) -> f32 {
    let id = format!("zone{n:03}_{field}");
    let (_, f) = dsp_sampler::resolve(&id).unwrap();
    p.controls.get(&id).copied().unwrap_or(dsp_sampler::field_info(f).unwrap().default)
}

#[test]
fn clear_pad_resets_the_zones_controls_in_its_undo_step_and_the_next_sample_starts_from_defaults() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let (kick, snare, hat, ride) = (d.audio("Kick.wav", 128), d.audio("Snare.wav", 96), d.audio("Hat.wav", 64), d.audio("Ride.wav", 80));
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": kick})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 2, "path": snare})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 3, "pad": 3, "path": hat})).unwrap();
    control(&mut e, t, "zone001_level", -12.0);
    control(&mut e, t, "zone001_pan", 40.0);
    control(&mut e, t, "zone001_amp_attack", 0.25);
    // Another pad's settings and an untouched pad must come through unchanged.
    control(&mut e, t, "zone002_level", -3.0);
    let set = patch(&e, t);
    assert_eq!(slot(&set, 1).len(), 3, "the level, pan and envelope value are stored: {:?}", set.controls);
    let defaults = [("level", 0.0), ("pan", 0.0)];
    let attack_default = {
        let (_, f) = dsp_sampler::resolve("zone001_amp_attack").unwrap();
        dsp_sampler::field_info(f).unwrap().default
    };
    let steps = e.undo_history().len();
    e.execute("mix.sampler_zone_clear", &json!({"track": t, "zone": 1})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 1, "one undo step");
    let cleared = patch(&e, t);
    assert!(cleared.zone(0).is_none() && cleared.pad(0).zone.is_none());
    assert_eq!(slot(&cleared, 1), BTreeMap::new(), "the cleared zone's controls are gone");
    assert_eq!(slot(&cleared, 2), slot(&set, 2), "another zone keeps its controls");
    // The source indices renumber when the cleared zone's source drops out of the patch.
    let unindexed = |p: &PatchV1, z: u16| p.zone(z).map(|x| ZoneEntry { source: 0, ..x.clone() });
    assert_eq!((unindexed(&cleared, 1), unindexed(&cleared, 2)), (unindexed(&set, 1), unindexed(&set, 2)), "the other zones are unchanged");
    assert_eq!(slot(&cleared, 3), BTreeMap::new(), "an untouched zone has none before and after");
    // One undo restores the cleared pad with its settings; redo clears it again.
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(patch(&e, t), set, "undo restores the zone, its pad and its controls");
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(patch(&e, t), cleared, "redo clears them again");
    // The next sample in the freed slot (the lowest free zone, as a drop takes it) plays defaults.
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": ride})).unwrap();
    let next = patch(&e, t);
    assert!(next.zone(0).is_some());
    for (field, want) in defaults {
        assert_eq!(effective(&next, 1, field), want, "{field}");
    }
    assert_eq!(effective(&next, 1, "amp_attack"), attack_default);
    assert_eq!(effective(&next, 2, "level"), -3.0, "zone 2 keeps its level");
}

/// Slice loop setup: an instrument whose patch has a slice loop over a generated managed source
/// with `n` evenly spaced Manual slices showing, its pads from note 60.
fn sliced(n: u64) -> (Engine, TrackId) {
    let (mut e, t) = setup();
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    let rate = e.session().sample_rate.hz();
    let len = 48_000u64;
    let buf = AudioBuffer { sample_rate: rate, channels: vec![(0..len).map(|i| 0.4 * (i as f32 * 0.013).sin()).collect()] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
    let l = p.slicing.as_mut().unwrap();
    let markers = (0..n).map(|k| Marker { id: k as u32 + 1, frame: k * len / n, ..Marker::default() }).collect();
    l.manual = Some(Markers { markers, next_id: n as u32 + 1, ..Markers::default() });
    l.mode = SliceMode::Manual;
    (e, t)
}

#[test]
fn a_slice_binding_freed_by_a_marker_edit_resets_its_zones_controls() {
    let (mut e, t) = sliced(4);
    e.execute("mix.sampler_slices_to_pads", &json!({"track": t})).unwrap();
    let p = patch(&e, t);
    let z = p.zones.iter().find(|z| z.slice.as_ref().is_some_and(|b| b.id == 4)).map(|z| z.id + 1).unwrap();
    let kept = p.zones.iter().find(|z| z.slice.as_ref().is_some_and(|b| b.id == 1)).map(|z| z.id + 1).unwrap();
    control(&mut e, t, &format!("zone{z:03}_level"), -9.0);
    control(&mut e, t, &format!("zone{kept:03}_pan"), -20.0);
    let set = patch(&e, t);
    let out = e.execute("mix.sampler_slice_marker", &json!({"track": t, "op": "remove", "id": 4})).unwrap();
    assert_eq!(out["freed_zones"], json!([z]), "{out}");
    let after = patch(&e, t);
    assert_eq!(slot(&after, z), BTreeMap::new(), "the freed zone's controls are gone");
    assert_eq!(slot(&after, kept), slot(&set, kept), "a zone still bound keeps its controls");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(patch(&e, t), set, "one undo restores the binding and its controls");
}

#[test]
fn a_pad_slices_to_pads_unbinds_has_its_zones_controls_reset() {
    let (mut e, t) = sliced(4);
    // Zone 11 on pad A5 plays Manual slice 1; a run from note 36 takes other pads and unbinds it.
    {
        let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
        let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        let mut z = ZoneEntry::whole(10, 0, 48_000);
        z.slice = Some(SliceBinding { set: SliceSet::Manual, id: 1, ..SliceBinding::default() });
        p.zones.push(z);
        p.set_pad(PadEntry { zone: Some(10), ..p.pad(4) });
    }
    control(&mut e, t, "zone011_level", -6.0);
    let set = patch(&e, t);
    let out = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 36})).unwrap();
    assert_eq!(out["unbound"][0]["zone"], 11, "{out}");
    let after = patch(&e, t);
    assert!(after.zone(10).is_none());
    assert_eq!(slot(&after, 11), BTreeMap::new(), "the unbound pad's zone controls are gone");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(patch(&e, t), set, "one undo restores them");
}
