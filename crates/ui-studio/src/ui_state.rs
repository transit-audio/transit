//! Per-machine UI preferences.

use super::*;

/// Our own UI state (serde, so agents can read and set it; saved as `ui.json` in the transit
/// settings folder). Not `transit-ui-egui`'s `UiState` (plan 06, L13).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiState {
    pub low_latency: bool,
    #[serde(deserialize_with = "low_latency::deserialize_limit")]
    pub low_latency_limit_ms: f64,
    /// The audio device choice, kept with the preferences: per machine.
    pub audio: transit_playback::AudioConfig,
    /// The main bus monitoring set (#366): Mono, Dim, Ref, the monitor level and the reference
    /// level. Per machine, with the audio settings; never in the session or its undo history.
    pub monitor: main_monitor::MonitorPrefs,
    /// The Audio Devices window's last section (`audio_devices::Section::key`): per machine, in
    /// `ui.json`, never in the session.
    pub audio_devices_section: String,
    /// This machine's I/O paths, one table per audio system plus device (ADR 0015).
    pub io_maps: Vec<transit_model::IoMap>,
    /// File > Open Recent: sessions last opened or saved here, most recent first (at most 10).
    /// A per-machine preference; never part of the document or its undo history.
    pub recent_files: Vec<recent::RecentFile>,
    /// The folder a session was last saved to or opened from here (#385): where Save As, Open and
    /// Bounce start for a session that has no file yet. Per machine; `null` until the first save
    /// or open, and older files without it load as `None`. Written even when `None`: the app's
    /// loader keeps only fields the default state names (`config::decode_ui`).
    pub last_session_dir: Option<std::path::PathBuf>,
    /// The user's own VST3 and CLAP folders.
    pub plugin_dirs: plugin_scan::PluginDirs,
    /// Window > Plug-in Windows Follow transit (issue #173): editors hide, return and come to the
    /// front with the main window and have no taskbar button. Off: the behavior before #173.
    pub editors_follow: bool,
    /// Plug-in editors to have open, as (track, slot); the instrument is
    /// `transit_mix::INSTRUMENT_SLOT`. `ui.set` opens and closes editors through this list
    /// (the same field `tools/exp-quit.ps1` sets in SoundCraft).
    pub plugin_windows: Vec<(TrackId, usize)>,
    /// A migrated device window's last closed position (its top left, logical points) by device id
    /// (the plug-in id, 326-M; position only since 326-X, `crate::device_window`): where an
    /// instance never opened before opens (cascaded when it would land on an open window). An
    /// older entry's size is read past. Per machine, never in the session.
    #[serde(deserialize_with = "crate::device_window::read_device_positions")]
    pub device_windows: crate::device_window::DevicePositions,
    /// A device window's collapsed section bars by `<device id>/<section>` (326-M; the design
    /// language's section bar: collapse is a per-user view setting, enable is the parameter). A
    /// section without an entry is collapsed until it is in use.
    pub device_sections: std::collections::BTreeMap<String, bool>,
    /// Device windows that follow the selection (326-M, the title bar's link toggle), as their
    /// current (track, slot): such a window shows the focused track's device in the same slot.
    pub editor_follow: Vec<(TrackId, usize)>,
    /// How long each plug-in type took to load the last time, in milliseconds by plug-in id (issue
    /// #67): "last time 12 s" beside the loading readout. A per-machine preference, updated on
    /// every successful load; older files have none.
    pub plugin_load_ms: std::collections::BTreeMap<String, u64>,
    /// Width of the track list.
    pub header_width: f32,
    /// The main window's size (logical points) and maximized state.
    pub window_size: [f32; 2],
    pub window_maximized: bool,
    /// The mixer as a pane under the arrangement (Ableton's Ctrl+Alt+M).
    pub show_mixer: bool,
    /// The device chain of the focused track under the arrangement (Ableton's detail view, Ctrl+Alt+L).
    pub show_devices: bool,
    /// Window > MIDI Monitor: open, filters, size (#91).
    pub midi_monitor: midi_monitor::Prefs,
    /// Window > MIDI Mappings: open, size (#97).
    pub midi_mappings: midi_learn::Prefs,
    /// The controller profile: this machine's MIDI Learn mappings, keyed by port name (ADR 0028).
    /// Per machine; never part of a session.
    pub midi_profile: transit_model::MappingSet,
    /// Transport button settings and assignments by exact device name, never session data.
    pub midi_transport: transit_model::TransportProfiles,
    /// How a mapped knob takes over a control it is not at: scaled, jump or pickup (ADR 0028,
    /// amendment 2026-10-25). Per machine, with the profile; one setting for every mapping.
    #[serde(default = "transit_model::MapMode::takeover_default")]
    pub midi_takeover: transit_model::MapMode,
    /// The mixer fills the window instead of the arrangement (Cubase's F3).
    pub mixer_full: bool,
    /// Height of the mixer pane in points. A saved 420, the default before the short strip's
    /// board travel (#326), reads as the current default.
    #[serde(deserialize_with = "pane_height")]
    pub mixer_height: f32,
    /// The edit cursor: where Play starts and Paste lands. View state of the open session, not
    /// part of the document (setting it never dirties it) and not saved with the preferences.
    #[serde(skip)]
    pub cursor: Samples,
    /// The track the cursor was last set on (paste target).
    #[serde(skip)]
    pub cursor_track: Option<TrackId>,
    /// The focused track: the one the device chain shows and the mixer and track list highlight.
    /// Follows clicks in both views. View state, never in the document.
    #[serde(skip)]
    pub focus_track: Option<TrackId>,
    /// The piano roll's view preferences (slice 3).
    pub keys: keys::KeysPrefs,
    /// Collapsed groups (slice 2c): their members are out of the track list and the mixer. View
    /// state of the open session, never in the document; shown by `ui.inspect`, never loaded.
    #[serde(skip_deserializing)]
    pub collapsed: Vec<TrackId>,
    /// The browser's view preferences (slice 4): shown, width, tab, locations, favorites.
    pub browser: browser::BrowserPrefs,
    /// View > Appearance (slice 5): System follows the OS; Dark and Light pin the theme.
    pub appearance: theme::Appearance,
    /// Whole-interface zoom, composed with the OS display scale and saved per machine.
    #[serde(deserialize_with = "interface_zoom::deserialize")]
    pub ui_zoom: f32,
    /// Options > Middle C (ADR 0051): the name MIDI 60 carries in every transit window, `"C3"`
    /// (the default; a missing key reads it) or `"C4"`. Per machine; never in the session or its
    /// undo history, and never read by a command's parser.
    pub middle_c: transit_midi::MiddleC,
    /// Help > Keyboard Shortcuts is open, and its filter (view state, not saved).
    #[serde(skip)]
    pub show_shortcuts: bool,
    #[serde(skip)]
    pub shortcuts_filter: String,
    /// Transport > Count-in: Record clicks `count_in_bars` bars before it starts recording, with
    /// the playhead parked at the start (no room before bar 1 needed). A preference, not part of
    /// the document.
    pub count_in: bool,
    /// How many bars the count-in lasts when it is on: 1, 2 or 4 (the metronome popup).
    pub count_in_bars: u8,
    /// The click's level in dB on top of Setup > Click/Countoff, from the metronome popup. A
    /// preference: it reaches the mixer without touching the document or its undo history.
    pub metronome_db: f32,
    /// Automation > Thin after write (follow-ups): when a write pass ends, `automation.thin`
    /// runs on the tracks it wrote, as part of the pass's undo step. A preference.
    pub thin_after_write: bool,
    /// Help > Check for New Versions on Launch: at most once a day, one anonymous request to
    /// GitHub's public API (see [`update`]). A preference.
    pub check_for_updates: bool,
    /// When the last update check started (seconds since the Unix epoch; 0: never).
    pub last_update_check: u64,
    /// Tracks whose take lanes are shown (slice 6). View state of the open session, never in the
    /// document; the lanes it opens are in `ui.inspect`'s slice 6 layout.
    #[serde(skip)]
    pub takes_open: Vec<TrackId>,
    /// MIDI effect parameter sections opened or closed from their default (the first open): track,
    /// effect index, section (view state, not saved).
    #[serde(skip)]
    pub midi_fx_sections: Vec<(TrackId, usize, String)>,
    /// Edit > Undo History is open (view state, not saved).
    #[serde(skip)]
    pub show_history: bool,
    /// Tracks whose automation lanes are shown (slice 7, the A toggle), and the lanes open on
    /// them with their heights. View state of the open session, never in the document; the lanes
    /// as drawn are in `ui.inspect`'s `automation`.
    #[serde(skip)]
    pub auto_shown: Vec<TrackId>,
    #[serde(skip)]
    pub auto_lanes: Vec<automation::LaneView>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            low_latency: false,
            low_latency_limit_ms: 0.0,
            audio: transit_playback::AudioConfig::default(),
            monitor: main_monitor::MonitorPrefs::default(),
            audio_devices_section: String::new(),
            io_maps: Vec::new(),
            recent_files: Vec::new(),
            last_session_dir: None,
            plugin_dirs: plugin_scan::PluginDirs::default(),
            editors_follow: true,
            plugin_windows: Vec::new(),
            device_windows: std::collections::BTreeMap::new(),
            device_sections: std::collections::BTreeMap::new(),
            editor_follow: Vec::new(),
            plugin_load_ms: std::collections::BTreeMap::new(),
            header_width: 236.0,
            window_size: [1600.0, 1000.0],
            window_maximized: false,
            show_mixer: false,
            show_devices: true,
            midi_monitor: midi_monitor::Prefs::default(),
            midi_mappings: midi_learn::Prefs::default(),
            midi_profile: Default::default(),
            midi_transport: Default::default(),
            midi_takeover: transit_model::MapMode::takeover_default(),
            mixer_full: false,
            mixer_height: crate::mixer::default_pane_height(),
            cursor: 0,
            cursor_track: None,
            focus_track: None,
            keys: keys::KeysPrefs::default(),
            collapsed: Vec::new(),
            browser: browser::BrowserPrefs::default(),
            appearance: theme::Appearance::System,
            ui_zoom: 1.0,
            middle_c: transit_midi::MiddleC::C3,
            show_shortcuts: false,
            shortcuts_filter: String::new(),
            count_in: false,
            count_in_bars: 1,
            metronome_db: 0.0,
            thin_after_write: true,
            check_for_updates: true,
            last_update_check: 0,
            takes_open: Vec::new(),
            midi_fx_sections: Vec::new(),
            show_history: false,
            auto_shown: Vec::new(),
            auto_lanes: Vec::new(),
        }
    }
}

/// The docked pane's earlier defaults (pane heights nothing else ever wrote): 420 before #326,
/// and 475 before the main bus shared the channels' frame (the Fader construction board), which
/// gave the row the main bus footer's room.
const OLD_PANE_DEFAULTS: [f32; 2] = [420.0, 475.0];

fn pane_height<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let h = <f32 as serde::Deserialize>::deserialize(d)?;
    Ok(if OLD_PANE_DEFAULTS.contains(&h) { crate::mixer::default_pane_height() } else { h })
}

#[cfg(test)]
mod height_tests {
    use super::UiState;
    use serde_json::json;

    #[test]
    fn retired_track_height_is_ignored_and_omitted_on_save() {
        let without: UiState = serde_json::from_value(json!({})).unwrap();
        let saved = serde_json::to_value(&without).unwrap();
        assert!(saved.get("track_height").is_none());
        for height in [json!(46), json!(200), json!("invalid")] {
            let old: UiState = serde_json::from_value(json!({"track_height":height})).unwrap();
            assert_eq!(serde_json::to_value(old).unwrap(), saved);
        }
    }

    #[test]
    fn the_old_default_pane_height_reads_as_the_new_one_and_a_chosen_height_stays() {
        let new = crate::mixer::default_pane_height();
        for h in [420.0, 475.0] {
            let old: UiState = serde_json::from_value(json!({"mixer_height": h})).unwrap();
            assert_eq!(old.mixer_height, new, "{h}");
        }
        // Negative control: any other saved height is the user's and stays.
        let chosen: UiState = serde_json::from_value(json!({"mixer_height": 500.0})).unwrap();
        assert_eq!(chosen.mixer_height, 500.0);
        assert_eq!(UiState::default().mixer_height, new);
    }
}

/// The `ui.json` round trip (permanent since the pane toggles: the derive is plain again and no
/// load-time rule rewrites a field). Every leaf of the saved state is perturbed in turn; each
/// value that loads saves back unchanged and is a byte-identical fixed point, and a file with
/// every accepted perturbation and filled collections is one too.
#[cfg(test)]
mod round_trip_tests {
    use super::UiState;
    use serde_json::{Value, json};

    /// Fields that normalize on load by design: the old default pane height and the clamped ones.
    const NORMALIZED: [&str; 0] = [];

    fn load(v: &Value) -> Result<UiState, serde_json::Error> {
        serde_json::from_value(v.clone())
    }

    fn save(ui: &UiState) -> Value {
        serde_json::to_value(ui).unwrap()
    }

    /// The text saved, loaded and saved again must be the same bytes.
    fn assert_fixed_point(v: &Value, what: &str) {
        let text = serde_json::to_string_pretty(&save(&load(v).unwrap())).unwrap();
        let again = serde_json::to_string_pretty(&save(&serde_json::from_str::<UiState>(&text).unwrap())).unwrap();
        assert_eq!(text, again, "{what}: not a fixed point");
    }

    fn leaves(v: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match v {
            Value::Object(m) => {
                for (k, c) in m {
                    path.push(k.clone());
                    leaves(c, path, out);
                    path.pop();
                }
            }
            Value::Array(a) if !a.is_empty() => {
                for (i, c) in a.iter().enumerate() {
                    path.push(i.to_string());
                    leaves(c, path, out);
                    path.pop();
                }
            }
            _ => out.push(path.clone()),
        }
    }

    fn at<'a>(v: &'a mut Value, path: &[String]) -> &'a mut Value {
        path.iter().fold(v, |v, k| match v {
            Value::Array(a) => &mut a[k.parse::<usize>().unwrap()],
            _ => &mut v[k.as_str()],
        })
    }

    fn candidates(v: &Value) -> Vec<Value> {
        match v {
            Value::Bool(b) => vec![json!(!b)],
            Value::Number(n) if n.is_u64() => vec![json!(n.as_u64().unwrap() + 1), json!(2)],
            Value::Number(n) if n.is_i64() => vec![json!(n.as_i64().unwrap() + 1), json!(2)],
            Value::Number(n) => {
                let f = n.as_f64().unwrap();
                vec![json!(f + 1.0), json!(f * 0.75), json!(1.5)]
            }
            Value::String(_) => ["x", "dark", "light", "jump", "pickup", "C4"].iter().map(|s| json!(s)).collect(),
            Value::Null => vec![json!(1), json!("x")],
            Value::Array(_) | Value::Object(_) => Vec::new(),
        }
    }

    /// The default state with every collection given an entry, so their leaves are walked too.
    fn filled() -> Value {
        // The controller profile and the transport buttons, learned the way a user does.
        let mut app = crate::tests::app();
        app.run("track.new", json!({"kind": "instrument"})).unwrap();
        app.midi_learn_feed(0.0);
        let source = json!({"port": "KeyStep", "channel": 2, "kind": "cc", "number": 21});
        app.run("midi.map", json!({"source": source, "target": "strip:1:volume", "scope": "profile"})).unwrap();
        let button = json!({"port": "Controller", "kind": "cc", "number": 20, "channel": 1});
        app.run("midi.map", json!({"target": "transport:cycle", "scope": "profile", "source": button})).unwrap();
        app.midi_learn_feed(0.0);
        let mut ui = UiState { midi_profile: app.engine.midi_profile().clone(), midi_transport: app.ui.midi_transport.clone(), ..UiState::default() };
        assert!(!ui.midi_profile.is_empty(), "the controller profile is filled");
        assert!(!ui.midi_transport.is_empty(), "the transport buttons are filled");
        ui.plugin_dirs.vst3.push("D:/VST3".into());
        ui.plugin_dirs.clap.push("D:/CLAP".into());
        ui.io_maps.push(transit_model::IoMap::for_device("WASAPI", "Speakers", 2, 2));
        ui.recent_files.push(super::recent::RecentFile { path: "C:/a/b.transit".into(), opened_at: 7 });
        ui.last_session_dir = Some("C:/a".into());
        let mut v = save(&ui);
        v["plugin_windows"] = json!([[3, 1]]);
        v["editor_follow"] = json!([[4, 2]]);
        v["device_windows"] = json!({"plaits": [10.0, 20.0]});
        v["device_sections"] = json!({"plaits/env": true});
        v["plugin_load_ms"] = json!({"vst3:abc": 1200});
        let loaded = save(&load(&v).unwrap());
        assert_eq!(loaded, v, "the filled collections load and save unchanged");
        v
    }

    #[test]
    fn every_field_survives_save_and_load_unchanged() {
        let base = filled();
        assert_fixed_point(&json!({}), "an empty file");
        assert_fixed_point(&base, "the filled default");
        let mut paths = Vec::new();
        leaves(&base, &mut Vec::new(), &mut paths);
        let mut all = base.clone();
        let mut covered = std::collections::BTreeSet::new();
        for path in &paths {
            let old = at(&mut base.clone(), path).clone();
            for c in candidates(&old) {
                let mut v = base.clone();
                *at(&mut v, path) = c.clone();
                let Ok(ui) = load(&v) else { continue };
                let saved = save(&ui);
                let what = path.join(".");
                if !NORMALIZED.contains(&what.as_str()) {
                    assert_eq!(at(&mut saved.clone(), path), &c, "{what} saves back what was loaded");
                }
                assert_fixed_point(&v, &what);
                if c != old {
                    covered.insert(path[0].clone());
                    *at(&mut all, path) = c;
                    break;
                }
            }
        }
        assert_fixed_point(&all, "every perturbation at once");
        let all_saved = save(&load(&all).unwrap());
        assert_eq!(all_saved, all, "the all-perturbed file loads and saves unchanged");
        // `collapsed` is shown by `ui.inspect` and never loaded (`skip_deserializing`).
        let keys: Vec<&String> = base.as_object().unwrap().keys().filter(|k| !covered.contains(*k) && *k != "collapsed").collect();
        assert!(keys.is_empty(), "fields with no perturbation that loads: {keys:?}");
        // The two lower panes are independent: both on stays both on.
        let mut both = base.clone();
        both["show_mixer"] = json!(true);
        both["show_devices"] = json!(true);
        assert_eq!(save(&load(&both).unwrap()), both);
        // An unknown field is ignored and a missing newer one takes its default, as before.
        let mut extra = base.clone();
        extra["no_such_field"] = json!(1);
        assert_eq!(save(&load(&extra).unwrap()), base);
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove("check_for_updates");
        assert_eq!(save(&load(&missing).unwrap()), base);
        // ADR 0051: a file without `middle_c` reads C3, and the key is saved as its label.
        let mut older = base.clone();
        older.as_object_mut().unwrap().remove("middle_c");
        assert_eq!(load(&older).unwrap().middle_c, transit_midi::MiddleC::C3);
        assert_eq!(base["middle_c"], json!("C3"));
        let mut c4 = base.clone();
        c4["middle_c"] = json!("C4");
        assert_eq!(load(&c4).unwrap().middle_c, transit_midi::MiddleC::C4);
        c4["middle_c"] = json!("C5");
        assert!(load(&c4).is_err(), "a third convention does not load");
    }
}
