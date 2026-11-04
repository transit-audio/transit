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
    /// The Audio Devices window's last section (`audio_devices::Section::key`): per machine, in
    /// `ui.json`, never in the session.
    pub audio_devices_section: String,
    /// This machine's I/O paths, one table per audio system plus device (ADR 0015).
    pub io_maps: Vec<transit_model::IoMap>,
    /// File > Open Recent: sessions last opened or saved here, most recent first (at most 10).
    /// A per-machine preference; never part of the document or its undo history.
    pub recent_files: Vec<recent::RecentFile>,
    /// The user's own VST3 and CLAP folders.
    pub plugin_dirs: plugin_scan::PluginDirs,
    /// Window > Plug-in Windows Follow transit (issue #173): editors hide, return and come to the
    /// front with the main window and have no taskbar button. Off: the behavior before #173.
    pub editors_follow: bool,
    /// Plug-in editors to have open, as (track, slot); the instrument is
    /// `transit_mix::INSTRUMENT_SLOT`. `ui.set` opens and closes editors through this list
    /// (the same field `tools/exp-quit.ps1` sets in SoundCraft).
    pub plugin_windows: Vec<(TrackId, usize)>,
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
    /// Height of the mixer pane in points.
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
            audio_devices_section: String::new(),
            io_maps: Vec::new(),
            recent_files: Vec::new(),
            plugin_dirs: plugin_scan::PluginDirs::default(),
            editors_follow: true,
            plugin_windows: Vec::new(),
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
            mixer_height: 420.0,
            cursor: 0,
            cursor_track: None,
            focus_track: None,
            keys: keys::KeysPrefs::default(),
            collapsed: Vec::new(),
            browser: browser::BrowserPrefs::default(),
            appearance: theme::Appearance::System,
            ui_zoom: 1.0,
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
}
