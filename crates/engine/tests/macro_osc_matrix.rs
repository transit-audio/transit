//! The Macro Oscillator's shaper and modulation matrix in a session (#243): off by default and the
//! same bits as before them; an old session's note-to-macro mapping migrates to matrix rows and
//! renders the same bits as the build that saved it; the rewritten factory preset too; the shaper
//! and rows save, load and render the same at any host block size; a rack member's matrix reads
//! the rack's macros.
//!
//! The golden hashes were taken from the build before #243 (main f2feabf0): the fixtures in
//! `tests/fixtures` are sessions that build saved, rendered by it over 120 000 frames.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use transit_engine::Engine;
use transit_model::{Session, TrackId};
use transit_time::Range;

const ID: &str = "macro_oscillator";

/// FNV-1a over the render's f32 bits, channel after channel.
fn fnv(a: &[Vec<f32>]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in a.iter().flatten() {
        for b in v.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    h
}

fn render(s: &Session, block: usize) -> Vec<Vec<f32>> {
    transit_mix::render_range(s, Range::new(0, 120_000), block)
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The note-to-macro fixture: two Macro Oscillators (a three-voice one with eight depths and both
/// curves, and a mono legato one), lanes on two depths and a curve's neighbor, rendered at 512 and
/// at 64 frames per block by the build before #243.
const MAPPING_GOLDEN_512: u64 = 0x0369_c012_d361_e065;
const MAPPING_GOLDEN_64: u64 = 0xbe79_399a_b815_d989;
/// The plain fixture (defaults, a poly and a mono track), at 512 and 37 frames per block.
const PLAIN_GOLDEN: u64 = 0x6b24_dd39_e004_0ab9;
/// The "Key Tracked Timbre" factory preset as it was (`key_timbre` 50) through [`preset_render`].
const KEY_TRACKED_TIMBRE_GOLDEN: u64 = 0xf9b8_411d_79c8_1dd1;

#[test]
fn an_old_session_migrates_its_mapping_to_rows_and_renders_the_same_bits() {
    let text = fixture("macro_osc_note_mapping_v0_5.transit");
    assert!(text.contains("velocity_timbre"), "the fixture is in the old format");
    let (s, stored) = Session::from_json_as_saved(&text).unwrap();
    assert!(stored.is_some(), "a migrated session reads as modified");
    for t in &s.tracks {
        let ins = t.instrument.as_ref().unwrap();
        assert!(!ins.params.keys().any(|k| transit_model::legacy_mapping::is_old_param(k)), "{:?}", ins.params.keys());
        assert!(ins.params.contains_key("mod1_source"), "rows were written");
        for l in &t.automation {
            let label = l.param.label();
            assert!(!label.contains("velocity_") && !label.contains("key_"), "lane {label} retargeted");
        }
    }
    assert!(s.load_notes.iter().any(|n| n.contains(transit_model::legacy_mapping::MIGRATED_NOTE)), "{:?}", s.load_notes);
    assert_eq!(fnv(&render(&s, 512)), MAPPING_GOLDEN_512, "512-frame blocks: the bits of the build before #243");
    assert_eq!(fnv(&render(&s, 64)), MAPPING_GOLDEN_64, "64-frame blocks: the bits of the build before #243");
    // Saved again and reloaded: the rows are ordinary parameters now.
    let again = Session::from_json(&s.to_json().unwrap()).unwrap();
    assert_eq!(fnv(&render(&again, 512)), MAPPING_GOLDEN_512);
}

/// Off by default: a session of defaults plays the bits of the build before #243, whether the
/// shaper's and the matrix's parameters are absent from the document (an old file) or present at
/// their defaults (a new one); and at any host block size.
#[test]
fn off_by_default_the_render_is_the_build_before_243() {
    let old = Session::from_json(&fixture("macro_osc_plain_v0_5.transit")).unwrap();
    assert!(!old.tracks[0].instrument.as_ref().unwrap().params.contains_key("shaper"), "the flag is absent from the old file");
    assert_eq!(fnv(&render(&old, 512)), PLAIN_GOLDEN);
    assert_eq!(fnv(&render(&old, 37)), PLAIN_GOLDEN);
    let mut present = old.clone();
    let info = transit_dsp::plugin_info(ID).unwrap();
    for t in &mut present.tracks {
        let ins = t.instrument.as_mut().unwrap();
        for p in info.params {
            ins.params.entry(p.id.to_string()).or_insert(p.default);
        }
        assert_eq!(ins.params.get("shaper"), Some(&0.0), "the flag is present, off");
    }
    assert_eq!(fnv(&render(&present, 512)), PLAIN_GOLDEN, "the flag present and off: the same bits");
    let mut on = present.clone();
    on.tracks[0].instrument.as_mut().unwrap().params.insert("shaper".into(), 1.0);
    assert_ne!(fnv(&render(&on, 512)), PLAIN_GOLDEN, "the shaper on changes the render");
}

/// What the "Key Tracked Timbre" preset's parameters play: notes over five octaves at varied
/// velocities, 300 blocks of 256 frames, both channels.
fn preset_render(params: &[(String, f32)]) -> u64 {
    let mut m = transit_dsp::create(ID).unwrap();
    for (k, v) in params {
        assert!(m.set_param(k, *v), "{k}");
    }
    m.prepare(48_000.0, 256, 2);
    let mut io = vec![vec![0.0f32; 256]; 2];
    let mut all = vec![Vec::new(), Vec::new()];
    for b in 0..300usize {
        let key = 24 + (b / 20 * 7 % 80) as u8;
        if b % 20 == 0 {
            m.note_on(b % 7, key, 20 + (b * 13 % 107) as u8);
        }
        if b % 20 == 13 {
            m.note_off(3, key);
        }
        m.process(&mut io, 256);
        for c in 0..2 {
            all[c].extend_from_slice(&io[c]);
        }
    }
    fnv(&all)
}

/// The factory presets carry rows, not the old ids, and the one that used the mapping plays as
/// before.
#[test]
fn the_rewritten_preset_plays_as_before() {
    let dir = format!("{}/../../assets/presets/macro_oscillator", env!("CARGO_MANIFEST_DIR"));
    for e in std::fs::read_dir(&dir).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        for k in v["params"].as_object().unwrap().keys() {
            assert!(!transit_model::legacy_mapping::is_old_param(k), "{k}");
            assert!(transit_dsp::plugin_info(ID).unwrap().param(k).is_some(), "{k} is a parameter");
        }
    }
    let text = std::fs::read_to_string(format!("{dir}/Key Tracked Timbre.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let params: Vec<(String, f32)> = v["params"].as_object().unwrap().iter().map(|(k, x)| (k.clone(), x.as_f64().unwrap() as f32)).collect();
    assert_eq!(preset_render(&params), KEY_TRACKED_TIMBRE_GOLDEN);
}

/// The model's migration writes the matrix's own indices: Velocity and Key, Harmonics .. Decay.
#[test]
fn the_migration_constants_name_the_matrix_choices() {
    use transit_dsp::plugins::macro_osc::matrix::{DESTINATIONS, ROWS, SOURCES};
    use transit_model::legacy_mapping as lm;
    assert_eq!(SOURCES[lm::SOURCE_VELOCITY as usize], "Velocity");
    assert_eq!(SOURCES[lm::SOURCE_KEY as usize], "Key");
    let dests: Vec<&str> = lm::DEST_MACROS.iter().map(|d| DESTINATIONS[*d as usize]).collect();
    assert_eq!(dests, ["Harmonics", "Timbre", "Morph", "Decay"]);
    assert_eq!(lm::ROWS, ROWS);
    for n in 1..=ROWS {
        for f in ["source", "via", "dest", "amount", "curve"] {
            assert!(transit_dsp::plugin_info(ID).unwrap().param(&lm::row_id(n, f)).is_some());
        }
    }
}

fn engine_with(set: &[(&str, f32)]) -> (Engine, TrackId) {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Macro"})).unwrap();
    let track = r["tracks"][0].as_u64().unwrap();
    e.execute("mix.instrument", &json!({"track": track, "plugin": ID})).unwrap();
    for (p, v) in set {
        e.execute("mix.instrument_param", &json!({"track": track, "param": p, "value": v})).unwrap();
    }
    let r = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96_000})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    for (pitch, start, len, vel) in [(48, 0, 900, 90), (55, 240, 900, 60), (67, 960, 1500, 120), (72, 1920, 600, 30), (36, 2400, 900, 100)] {
        e.execute("event.midi_note", &json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": len, "velocity": vel})).unwrap();
    }
    (e, TrackId(track))
}

/// The shaper on with every LFO shape, a synced LFO, and all sixteen rows (every destination, vias
/// and curves): the render is the same bits at any host block size and after a save and load.
const FULL: &[(&str, f32)] = &[
    ("voices", 7.0),
    ("shaper", 1.0),
    ("filter_mode", 0.0),
    ("filter_slope", 1.0),
    ("cutoff", 1200.0),
    ("resonance", 40.0),
    ("drive", 6.0),
    ("key_track", 50.0),
    ("filter_env_amount", 40.0),
    ("amp_release", 150.0),
    ("lfo1_shape", 5.0),
    ("lfo1_rate", 3.0),
    ("lfo1_fade", 200.0),
    ("lfo2_shape", 4.0),
    ("lfo2_sync", 1.0),
    ("lfo2_retrigger", 0.0),
    ("lfo2_division", 5.0),
];

fn full_rows() -> Vec<(String, f32)> {
    // (source, via, destination, amount, curve) for each of the sixteen rows.
    let rows: [(f32, f32, f32, f32, f32); 16] = [
        (5.0, 0.0, 2.0, 30.0, 1.0),
        (6.0, 0.0, 3.0, -40.0, 2.0),
        (1.0, 10.0, 1.0, 5.0, 0.0),
        (2.0, 0.0, 4.0, 25.0, 0.0),
        (3.0, 0.0, 5.0, 10.0, 3.0),
        (4.0, 0.0, 6.0, -20.0, 4.0),
        (12.0, 0.0, 14.0, 60.0, 0.0),
        (1.0, 0.0, 7.0, 30.0, 0.0),
        (11.0, 0.0, 8.0, 20.0, 0.0),
        (5.0, 0.0, 9.0, 50.0, 0.0),
        (10.0, 0.0, 10.0, 40.0, 0.0),
        (8.0, 0.0, 11.0, 40.0, 0.0),
        (9.0, 0.0, 12.0, -30.0, 0.0),
        (7.0, 0.0, 13.0, -30.0, 0.0),
        (13.0, 5.0, 3.0, 20.0, 2.0),
        (2.0, 1.0, 7.0, 15.0, 1.0),
    ];
    let mut out = Vec::new();
    for (i, (s, v, d, a, c)) in rows.iter().enumerate() {
        let n = i + 1;
        out.extend([
            (format!("mod{n}_source"), *s),
            (format!("mod{n}_via"), *v),
            (format!("mod{n}_dest"), *d),
            (format!("mod{n}_amount"), *a),
            (format!("mod{n}_curve"), *c),
        ]);
    }
    out
}

#[test]
fn the_shaper_and_a_full_matrix_render_the_same_at_any_block_size_and_after_save_and_load() {
    let (mut e, t) = engine_with(FULL);
    for (p, v) in full_rows() {
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": p, "value": v})).unwrap();
    }
    let reference = render(e.session(), 512);
    assert!(reference[0].iter().any(|v| v.abs() > 0.01), "it sounds");
    assert!(reference[0] != reference[1], "a Pan row makes the voices stereo");
    for block in [64, 37, 1000] {
        assert!(fnv(&render(e.session(), block)) == fnv(&reference), "{block}-frame blocks differ");
    }
    let loaded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(fnv(&render(&loaded, 512)), fnv(&reference), "the loaded session renders the same bits");
    let (plain, _) = engine_with(&[("voices", 7.0)]);
    assert_ne!(fnv(&render(plain.session(), 512)), fnv(&reference));
}

/// Every row parameter is an automation target addressed by its stable id.
#[test]
fn rows_are_automation_targets() {
    let (mut e, t) = engine_with(&[("mod1_source", 6.0), ("mod1_dest", 3.0)]);
    let plain = render(e.session(), 512);
    let mut lane = transit_model::AutomationLane::new(transit_model::AutoParam::parse("plugin:instrument:mod1_amount").unwrap());
    lane.set_point(0, -100.0);
    lane.set_point(96_000, 100.0);
    let track = e.session_mut().track_mut(t).unwrap();
    track.mixer.automation_mode = transit_model::AutomationMode::Read;
    track.automation.push(lane);
    assert_ne!(fnv(&render(e.session(), 512)), fnv(&plain), "a row's amount lane changes the render");
}

/// A rack member's matrix reads the rack's macros: a Macro 1 row moves the sound with the macro's
/// knob; the same instrument outside a rack reads 0, as if the row's amount were 0.
#[test]
fn a_rack_members_matrix_reads_the_rack_macros() {
    let row = [("mod1_source", 13.0), ("mod1_dest", 3.0), ("mod1_amount", 100.0), ("timbre", 0.0)];
    let (mut e, t) = engine_with(&row);
    let (silent_row, _) = engine_with(&[("mod1_source", 13.0), ("mod1_dest", 3.0), ("mod1_amount", 0.0), ("timbre", 0.0)]);
    assert_eq!(fnv(&render(e.session(), 512)), fnv(&render(silent_row.session(), 512)), "outside a rack the macros read 0");
    let folder = {
        let s = e.session_mut();
        let f = s.add_track(transit_model::TrackKind::Folder, transit_model::ChannelFormat::Stereo, Some("Rack"));
        s.track_mut(t).unwrap().folder = Some(f);
        let rack = transit_model::Rack { members: vec![transit_model::RackMember::new(t)], ..transit_model::Rack::default() };
        s.track_mut(f).unwrap().rack = Some(rack);
        f
    };
    let at_zero = fnv(&render(e.session(), 512));
    e.session_mut().track_mut(folder).unwrap().rack.as_mut().unwrap().macros[0].value = 1.0;
    let at_one = fnv(&render(e.session(), 512));
    assert_ne!(at_one, at_zero, "the macro at 1 opens Timbre");
}
