//! The Instrument view commands through the engine (ADR 0041, amendment 2026-11-08 items 1, 2, 3
//! and 11, amendment 2026-11-09 refusal (c); #270): group modes, the batched key map, the zone
//! edit's new keys and set-time checks, the seed, and `mix.sampler_state`. Sources are generated
//! buffers; renders go through the offline mix (the plan compiler's path). No sleeps.

use crate::{Engine, EngineError};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_midi::{MiddleC, Note, Sequence};
use transit_model::sampler_patch::{LoopMode, PatchV1, PlayMode, RoundRobin, SliceBinding, SliceSet};
use transit_model::{Clip, SourceAudio, TrackId};
use transit_time::{Range, TICKS_PER_QUARTER};

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("sc-sampler-groups-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn instrument(e: &mut Engine) -> TrackId {
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    e.session().tracks.last().unwrap().id
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

fn edit_patch(e: &mut Engine, t: TrackId, f: impl FnOnce(&mut PatchV1)) {
    f(e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch.as_mut().unwrap().ready_mut().unwrap());
}

/// A session source holding `frames` frames of `f(i)` at `rate` Hz, stored as given (a source
/// keeps its native rate).
fn source_with(e: &mut Engine, rate: u32, frames: usize, f: impl Fn(usize) -> f32) -> u64 {
    let buf = AudioBuffer { sample_rate: rate, channels: vec![(0..frames).map(f).collect()] };
    crate::io::add_source_audio(e.session_mut(), "gen", Arc::new(SourceAudio::new(buf)), None, FileFormat::Wav).0
}

/// A constant source of 48,000 frames at 48 kHz.
fn level(e: &mut Engine, v: f32) -> u64 {
    source_with(e, 48_000, 48_000, |_| v)
}

/// A new patch (seed 1) on a new instrument track, with one 48 kHz source.
fn setup(e: &mut Engine) -> (TrackId, u64) {
    let t = instrument(e);
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    let src = level(e, 0.5);
    (t, src)
}

/// Zone `zone` on `src`, unbound, keys `[key, key]` (so the zones never pass eight per note).
fn zone_on(e: &mut Engine, t: TrackId, zone: u16, src: u64, key: u8) {
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": zone, "source": src, "playback": {"keys": [key, key]}})).unwrap();
}

/// The text of a refused command.
fn refusal(r: Result<Value, EngineError>) -> String {
    match r {
        Err(EngineError::BadParams(_, m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Runs `cmd` and requires a refusal with exactly `text` that leaves the document as it was.
fn refused(e: &mut Engine, cmd: &str, p: Value, text: &str) {
    let (before, revision) = (e.session_arc(), e.revision);
    assert_eq!(refusal(e.execute(cmd, &p)), text, "{cmd} {p}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()), "{cmd} {p}: a refusal leaves the document bit-identical");
    assert_eq!(e.revision, revision, "{cmd} {p}: nothing to publish");
}

/// Runs an undoable `cmd`, which must be one undo step, undone to `before` and redone to `after`.
fn one_step(e: &mut Engine, t: TrackId, cmd: &str, p: Value) -> PatchV1 {
    let (before, steps) = (patch(e, t), e.undo_history().len());
    e.execute(cmd, &p).unwrap();
    let after = patch(e, t);
    assert_ne!(after, before, "{cmd} changed the patch");
    assert_eq!(e.undo_history().len(), steps + 1, "{cmd}: one undo step");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(patch(e, t), before, "{cmd}: undo restores the patch bit for bit");
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(patch(e, t), after, "{cmd}: redo reproduces it");
    after
}

fn state(e: &mut Engine, t: TrackId) -> Value {
    e.execute("mix.sampler_state", &json!({"track": t})).unwrap()
}

fn zone_state(e: &mut Engine, t: TrackId, zone: u16) -> Value {
    state(e, t)["zones"].as_array().unwrap().iter().find(|z| z["zone"] == zone).cloned().unwrap()
}

fn member(zone: u16, keys: [u8; 2], velocities: [u8; 2], group: u8, layer: u8, alternative: u8) -> Value {
    json!({"zone": zone, "keys": keys, "velocities": velocities, "root": 60, "membership": {"group": group, "layer": layer, "alternative": alternative}})
}

#[test]
fn loop_requires_gate_both_ways() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "source": src, "playback": {"mode": "one_shot"}})).unwrap();
    let text = "A one-shot cannot loop: choose Gate, or turn the loop off.";
    refused(&mut e, "mix.sampler_zone_edit", json!({"track": t, "zone": 1, "region": {"loop_mode": "forward"}}), text);
    // The Slices view's Loop press: the loop and Gate in one edit, one step restoring both.
    let both = json!({"track": t, "zone": 1, "region": {"loop_mode": "forward"}, "playback": {"mode": "gate"}});
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", both);
    assert_eq!((after.zone(0).unwrap().mode, after.zone(0).unwrap().loop_mode), (PlayMode::Gate, LoopMode::Forward));
    // One-shot on a looping zone, unless the same edit turns the loop off.
    refused(&mut e, "mix.sampler_zone_edit", json!({"track": t, "zone": 1, "playback": {"mode": "one_shot"}}), text);
    let off = json!({"track": t, "zone": 1, "region": {"loop_mode": "off"}, "playback": {"mode": "one_shot"}});
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", off);
    assert_eq!((after.zone(0).unwrap().mode, after.zone(0).unwrap().loop_mode), (PlayMode::OneShot, LoopMode::Off));
    // Checked when set: a stored looping one-shot (a loaded file) takes an edit of something else,
    // and plays unlooped.
    edit_patch(&mut e, t, |p| p.zones[0].loop_mode = LoopMode::Forward);
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"transpose": 2}})).unwrap();
    let z = zone_state(&mut e, t, 1);
    assert_eq!((z["loop_mode"].clone(), z["loop_used"].clone()), (json!("forward"), json!("off")));
}

/// A slice loop over the source's 48,000 frames, Manual markers at 0, 10,000 and 10,001 (slice 2
/// is one frame long), and zone 2 bound to slice `slice` (1-based) with stored fields over the
/// whole source.
fn sliced(e: &mut Engine, slice: usize) -> (TrackId, u64) {
    let (t, src) = setup(e);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": src})).unwrap();
    for frame in [10_000, 10_001] {
        e.execute("mix.sampler_slice_marker", &json!({"track": t, "op": "add", "frame": frame, "snap": false})).unwrap();
    }
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "source": src})).unwrap();
    edit_patch(e, t, |p| {
        let id = p.slicing.as_ref().unwrap().manual.as_ref().unwrap().markers[slice - 1].id;
        p.zones[0].slice = Some(SliceBinding { set: SliceSet::Manual, id, ..SliceBinding::default() });
    });
    (t, src)
}

#[test]
fn slice_zone_loop_edit_writes_through_in_one_undo_step() {
    let mut e = Engine::default();
    let (t, _) = sliced(&mut e, 3);
    let z = patch(&e, t).zone(1).cloned().unwrap();
    assert_eq!((z.start, z.end, z.loop_start, z.loop_end), (0, 48_000, 0, 48_000), "stored fields disagree with slice 3");
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", json!({"track": t, "zone": 2, "region": {"loop_mode": "forward"}}));
    let z = after.zone(1).unwrap();
    assert_eq!((z.start, z.end, z.loop_start, z.loop_end, z.loop_mode), (10_001, 48_000, 10_001, 48_000, LoopMode::Forward), "written through");
    assert_eq!(z.source, after.slicing.as_ref().unwrap().source, "the slice's source too");
    // Its region and loop points follow the slice: an edit of them is refused.
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 2, "region": {"loop_start": 20_000}}),
        "Zone 2 plays slice 3: its region and loop follow the slice (`loop_start` cannot be set).",
    );
    let r = e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 2, "region": {"loop_mode": "off"}})).unwrap();
    assert_eq!((r["loop_used"].clone(), r["region"]["start"].clone()), (json!("off"), json!(10_001)), "the result carries the used values");
}

#[test]
fn one_frame_slice_refuses_a_loop_with_the_exact_text() {
    let mut e = Engine::default();
    let (t, _) = sliced(&mut e, 2);
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 2, "region": {"loop_mode": "forward"}}),
        "Slice 2 is one frame long, too short to loop.",
    );
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 2, "region": {"loop_mode": "ping_pong"}}),
        "Slice 2 is one frame long, too short to loop.",
    );
    // A looping slice shortened to one frame later (stored) stands and plays unlooped.
    edit_patch(&mut e, t, |p| p.zones[0].loop_mode = LoopMode::Forward);
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 2, "playback": {"transpose": 1}})).unwrap();
    assert_eq!(zone_state(&mut e, t, 2)["loop_used"], "off");
}

#[test]
fn pingpong_press_sends_crossfade_zero_and_undo_restores_it() {
    let mut e = Engine::default();
    let (t, _) = sliced(&mut e, 3);
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 2, "region": {"loop_mode": "forward", "crossfade": 100}})).unwrap();
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 2, "region": {"loop_mode": "ping_pong"}}),
        "A ping-pong loop has no crossfade: set the crossfade to 0 in the same edit.",
    );
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", json!({"track": t, "zone": 2, "region": {"loop_mode": "ping_pong", "crossfade": 0}}));
    assert_eq!((after.zone(1).unwrap().loop_mode, after.zone(1).unwrap().crossfade), (LoopMode::PingPong, 0));
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(
        (patch(&e, t).zone(1).unwrap().loop_mode, patch(&e, t).zone(1).unwrap().crossfade),
        (LoopMode::Forward, 100),
        "undo restores the crossfade"
    );
}

#[test]
fn crossfade_set_time_limits() {
    let mut e = Engine::default();
    let (t, src48) = setup(&mut e);
    let src44 = source_with(&mut e, 44_100, 44_100, |_| 0.25);
    // A forward loop of 453 frames on each source: at most 226 frames of crossfade.
    for (zone, src) in [(1, src48), (2, src44)] {
        let p = json!({"track": t, "zone": zone, "source": src, "region": {"loop_mode": "forward", "loop_start": 1000, "loop_end": 1453}, "playback": {"keys": [zone, zone]}});
        e.execute("mix.sampler_zone_source", &p).unwrap();
    }
    let x = |zone: u16, frames: u64| json!({"track": t, "zone": zone, "region": {"crossfade": frames}});
    refused(&mut e, "mix.sampler_zone_edit", x(1, 227), "Crossfade is at most 4.7 ms (226 frames) for this loop.");
    refused(&mut e, "mix.sampler_zone_edit", x(2, 227), "Crossfade is at most 5.1 ms (226 frames) for this loop.");
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", x(1, 226));
    assert_eq!(after.zone(0).unwrap().crossfade, 226);
    // Ping-pong refuses any crossfade; the same edit setting 0 passes.
    let pp = json!({"track": t, "zone": 2, "region": {"loop_mode": "ping_pong", "crossfade": 5}});
    refused(&mut e, "mix.sampler_zone_edit", pp, "A ping-pong loop has no crossfade: set the crossfade to 0 in the same edit.");
    // Clamped when used, never refused on read or by another edit: the loop shrinks to 300 frames.
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "region": {"loop_end": 1300}})).unwrap();
    let z = zone_state(&mut e, t, 1);
    assert_eq!((z["crossfade"].clone(), z["crossfade_used"].clone(), z["rate"].clone()), (json!(226), json!(150), json!(48_000)));
    assert_eq!(zone_state(&mut e, t, 2)["rate"], 44_100, "the zone source's native rate");
    refused(&mut e, "mix.sampler_zone_edit", x(1, 151), "Crossfade is at most 3.1 ms (150 frames) for this loop.");
    let bad = json!({"track": t, "zone": 1, "region": {"crossfade": -3}});
    refused(&mut e, "mix.sampler_zone_edit", bad, "`crossfade` must be a whole number of source frames, 0 or more");
}

#[test]
fn fades_set_time_limits() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "source": src, "playback": {"keys": [40, 50]}})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 1, "source": src})).unwrap();
    let f = |zone: u16, w: [u8; 4]| json!({"track": t, "zone": zone, "playback": {"fades": {"key_lo": w[0], "key_hi": w[1], "vel_lo": w[2], "vel_hi": w[3]}}});
    refused(&mut e, "mix.sampler_zone_edit", f(1, [6, 5, 0, 0]), "Key fades are at most 10 keys together for this zone.");
    refused(&mut e, "mix.sampler_zone_edit", f(1, [0, 0, 100, 27]), "Velocity fades are at most 126 velocity steps together for this zone.");
    refused(&mut e, "mix.sampler_zone_edit", f(2, [0, 0, 1, 0]), "A pad's zone has no fades: it plays one key and one velocity layer.");
    refused(&mut e, "mix.sampler_zone_edit", f(1, [128, 0, 0, 0]), "fade widths are integers from 0 to 127");
    let partial = json!({"track": t, "zone": 1, "playback": {"fades": {"key_lo": 2}}});
    refused(&mut e, "mix.sampler_zone_edit", partial, "fade widths are integers from 0 to 127");
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", f(1, [5, 5, 3, 0]));
    assert_eq!(after.zone(0).unwrap().fades.widths(), [5, 5, 3, 0]);
    // A later range edit keeps the widths; playback uses them clamped (the high one shrinks first).
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"keys": [40, 44]}})).unwrap();
    let z = zone_state(&mut e, t, 1);
    assert_eq!(z["fades"], json!({"key_lo": 5, "key_hi": 5, "vel_lo": 3, "vel_hi": 0}));
    assert_eq!(z["fades_used"], json!({"key_lo": 4, "key_hi": 0, "vel_lo": 3, "vel_hi": 0}));
}

#[test]
fn an_unbound_trim_that_leaves_the_loop_outside_is_refused_and_one_that_moves_the_loop_passes() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    let p = json!({"track": t, "zone": 1, "source": src, "region": {"loop_mode": "forward", "loop_start": 1000, "loop_end": 30_000}});
    e.execute("mix.sampler_zone_source", &p).unwrap();
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 1, "region": {"end": 20_000}}),
        "Zone 1's loop (frames 1000 to 30000) must lie inside its region (frames 0 to 20000) and be at least two frames long: move the loop in the same edit, or turn it off.",
    );
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", json!({"track": t, "zone": 1, "region": {"end": 20_000, "loop_end": 20_000}}));
    let z = after.zone(0).unwrap();
    assert_eq!((z.end, z.loop_start, z.loop_end), (20_000, 1000, 20_000));
    // With the loop off, the trim needs nothing else.
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "region": {"loop_mode": "off"}})).unwrap();
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "region": {"end": 500}})).unwrap();
}

#[test]
fn commands_undo_as_one_step() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    for z in 1..=6 {
        zone_on(&mut e, t, z, src, 30 + z as u8);
    }
    // A zone edit with membership, fades and crossfade together.
    let p = json!({"track": t, "zone": 1, "region": {"loop_mode": "forward", "loop_start": 0, "loop_end": 1000, "crossfade": 40},
        "playback": {"membership": {"group": 2, "layer": 0, "alternative": 0}, "fades": {"key_lo": 0, "key_hi": 0, "vel_lo": 10, "vel_hi": 0}}});
    let after = one_step(&mut e, t, "mix.sampler_zone_edit", p);
    let z = after.zone(0).unwrap();
    assert_eq!((z.crossfade, z.fades.widths(), z.membership.as_ref().map(|m| m.group)), (40, [0, 0, 10, 0], Some(2)));
    one_step(&mut e, t, "mix.sampler_group_set", json!({"track": t, "group": 2, "round_robin": "random"}));
    // A six-zone batch: group 5 of two layers and three alternatives.
    let zones: Vec<Value> =
        (0..6u16).map(|i| member(i + 1, [36, 60], if i < 3 { [1, 63] } else { [64, 127] }, 5, (i / 3) as u8, (i % 3) as u8)).collect();
    let after = one_step(&mut e, t, "mix.sampler_zone_map", json!({"track": t, "zones": zones}));
    assert!(after.zones.iter().all(|z| z.keys == [36, 60] && z.membership.as_ref().is_some_and(|m| m.group == 5)));
}

#[test]
fn group_set_stores_a_mode_and_sequential_is_not_stored() {
    let mut e = Engine::default();
    let (t, _) = setup(&mut e);
    let r = e.execute("mix.sampler_group_set", &json!({"track": t, "group": 255, "round_robin": "random"})).unwrap();
    assert_eq!(r, json!({"track": t, "group": 255, "round_robin": "random"}));
    assert_eq!(patch(&e, t).groups[0].round_robin, RoundRobin::Random);
    let steps = e.undo_history().len();
    e.execute("mix.sampler_group_set", &json!({"track": t, "group": 255, "round_robin": "random"})).unwrap();
    assert_eq!(e.undo_history().len(), steps, "an unchanged mode records no step");
    e.execute("mix.sampler_group_set", &json!({"track": t, "group": 255, "round_robin": "sequential"})).unwrap();
    assert!(patch(&e, t).groups.is_empty(), "the default is not stored");
    refused(&mut e, "mix.sampler_group_set", json!({"track": t, "group": 256, "round_robin": "random"}), "`group` must be an integer from 0 to 255");
    refused(&mut e, "mix.sampler_group_set", json!({"track": t, "group": 1, "round_robin": "shuffle"}), "`round_robin` must be sequential or random");
}

/// Ten zones on keys 50 to 59, one each.
fn ten(e: &mut Engine) -> (TrackId, u64) {
    let (t, src) = setup(e);
    for z in 1..=10u16 {
        zone_on(e, t, z, src, 49 + z as u8);
    }
    (t, src)
}

#[test]
fn the_map_refuses_each_group_shape_fault_with_its_text_and_changes_nothing() {
    let mut e = Engine::default();
    let (t, _) = ten(&mut e);
    let map = |zones: Vec<Value>| json!({"track": t, "zones": zones});
    let good: Vec<Value> =
        (0..6u16).map(|i| member(i + 1, [36, 60], if i < 3 { [1, 63] } else { [64, 127] }, 3, (i / 3) as u8, (i % 3) as u8)).collect();
    let mut nine_layers = good.clone();
    nine_layers.push(member(7, [36, 60], [1, 5], 3, 8, 0));
    let nine_alternatives: Vec<Value> = (0..9u16).map(|a| member(a + 1, [36, 60], [1, 127], 3, 0, a as u8)).collect();
    let mut keys = good.clone();
    keys[4]["keys"] = json!([36, 61]);
    let mut velocities = good.clone();
    velocities[2]["velocities"] = json!([1, 62]);
    let gap: Vec<Value> = good.iter().filter(|z| z["zone"] != 5).cloned().collect();
    let mut duplicate = good.clone();
    duplicate.push(member(7, [36, 60], [64, 127], 3, 1, 2));
    let cases = [
        (nine_layers, "group 3 would have 9 layers (at most 8)"),
        (nine_alternatives, "group 3 layer 0 would have a ninth alternative (at most 8)"),
        (keys, "group 3: zone 5 has another key range than zone 1 (a group has one key range)"),
        (velocities, "group 3 layer 0: zone 3 has another velocity range than zone 1 (a layer's alternatives share one)"),
        (gap, "group 3 layer 1 has no alternative 1 (alternatives count up from 0 without a gap)"),
        (duplicate, "zones 6 and 7 are both group 3 layer 1 alternative 2"),
    ];
    for (zones, text) in cases {
        refused(&mut e, "mix.sampler_zone_map", map(zones), text);
    }
    e.execute("mix.sampler_zone_map", &map(good)).unwrap();
    // A zone edit that breaks the shape is refused the same way (the map is admitted whole).
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 2, "playback": {"keys": [36, 59]}}),
        "group 3: zone 2 has another key range than zone 1 (a group has one key range)",
    );
}

#[test]
fn eight_layer_bound_refused_before_publication() {
    let mut e = Engine::default();
    let (t, src) = ten(&mut e);
    let plain = |zone: u16| json!({"zone": zone, "keys": [60, 60], "velocities": [1, 127], "root": 60, "membership": null});
    // Eight plain zones over note 60 are the bound; a ninth is refused, naming it and the note.
    e.execute("mix.sampler_zone_map", &json!({"track": t, "zones": (1..=8).map(plain).collect::<Vec<_>>()})).unwrap();
    refused(&mut e, "mix.sampler_zone_map", json!({"track": t, "zones": [plain(9)]}), "Zone 9 would be a ninth layer on C3.");
    // A group layer is one voice whatever its alternatives; a second layer at those velocities is
    // another voice.
    let r = json!({"track": t, "zones": [plain(1), member(9, [60, 60], [1, 127], 0, 0, 0), member(10, [60, 60], [1, 127], 0, 0, 1)]});
    refused(&mut e, "mix.sampler_zone_map", r, "Zone 9 would be a ninth layer on C3.");
    let swap = json!({"track": t, "zones": [
        json!({"zone": 1, "keys": [61, 61], "velocities": [1, 127], "root": 60, "membership": null}),
        member(9, [60, 60], [1, 127], 0, 0, 0), member(10, [60, 60], [1, 127], 0, 0, 1)]});
    e.execute("mix.sampler_zone_map", &swap).unwrap();
    // A zone added with the default full keyboard is a ninth voice on note 60 (named from note 0
    // up: the first note it would overload).
    zone_on(&mut e, t, 11, src, 62);
    refused(&mut e, "mix.sampler_zone_source", json!({"track": t, "zone": 12, "source": src}), "Zone 12 would be a ninth layer on C3.");
    // The 257th leaf.
    refused(&mut e, "mix.sampler_zone_map", json!({"track": t, "zones": [plain(257)]}), "zone 257 would be the 257th zone (at most 256)");
    refused(&mut e, "mix.sampler_zone_edit", json!({"track": t, "zone": 257}), "`zone` must be 1-256");
}

#[test]
fn a_ninth_layer_refusal_names_its_note_in_the_message_middle_c() {
    let mut texts = Vec::new();
    for names in [MiddleC::C3, MiddleC::C4] {
        let mut e = Engine::default();
        e.set_message_middle_c(names);
        let (t, _) = ten(&mut e);
        let plain = |zone: u16| json!({"zone": zone, "keys": [64, 64], "velocities": [1, 127], "root": 60, "membership": null});
        e.execute("mix.sampler_zone_map", &json!({"track": t, "zones": (1..=8).map(plain).collect::<Vec<_>>()})).unwrap();
        texts.push(refusal(e.execute("mix.sampler_zone_map", &json!({"track": t, "zones": [plain(9)]}))));
    }
    assert_eq!(texts, ["Zone 9 would be a ninth layer on E3.", "Zone 9 would be a ninth layer on E4."]);
}

#[test]
fn membership_on_a_bound_zone_without_unbinding_refused() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "source": src})).unwrap();
    let text = "Zone 1 is pad A1's zone: unbind the pad in the same edit (pad: null) to make it a group member.";
    refused(&mut e, "mix.sampler_zone_map", json!({"track": t, "zones": [member(1, [36, 36], [1, 127], 0, 0, 0)]}), text);
    refused(
        &mut e,
        "mix.sampler_zone_edit",
        json!({"track": t, "zone": 1, "playback": {"membership": {"group": 0, "layer": 0, "alternative": 0}}}),
        text,
    );
    // Changing views never unbinds: an edit without membership keeps the pad.
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"transpose": 1}})).unwrap();
    assert_eq!(patch(&e, t).pad(0).zone, Some(0));
}

#[test]
fn make_group_unbinds_the_pad_and_group_in_one_step() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "source": src})).unwrap();
    zone_on(&mut e, t, 2, src, 70);
    // Make group: pad A1's zone and zone 2 become group 4's two alternatives, the pad unbound.
    let mut first = member(1, [36, 48], [1, 127], 4, 0, 0);
    first["pad"] = Value::Null;
    let map = json!({"track": t, "zones": [first, member(2, [36, 48], [1, 127], 4, 0, 1)]});
    let after = one_step(&mut e, t, "mix.sampler_zone_map", map);
    assert_eq!(after.pad(0).zone, None, "the pad is unbound (its note stays)");
    assert_eq!(after.pad_note(0), 36);
    assert!(after.zones.iter().all(|z| z.membership.as_ref().is_some_and(|m| m.group == 4)));
    e.execute("edit.undo", &json!({})).unwrap();
    let back = patch(&e, t);
    assert_eq!((back.pad(0).zone, back.zone(0).unwrap().membership.clone()), (Some(0), None), "one undo restores the pad and the zone");
    // The zone edit takes `pad: null` too.
    e.execute(
        "mix.sampler_zone_edit",
        &json!({"track": t, "zone": 1, "pad": null, "playback": {"membership": {"group": 1, "layer": 0, "alternative": 0}}}),
    )
    .unwrap();
    assert_eq!(patch(&e, t).pad(0).zone, None);
}

#[test]
fn seed_command_stores_one_for_zero_and_refuses_out_of_range() {
    let d = Dir::new("seed");
    let text = "seed must be an integer from 0 to 4294967295 (0 is stored as 1)";
    let mut e = Engine::default();
    let t = instrument(&mut e);
    for bad in [json!(4_294_967_296u64), json!(-1), json!(1.5), json!("7"), Value::Null] {
        refused(&mut e, "mix.sampler_new_patch", json!({"track": t, "seed": bad}), text);
    }
    let r = e.execute("mix.sampler_new_patch", &json!({"track": t, "seed": 0})).unwrap();
    assert_eq!((r["seed"].clone(), patch(&e, t).seed), (json!(1), 1));
    let r = e.execute("mix.sampler_seed", &json!({"track": t, "seed": 4_294_967_295u64})).unwrap();
    assert_eq!(r["seed"], 4_294_967_295u64);
    let r = e.execute("mix.sampler_seed", &json!({"track": t, "seed": 0})).unwrap();
    assert_eq!((r["seed"].clone(), patch(&e, t).seed), (json!(1), 1));
    refused(&mut e, "mix.sampler_seed", json!({"track": t, "seed": 4_294_967_296u64}), text);
    refused(&mut e, "mix.sampler_seed", json!({"track": t}), text);
    // A file whose seed is 0 is not rewritten on open; it plays as 1 and the query reports 1.
    edit_patch(&mut e, t, |p| p.seed = 0);
    let path = d.path("zero.transit");
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": path})).unwrap();
    assert_eq!(patch(&back, t).seed, 0, "opening writes nothing");
    assert_eq!(state(&mut back, t)["seed"], 1);
    let ins = back.session().track(t).unwrap().instrument.clone().unwrap();
    assert_eq!(ins.sampler_patch.as_ref().unwrap().seed(), 1, "the instance's seed");
    back.execute("session.save", &json!({})).unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let seeds: Vec<Value> =
        saved["tracks"].as_array().unwrap().iter().filter_map(|tr| tr["instrument"]["sampler_patch"].get("seed").cloned()).collect();
    assert_eq!(seeds, [json!(0)], "saved again as it was");
}

/// Quarter-note note 60s at velocity 127, one every quarter for `n` quarters.
fn notes(e: &mut Engine, t: TrackId, n: i64) {
    let q = TICKS_PER_QUARTER;
    let mut seq = Sequence::default();
    for k in 0..n {
        seq.notes.push(Note { pitch: 60, velocity: 127, release_velocity: 64, channel: 0, start: k * q, length: q / 2 });
    }
    let s = e.session_mut();
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 8, seq));
}

/// The level of each of `n` notes, 500 frames in (24,000 frames a quarter at 120 BPM).
fn levels(e: &Engine, n: usize) -> Vec<f32> {
    let out = transit_mix::render_range(e.session(), Range::new(0, 24_000 * n as i64), 512);
    (0..n).map(|k| out[0][k * 24_000 + 500]).collect()
}

/// Group 0 of three alternatives on constant sources (levels rising with the alternative), keys 60,
/// Gate, no envelope; a Random group 1 of four on keys 61; fades, a crossfade and the seed set.
fn round_robin_kit(e: &mut Engine) -> TrackId {
    let t = instrument(e);
    e.execute("mix.sampler_new_patch", &json!({"track": t, "seed": 777})).unwrap();
    let gate = json!({"mode": "gate", "attack_ms": 0, "release_ms": 0, "key_tracking": false});
    for a in 0..3u16 {
        let src = level(e, 0.1 * f32::from(a + 1));
        let mut playback = gate.clone();
        playback["keys"] = json!([60, 60]);
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": a + 1, "source": src, "playback": playback})).unwrap();
    }
    for a in 0..4u16 {
        let src = level(e, 0.05 * f32::from(a + 1));
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": a + 4, "source": src, "playback": {"keys": [61, 61]}})).unwrap();
    }
    let mut zones: Vec<Value> = (0..3u16).map(|a| member(a + 1, [60, 60], [1, 127], 0, 0, a as u8)).collect();
    zones.extend((0..4u16).map(|a| member(a + 4, [61, 61], [1, 127], 1, 0, a as u8)));
    zones[1]["fades"] = json!({"key_lo": 0, "key_hi": 0, "vel_lo": 2, "vel_hi": 0});
    e.execute("mix.sampler_zone_map", &json!({"track": t, "zones": zones})).unwrap();
    e.execute("mix.sampler_group_set", &json!({"track": t, "group": 1, "round_robin": "random"})).unwrap();
    let xf = json!({"track": t, "zone": 4, "region": {"loop_mode": "forward", "loop_start": 0, "loop_end": 1000, "crossfade": 120}, "playback": {"mode": "gate"}});
    e.execute("mix.sampler_zone_edit", &xf).unwrap();
    notes(e, t, 4);
    t
}

#[test]
fn round_robin_survives_save_and_load() {
    let d = Dir::new("rr");
    let mut e = Engine::default();
    let t = round_robin_kit(&mut e);
    let path = d.path("rr.transit");
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": path})).unwrap();
    let (a, b) = (patch(&e, t), patch(&back, t));
    assert_eq!(b, a, "modes, seed, memberships, fades and crossfade come back as saved");
    assert_eq!(
        (b.seed, b.groups[0].round_robin, b.zone(3).unwrap().crossfade, b.zone(1).unwrap().fades.widths()),
        (777, RoundRobin::Random, 120, [0, 0, 2, 0])
    );
    // The reopened session's counters start over: the sequential group plays alternatives 0, 1, 2,
    // 0 from the first note, as the session it was saved from does when it plays from the start.
    let (l, m) = (levels(&back, 4), levels(&e, 4));
    assert_eq!(l, m);
    assert!(l[0] < l[1] && l[1] < l[2] && l[3] == l[0], "0, 1, 2, 0: {l:?}");
}

#[test]
fn crossfade_fades_and_grid_fields_absent_at_defaults() {
    let mut e = Engine::default();
    let (t, src) = setup(&mut e);
    zone_on(&mut e, t, 1, src, 60);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": src})).unwrap();
    let v = serde_json::to_value(patch(&e, t)).unwrap();
    for key in ["crossfade", "fades", "membership"] {
        assert!(v["zones"][0].get(key).is_none(), "{key} is absent at its default: {v}");
    }
    assert!(v.get("groups").is_none() && v["loop"].get("grid_bpm").is_none(), "{v}");
    let p = json!({"track": t, "zone": 1, "region": {"loop_mode": "forward", "loop_start": 0, "loop_end": 1000, "crossfade": 3},
        "playback": {"fades": {"key_lo": 0, "key_hi": 0, "vel_lo": 1, "vel_hi": 0}, "membership": {"group": 9, "layer": 0, "alternative": 0}}});
    e.execute("mix.sampler_zone_edit", &p).unwrap();
    e.execute("mix.sampler_group_set", &json!({"track": t, "group": 9, "round_robin": "random"})).unwrap();
    edit_patch(&mut e, t, |p| p.slicing.as_mut().unwrap().grid_bpm = Some(96.0));
    let v = serde_json::to_value(patch(&e, t)).unwrap();
    assert_eq!((v["zones"][0]["crossfade"].clone(), v["zones"][0]["fades"]["vel_lo"].clone()), (json!(3), json!(1)));
    assert_eq!((v["groups"][0]["round_robin"].clone(), v["loop"]["grid_bpm"].clone()), (json!("random"), json!(96.0)));
    // Back to the defaults: absent again.
    let p = json!({"track": t, "zone": 1, "region": {"crossfade": 0}, "playback": {"fades": {"key_lo": 0, "key_hi": 0, "vel_lo": 0, "vel_hi": 0}, "membership": null}});
    e.execute("mix.sampler_zone_edit", &p).unwrap();
    e.execute("mix.sampler_group_set", &json!({"track": t, "group": 9, "round_robin": "sequential"})).unwrap();
    let v = serde_json::to_value(patch(&e, t)).unwrap();
    for key in ["crossfade", "fades", "membership"] {
        assert!(v["zones"][0].get(key).is_none(), "{key} is absent again: {v}");
    }
    assert!(v.get("groups").is_none());
}

#[test]
fn state_reports_stored_and_used_values() {
    let mut e = Engine::default();
    let (t, src) = sliced(&mut e, 3);
    // Zone 1: unbound forward loop of 300 frames, crossfade 100 (unclamped), fades 2/1 on keys
    // 40..50 (unclamped); zone 3 likewise, then clamped by later edits; zone 2 slice-bound.
    for (zone, key) in [(1u16, 40u8), (3, 40)] {
        let p = json!({"track": t, "zone": zone, "source": src, "region": {"loop_mode": "forward", "loop_start": 0, "loop_end": 300, "crossfade": 100},
            "playback": {"keys": [key, key + 10], "fades": {"key_lo": 2, "key_hi": 1, "vel_lo": 0, "vel_hi": 0}}});
        e.execute("mix.sampler_zone_source", &p).unwrap();
    }
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 3, "region": {"loop_end": 120}, "playback": {"keys": [40, 41]}})).unwrap();
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 2, "region": {"loop_mode": "forward"}})).unwrap();
    edit_patch(&mut e, t, |p| {
        p.seed = 0;
        p.zone_mut(0).unwrap().mode = PlayMode::OneShot;
    });
    let s = state(&mut e, t);
    assert_eq!(s["seed"], 1, "normalized");
    assert_eq!(s["patch"]["seed"], 0, "stored");
    let z = |n: u16| s["zones"].as_array().unwrap().iter().find(|z| z["zone"] == n).cloned().unwrap();
    // Zone 1 stores a forward loop but is a one-shot (a loaded file): loop_used off, no crossfade.
    assert_eq!(
        (z(1)["loop_mode"].clone(), z(1)["loop_used"].clone(), z(1)["crossfade"].clone(), z(1)["crossfade_used"].clone()),
        (json!("forward"), json!("off"), json!(100), json!(0))
    );
    assert_eq!(
        (z(1)["fades"].clone(), z(1)["fades_used"].clone()),
        (json!({"key_lo": 2, "key_hi": 1, "vel_lo": 0, "vel_hi": 0}), json!({"key_lo": 2, "key_hi": 1, "vel_lo": 0, "vel_hi": 0}))
    );
    // Zone 3: clamped crossfade (loop 120: 60 used) and fades (span 1: 1 and 0 used).
    assert_eq!((z(3)["loop_used"].clone(), z(3)["crossfade"].clone(), z(3)["crossfade_used"].clone()), (json!("forward"), json!(100), json!(60)));
    assert_eq!(z(3)["fades_used"], json!({"key_lo": 1, "key_hi": 0, "vel_lo": 0, "vel_hi": 0}));
    assert_eq!((z(3)["region"].clone(), z(3)["slice"].clone(), z(3)["membership"].clone()), (Value::Null, Value::Null, Value::Null));
    // Zone 2: slice-bound, its derived region and loop.
    assert_eq!(z(2)["slice"], json!({"set": "manual", "id": patch(&e, t).zone(1).unwrap().slice.as_ref().unwrap().id, "slice": 3}));
    assert_eq!(z(2)["region"], json!({"source": 0, "start": 10_001, "end": 48_000, "loop_start": 10_001, "loop_end": 48_000}));
    assert_eq!(z(2)["loop_used"], "forward");
    assert_eq!(s["groups"], json!([]));
}

/// The FNV-1a digest of the stored patch's JSON text and the render's bits.
fn digest(e: &Engine, t: TrackId, frames: i64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |b: &[u8]| {
        for x in b {
            h = (h ^ u64::from(*x)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(serde_json::to_string(&patch(e, t)).unwrap().as_bytes());
    for c in transit_mix::render_range(e.session(), Range::new(0, frames), 512) {
        for x in c {
            eat(&x.to_bits().to_le_bytes());
        }
    }
    h
}

/// Integer noise for a golden: a 32-bit LCG, its top 24 bits as a signed fraction at half scale.
/// Every step (integer arithmetic, a 24-bit integer to f32, division by a power of two) is exact
/// on every platform, unlike `f32::sin`, which the platform C runtime computes.
fn noise(n: usize) -> Vec<f32> {
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((x >> 8) as i32 - (1 << 23)) as f32 / (1 << 24) as f32
        })
        .collect()
}

/// The digest of a kit built with the zone commands' old keys only, captured on the base commit
/// (before this slice) in its own fresh target. The render path is IEEE-exact arithmetic on every
/// platform: an integer-noise source at the session rate, no key tracking and no transpose (the
/// step is exactly 1), 0 dB, no shaper or matrix, linear envelopes and centered pan.
const OLD_CALLERS_GOLDEN: u64 = 0xd316_b9a8_606a_cb26;

#[test]
fn zone_edit_without_the_new_keys_is_unchanged() {
    let mut e = Engine::default();
    let t = instrument(&mut e);
    e.execute("mix.sampler_new_patch", &json!({"track": t, "seed": 9})).unwrap();
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    let n = noise(24_000);
    let src = source_with(&mut e, 48_000, 24_000, |i| n[i]);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "source": src})).unwrap();
    e.execute(
        "mix.sampler_zone_source",
        &json!({"track": t, "zone": 2, "source": src, "playback": {"keys": [70, 80], "root": 72, "key_tracking": false}}),
    )
    .unwrap();
    let r = e
        .execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 2, "region": {"loop_mode": "forward", "loop_start": 100, "loop_end": 20_000}, "playback": {"velocity_sensitivity": 0.5, "release_ms": 50}}))
        .unwrap();
    assert_eq!((r["track"].clone(), r["zone"].clone()), (json!(t), json!(2)), "old callers read the same keys");
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 1, "playback": {"attack_ms": 3}})).unwrap();
    let q = TICKS_PER_QUARTER;
    let mut seq = Sequence::default();
    for (pitch, start) in [(60u8, 0i64), (72, 1), (75, 2)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: start * q, length: q });
    }
    let s = e.session_mut();
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 4, seq));
    let d = digest(&e, t, 96_000);
    println!("OLD_CALLERS_DIGEST={d:#018x}");
    assert_eq!(d, OLD_CALLERS_GOLDEN, "an old caller's patch and render are bit-identical to the base's");
}
