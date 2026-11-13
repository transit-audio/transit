//! transit's studio UI: an arrangement window and a transport over the transit engine.
//!
//! "Cubase bones, Ableton manners" (ADR 0008). The UI draws engine state and acts by executing
//! engine commands; audio goes through [`transit_playback::Player`] and the host loop in
//! [`host`], which is a marked copy of `transit-ui-egui`'s `SoundApp` loop (plan 06, section 3).
//! Nothing here depends on `transit-ui-egui`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod about;
pub mod actions;
pub mod arrange;
mod audio_connection;
pub mod audio_devices;
pub mod audio_devices_io;
pub mod audio_devices_system;
pub mod automation;
mod automation_picker;
pub mod bounce_in_place;
pub mod browser;
pub mod builtin_editor;
pub mod chrome;
pub mod chrome_pick;
pub mod clip_inspector;
pub mod compressor_panel;
pub mod control;
mod dawproject;
pub mod debug_log;
pub mod device_editor;
pub mod device_window;
pub mod devices;
pub mod diagnostics;
pub mod drops;
pub mod edit_tools;
mod editor_group;
mod editors;
mod external_instrument;
mod external_measure;
pub mod fades;
pub mod filter_curve;
pub mod fine;
pub mod finish;
pub mod fonts;
mod frame_logic;
pub mod grid;
pub mod groups;
mod hardware_measure;
pub mod hardware_ui;
pub mod help;
pub mod host;
pub mod host_hang;
pub mod icons;
pub mod import;
mod interface_zoom;
mod io_settings;
pub mod keys;
pub mod load_readout;
mod loopbar;
mod low_latency;
pub mod macro_osc_panel;
pub mod main_monitor;
pub mod menus;
pub mod metronome;
mod middle_c;
pub mod midi_fx_panel;
mod midi_input;
pub mod midi_learn;
pub mod midi_monitor;
mod midi_output;
mod midi_transport;
pub mod mixer;
pub mod modal_voice_panel;
pub mod modulator_panel;
pub mod panel;
pub mod panel_matrix;
pub mod plugin_header;
pub mod plugin_scan;
mod prefs_file;
pub mod presets;
pub mod rack_panel;
pub mod random_sequencer_panel;
pub mod recent;
pub mod render_measurement;
pub mod resonator_panel;
pub mod sampler_panel;
pub mod scope_strip;
pub mod session_folder;
pub mod shortcuts;
pub mod splash;
mod status;
mod takes;
pub mod telemetry;
pub mod tempo_edit;
pub mod texture_panel;
pub mod texture_synth_panel;
pub mod theme;
pub mod time_stretcher_panel;
pub mod title_bar;
pub mod tokens;
pub mod transport;
pub mod tuner_panel;
mod unresponsive;
pub mod update;
pub mod waveform;
pub mod widgets;

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use transit_engine::Engine;
use transit_model::TrackId;
use transit_playback::Player;
use transit_time::{Range, Samples};

pub use control::ControlRequest;

/// The most frames `ui.set imports_pace` may stretch a fake prepare over.
const MAX_IMPORTS_PACE: u32 = 600;

mod ui_state;
pub use ui_state::UiState;

/// Host services (file dialogs) injected by the app; absent in tests.
#[derive(Default)]
pub struct Services {
    /// Pick a file to open for a purpose (`session.open`, `folder`, ...), starting in the given
    /// folder when there is one (#385); `None`: the dialog's own memory.
    pub pick_open: Option<Box<dyn Fn(&str, Option<&std::path::Path>) -> Option<String>>>,
    /// Pick a file to save: the default is a file name, or a full path whose folder the dialog
    /// starts in (#385).
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
    /// Asks a yes/no question (deleting a plug-in preset, issue #68); `None`: such actions are
    /// refused.
    pub confirm: Option<Box<dyn Fn(&str) -> bool>>,
    /// The app's settings folder; user presets live under `presets/` in it. `None`: factory presets only.
    pub config_dir: Option<std::path::PathBuf>,
    /// Replaces the deferred launch open of the audio device (tests); `None`: the real open.
    pub open_audio: Option<Box<dyn Fn(std::sync::Arc<transit_model::Session>, &transit_playback::AudioConfig) -> Player>>,
    /// Starts a new-version check (the app's, behind its `update-check` feature); `None`: no check.
    pub update_check: Option<update::UpdateCheck>,
    /// The check is off because `TRANSIT_NO_UPDATE_CHECK` is set (says why in the Help menu).
    pub update_off_by_env: bool,
    /// The app's logger, for Help > Debug Logging (`help.debug_logging`); `None`: no such control.
    pub log: Option<Box<dyn debug_log::LogControl>>,
}

/// Ballistic meter display state (copied from `transit-ui-egui`, see [`host::feed_meter`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterDisplay {
    pub level: [f32; 2],
    pub hold: [f32; 2],
    /// Seconds since each channel's hold line was last set by a peak.
    pub hold_age: [f32; 2],
    pub clip: bool,
    /// The loudest sample since the last reset (linear), held until the strip's peak readout is
    /// clicked. Fed from the same peaks as `level`.
    pub peak: f32,
    pub gr: f32,
    pub plugin_failures: u32,
    /// The strip's plug-in processing error count last seen (`transit_mix::PluginFailures::errors`).
    pub plugin_errors: u32,
    /// The mixer's channel meter reading per channel (linear): the incoming peaks stepped with
    /// the kit's peak fall-off ballistics (`widgets::level_meter`), fed with `level`.
    pub falloff: [f32; 2],
    /// The clip latch per channel (the engine publishes a peak per channel); `clip` is either.
    pub clip_ch: [bool; 2],
    /// The engine's RMS per channel (linear, over `transit_mix::RMS_WINDOW_MS`), shown as
    /// published: the window is its ballistics. The channel meter's solid layer (#395).
    pub rms: [f32; 2],
}

impl MeterDisplay {
    /// Clears the clip latches (both channels).
    pub fn clear_clip(&mut self) {
        self.clip = false;
        self.clip_ch = [false; 2];
    }
}

/// A modal question.
#[derive(Debug, Clone)]
pub enum Dialog {
    /// Save changes before running `then` (session.new/open, app.quit).
    SaveChanges { then: (String, Value) },
    /// Delete several tracks, or tracks that hold clips (Delete key or the track menu).
    DeleteTracks { tracks: Vec<TrackId>, clips: bool },
    /// File > Bounce: its form, then its progress (slice 6, `finish`).
    Bounce,
    /// File > Export DAWproject: the stem fallback is stated before choosing a location.
    DawprojectExport,
    /// Track > Bounce in Place.
    BounceInPlace,
    /// Track > Show Hidden Tracks.
    HiddenTracks,
    /// Delete a take (#25): the take lane menu's Delete Take asks first.
    DeleteTake { track: TrackId, take: u32 },
}

/// An editor asked for (by `ui.set` or a menu) and not open yet: retried until the plug-in is
/// loaded, or given up at `deadline` (seconds of UI time).
#[derive(Debug, Clone)]
struct EditorWanted {
    key: (TrackId, usize),
    name: String,
    deadline: f64,
    next_try: f64,
    opened: bool,
    requested: bool,
    waiting_logged: bool,
    /// After a reorder: the player binds the instance to its old slot until the new plan is
    /// adopted, so "not open" is not yet "closed by the user" before this time.
    settle_until: f64,
}

pub struct StudioApp {
    pub engine: Engine,
    /// Take stacks per track for the open document's current revision (#25).
    pub(crate) take_cache: takes::TakeCache,
    pub player: Option<Player>,
    player_requests: host::PlayerRequests,
    /// Input capture, opened on first record.
    pub recorder: Option<transit_playback::record::Recorder>,
    /// Synthetic recorder output for tests that must not open an input device.
    #[cfg(test)]
    finished_test_takes: Option<transit_playback::record::Takes>,
    /// Synthetic MIDI capture for tests that must not run the audio thread.
    #[cfg(test)]
    finished_test_midi: Option<transit_playback::MidiTake>,
    record_start: Samples,
    /// Where the pass under way has got to, counting laps of the cycle: the transport position
    /// wraps, the capture's device time does not.
    record_clock: Option<host::RecordClock>,
    /// The loop the pass under way records under (None: straight through), fixed at its start:
    /// the player keeps it until Stop, and its laps and MIDI clips are cut by it (#81).
    pub(crate) take_cycle: Option<Range>,
    /// The MIDI pass under way as drawn: provisional notes, rebuilt each frame (or every other).
    pub(crate) midi_live: Option<host::MidiLive>,
    midi_live_tick: u32,
    /// The MIDI take journal of the pass under way (crash recovery, K11).
    pub(crate) midi_journal: Option<transit_engine::midi_journal::Writer>,
    /// The journal of a finished pass, until the autosave holds its clips (K13).
    pub(crate) placed_journal: Option<host::PlacedJournal>,
    /// Makes the next Stop see an approximate start, standing in for the recorder's latch.
    #[cfg(test)]
    test_approximate_start: bool,
    /// A lap limit for cycle recording smaller than the engine's, so a test reaches it (#522).
    #[cfg(test)]
    test_lap_limit: Option<Samples>,
    /// Punch range to trim the take to (loop recording only in this UI).
    punch: Option<Range>,
    /// Why the audio input did not open or stopped (the open error, or the device trouble), until
    /// the device is reopened or the document replaced. The Audio Devices card and the I/O page say it.
    pub(crate) input_failure: Option<String>,
    /// When to reopen the device after its driver asked for it (ASIO reset), and what to say.
    pub(crate) reopen: transit_playback::reopen::Reopener,
    /// Opens the replacement Player of an automatic reopen in tests (a scripted driver); the app
    /// uses `Player::with_config_and_monitor`.
    #[cfg(test)]
    pub(crate) reopen_with: Option<Box<dyn Fn(std::sync::Arc<transit_model::Session>, &transit_playback::AudioConfig) -> Player>>,
    lost_shown: u64,
    take_plan: Option<transit_engine::io::Recording>,
    pub ui: UiState,
    /// The status line.
    pub status: String,
    /// Ages the status line out and keeps the last message for the diagnostics report (#203).
    pub(crate) status_age: status::StatusAge,
    /// The last plug-in state capture left plug-ins out (issue #56): the notice, for a status
    /// that replaces it in the same action (a bounce starting).
    state_notice: Option<String>,
    pub services: Services,
    pub autosave_dir: Option<std::path::PathBuf>,
    autosaver: Option<transit_engine::autosave::Autosaver>,
    autosaved_rev: u64,
    pub leftovers: Vec<transit_engine::autosave::Leftover>,
    panic_rev: u64,
    last_autosave: Option<std::time::Instant>,
    pub main_meter: MeterDisplay,
    pub meters: HashMap<TrackId, MeterDisplay>,
    meter_snapshot: transit_playback::MeterSnapshot,
    last_meter_poll: f64,
    /// Transport simulation when there is no player (tests, `--no-audio`).
    sim: Option<(Samples, Option<Samples>, Option<Range>)>,
    /// How far the simulated transport has jumped back at its loop wraps since the pass under way
    /// was planned: its position plus this never wraps (the player's equivalent is the output
    /// frame it consumed, `Player::samples_since`).
    pub(crate) sim_laps: Samples,
    /// The same for a record count-in: samples left, then what to play when it ends.
    sim_count_in: Option<(Samples, (Samples, Option<Samples>, Option<Range>))>,
    /// The running count-in's grid, for the transport's beat counter.
    count_in_plan: Option<transit_playback::CountIn>,
    /// The metronome popup is open.
    pub(crate) metro_popup: bool,
    /// The loop the session asked for at the last frame, so a change while playing retargets the running loop.
    loop_seen: Option<Range>,
    record_hint: Option<(f64, String)>,
    midi_transport: midi_transport::State,
    pub quit_requested: bool,
    pub frame_ms: f32,
    last_frame: Option<f64>,
    last_rev: u64,
    low_latency: low_latency::State,
    /// The opened device still needs its saved I/O table; the engine's table revision last passed on.
    io_adopt: bool,
    io_pushed: u64,
    /// The status-bar path warning, with the (document, I/O map) revisions it was computed for.
    io_warning_cache: Option<((u64, u64), Option<String>)>,
    /// A ResetSession boundary awaiting successful admission or device reopen.
    restore_pending: bool,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_shots: Vec<control::PendingShot>,
    pub synthetic: Vec<egui::Event>,
    synthetic_grace: u32,
    /// Modifiers held by the synthetic gesture being played (`ui.drag {alt: true}`).
    synthetic_mods: Option<egui::Modifiers>,
    /// The appearance last given to egui (re-applied when `ui.appearance` changes).
    theme_applied: Option<theme::Appearance>,
    zoom_applied: Option<f32>,
    pub dialog: Option<Dialog>,
    /// The selected tracks were picked by clicking their headers (arrangement header or mixer
    /// strip): only then does Delete take the tracks (Ableton). Any other selection change, such
    /// as a clip click, a lane click or Select All, clears it. View state, not the document.
    pub tracks_selected_by_header: bool,
    editors: Vec<EditorWanted>,
    /// Editor windows following the main window (issue #173).
    editor_group: editor_group::EditorGroup,
    /// Plug-in host threads not responding, as last reported in the status line (ADR 0018).
    host_wedged_shown: usize,
    /// The message for a plug-in host thread stuck in a plug-in, with Save and Give up (#205).
    pub host_hang: host_hang::HostHang,
    /// Slots reading "Not responding", as last reported in the status line (issue #82).
    stuck_shown: usize,
    /// The status line's "Retry all" (glyph and label) as last drawn: shown while more than one
    /// slot reads "Not responding" (issue #82).
    pub retry_all_rect: Option<egui::Rect>,
    /// The hosted editor windows' header strips: preset operations under way (issue #68).
    pub plugin_header: plugin_header::HeaderState,
    /// The arrangement: gesture in progress, scroll, and last frame's layout (for `ui.inspect`).
    pub arrange: arrange::ArrangeState,
    /// What the last Copy took, so a Paste onto the original is refused (UX gap, plan 06).
    pub copied: Vec<(TrackId, Range)>,
    /// The mixer and device chain: gesture in progress, scroll, and last frame's layout.
    pub mixer: mixer::MixerState,
    pub tempo_ui: tempo_edit::TempoUi,
    /// The clip inspector's fade fields: the one being typed in and last frame's drawing (#182).
    pub inspector: clip_inspector::State,
    /// The piano roll (slice 3): open clip, selection, gesture, scroll, last frame's layout.
    pub keys: keys::KeysState,
    /// The browser (slice 4): listings, selection, drag, last frame's layout.
    pub browser: browser::BrowserState,
    /// Audio imports in flight (L30): prepared by the engine's workers, committed in order.
    pub imports: import::ImportQueue,
    /// Help > Keyboard Shortcuts as last drawn (slice 5).
    pub shortcuts_window: menus::ShortcutsLayout,
    /// Help > About transit: open flag and last frame's layout.
    pub about: about::AboutState,
    /// The new-version notice and the check behind it.
    pub update: update::UpdateState,
    /// The overload counters when the transport last started (see [`debug_log::stop_line`]).
    run_counts: Option<debug_log::RunCounts>,
    /// External MIDI output ports connected at the last frame (for the closed line).
    midi_out_connected: Vec<String>,
    /// The launch splash (shown while plug-ins scan or the first device opens).
    pub splash: splash::SplashState,
    /// The output-callback telemetry readout and its window (polled a few times a second).
    pub telemetry: telemetry::TelemetryState,
    /// The Info panel (#178): on or off, and the entry it shows.
    pub help: help::HelpState,
    /// Slice 6: the name being typed, the bounce dialog and job, take lanes, the undo history.
    pub finish: finish::FinishState,
    pub bounce_place: bounce_in_place::State,
    /// Slice 7: the automation lanes' gesture, point selection, write pass and last layout.
    pub auto: automation::AutoState,
    /// Audio › Audio Devices: the window, its draft choices and the device listing.
    pub audio_devices: audio_devices::AudioDevicesState,
    /// Window > MIDI Monitor: the rows and readouts behind the window (#91).
    pub midi_monitor: midi_monitor::State,
    /// MIDI Learn: the mappings in force, the learn menu and markers as drawn (#97).
    pub midi_learn: midi_learn::State,
    /// The External Instrument card's Measure (ADR 0020 decision 3).
    pub external_measure: external_measure::MeasureState,
    /// The Open Recent entries as last drawn (empty while the submenu is closed).
    pub recent_menu: Vec<recent::RecentItemLayout>,
    /// The built-in plug-in editor windows as last drawn.
    pub builtin_editor: builtin_editor::EditorState,
    /// The Copy diagnostics buttons' state (#114).
    pub diag: diagnostics::DiagState,
    /// The control channel's render measurement (#174, the tail measurement).
    pub measure: render_measurement::MeasureState,
}

impl StudioApp {
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        StudioApp {
            engine,
            player,
            player_requests: host::PlayerRequests::default(),
            take_cache: takes::TakeCache::default(),
            recorder: None,
            #[cfg(test)]
            finished_test_takes: None,
            #[cfg(test)]
            finished_test_midi: None,
            record_start: 0,
            record_clock: None,
            take_cycle: None,
            midi_live: None,
            midi_live_tick: 0,
            midi_journal: None,
            placed_journal: None,
            #[cfg(test)]
            test_approximate_start: false,
            #[cfg(test)]
            test_lap_limit: None,
            punch: None,
            input_failure: None,
            reopen: transit_playback::reopen::Reopener::default(),
            #[cfg(test)]
            reopen_with: None,
            lost_shown: 0,
            take_plan: None,
            ui: UiState::default(),
            status: String::new(),
            // The crate's tests hold status messages: their harnesses frame through wall-clock
            // waits, so aging there would be host-speed dependent (#203, E49).
            status_age: if cfg!(test) { status::StatusAge::held() } else { status::StatusAge::default() },
            state_notice: None,
            services,
            diag: Default::default(),
            measure: Default::default(),
            autosave_dir: None,
            autosaver: None,
            autosaved_rev: 0,
            leftovers: Vec::new(),
            panic_rev: 0,
            last_autosave: None,
            main_meter: MeterDisplay::default(),
            meters: HashMap::new(),
            meter_snapshot: transit_playback::MeterSnapshot::default(),
            last_meter_poll: f64::NEG_INFINITY,
            sim: None,
            sim_laps: 0,
            sim_count_in: None,
            count_in_plan: None,
            metro_popup: false,
            loop_seen: None,
            record_hint: None,
            midi_transport: midi_transport::State::default(),
            quit_requested: false,
            frame_ms: 0.0,
            last_frame: None,
            last_rev: 0,
            low_latency: Default::default(),
            io_adopt: true,
            io_pushed: 0,
            io_warning_cache: None,
            restore_pending: false,
            control_rx: None,
            pending_shots: Vec::new(),
            synthetic: Vec::new(),
            synthetic_grace: 0,
            synthetic_mods: None,
            theme_applied: None,
            zoom_applied: None,
            dialog: None,
            tracks_selected_by_header: false,
            editors: Vec::new(),
            editor_group: editor_group::EditorGroup::default(),
            host_wedged_shown: 0,
            host_hang: host_hang::HostHang::default(),
            stuck_shown: 0,
            retry_all_rect: None,
            plugin_header: plugin_header::HeaderState::default(),
            arrange: arrange::ArrangeState::default(),
            copied: Vec::new(),
            mixer: mixer::MixerState::default(),
            tempo_ui: tempo_edit::TempoUi::default(),
            inspector: clip_inspector::State::default(),
            keys: keys::KeysState::default(),
            browser: browser::BrowserState::default(),
            imports: import::ImportQueue::default(),
            shortcuts_window: menus::ShortcutsLayout::default(),
            about: about::AboutState::default(),
            update: update::UpdateState::default(),
            run_counts: None,
            midi_out_connected: Vec::new(),
            splash: splash::SplashState::default(),
            telemetry: telemetry::TelemetryState::default(),
            help: help::HelpState::default(),
            finish: finish::FinishState::default(),
            bounce_place: bounce_in_place::State::default(),
            auto: automation::AutoState::default(),
            audio_devices: audio_devices::AudioDevicesState::default(),
            midi_monitor: midi_monitor::State::default(),
            midi_learn: midi_learn::State::default(),
            external_measure: external_measure::MeasureState::default(),
            recent_menu: Vec::new(),
            builtin_editor: builtin_editor::EditorState::default(),
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// The edit cursor: where Play starts and Paste lands. Distinct from the selection
    /// (`edit.selection`); view state in [`UiState`], never in the document.
    pub fn cursor(&self) -> Samples {
        self.ui.cursor.max(0)
    }

    /// The track the cursor was last set on (paste target), if it still exists.
    pub fn cursor_track(&self) -> Option<TrackId> {
        let id = self.ui.cursor_track?;
        self.engine.session().track(id).map(|t| t.id)
    }

    /// Set the cursor (and its track). View state only: the document is untouched.
    pub fn set_cursor(&mut self, at: Samples, track: Option<TrackId>) {
        self.ui.cursor = at.max(0);
        if let Some(t) = track {
            self.ui.cursor_track = Some(t);
        }
        if !self.engine.transport.playing {
            self.engine.transport.position = self.ui.cursor;
        }
    }

    /// The device card's editor button: closes the editor when it is open (or wanted), else asks for it.
    pub fn toggle_editor(&mut self, key: (TrackId, usize)) {
        if self.ui.plugin_windows.contains(&key) {
            self.close_editor(key);
        } else {
            self.want_editor(key);
        }
    }

    /// Close a plug-in editor window (or stop waiting for it to open).
    pub fn close_editor(&mut self, key: (TrackId, usize)) {
        if self.builtin_at(key).is_some() {
            // A built-in's editor is a window of ours: nothing in the player to close.
            self.ui.plugin_windows.retain(|k| *k != key);
            return;
        }
        if let Some(p) = &self.player
            && self.player_requests.result(&mut self.status, p.close_editor(key.0, key.1)).is_none()
        {
            return;
        }
        self.editors.retain(|e| e.key != key);
        self.ui.plugin_windows.retain(|k| *k != key);
    }

    /// Puts what the plug-in host thread reports (ADR 0018) in the status line: an editor that
    /// failed after its open was accepted, and a plug-in that stopped responding.
    fn plugin_host_status(&mut self) {
        self.note_plugin_timings();
        let Some(p) = &self.player else { return };
        for (track, slot, e) in p.take_editor_errors() {
            self.status = format!("Plug-in editor: {e}");
            self.editors.retain(|w| w.key != (track, slot));
            self.ui.plugin_windows.retain(|k| *k != (track, slot));
        }
        let wedged = p.plugin_host_wedged();
        let stuck = p.stuck_slots();
        if stuck.len() > self.stuck_shown {
            // Issue #82: name the plug-in and the way back.
            self.status = match stuck.as_slice() {
                [(_, slot, id)] => format!(
                    "{} is not responding (it has not finished loading); Retry on its slot loads it again",
                    self.plugin_name(id, *slot == transit_mix::INSTRUMENT_SLOT)
                ),
                many => format!("{} plug-ins are not responding (they have not finished loading); Retry all loads them again", many.len()),
            };
        } else if wedged > self.host_wedged_shown && stuck.is_empty() {
            self.status = "A plug-in is not responding; the live state of the plug-ins on its thread is not saved until it responds again".into();
        } else if wedged == 0 && self.host_wedged_shown > 0 {
            self.status = "The plug-in that was not responding has returned; its state is saved again".into();
        }
        self.host_wedged_shown = wedged;
        self.stuck_shown = stuck.len();
    }

    /// Makes a "Not responding" slot's plug-in again (issue #82): `plugin.retry`, as the slot's
    /// Retry button and context menu run it.
    pub fn retry_plugin(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        let slot = if slot == transit_mix::INSTRUMENT_SLOT { json!("instrument") } else { json!(slot) };
        self.run("plugin.retry", json!({"track": t.0, "slot": slot}))
    }

    /// `plugin.retry` and `plugin.retry_all`: checked by the engine (the slot exists; not a
    /// document change), then the player makes the plug-ins again.
    fn retry_plugins(&mut self, id: &str, params: &Value) -> Result<Value, String> {
        let checked = self.execute_ui(id, params).map_err(|e| {
            self.status = e.to_string();
            e.to_string()
        })?;
        let Some(p) = &self.player else {
            self.status = "Retry: no audio engine is running".into();
            return Err(self.status.clone());
        };
        if id == "plugin.retry_all" {
            let n = p.retry_all_plugins();
            self.status = match n {
                0 => "No plug-in is waiting for a retry".into(),
                1 => "Retrying 1 plug-in".into(),
                n => format!("Retrying {n} plug-ins"),
            };
            return Ok(json!({"retried": n}));
        }
        let track = TrackId(checked["track"].as_u64().unwrap_or_default());
        let slot = checked["slot"].as_u64().and_then(|s| usize::try_from(s).ok()).unwrap_or(transit_mix::INSTRUMENT_SLOT);
        let plugin = checked["plugin"].as_str().unwrap_or_default().to_owned();
        let name = self.plugin_name(&plugin, slot == transit_mix::INSTRUMENT_SLOT);
        if !self.player.as_ref().is_some_and(|p| p.retry_plugin(track, slot)) {
            self.status = format!("{name} is not waiting for a retry (only a slot that reads Not responding is)");
            return Err(self.status.clone());
        }
        self.status = format!("Retrying {name}");
        Ok(json!({"retried": 1}))
    }

    pub fn is_playing(&self) -> bool {
        self.player.as_ref().map_or(self.engine.transport.playing, Player::is_playing)
    }

    /// Current transport position: the playhead while playing, else the cursor.
    pub fn position(&self) -> Samples {
        if let Some(p) = &self.player
            && p.is_playing()
        {
            return p.position();
        }
        if self.engine.transport.playing { self.engine.transport.position } else { self.cursor() }
    }

    /// Why the last Record press did nothing, for a few seconds and while it still holds.
    pub fn record_hint(&self) -> Option<&str> {
        let (at, why) = self.record_hint.as_ref()?;
        let fresh = self.last_frame.unwrap_or_default() - at < 4.0;
        (fresh && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm)).then_some(why)
    }

    /// UI state as JSON (control channel `ui.inspect`).
    pub fn inspect(&self, ctx: &egui::Context) -> Value {
        let size = ctx.content_rect().size();
        let open = editor_group::open(self);
        let mut v = json!({
            "ui": self.ui,
            "window_size": [size.x, size.y],
            "playing": self.is_playing(),
            "recording": self.engine.transport.recording,
            "input_open": self.recorder.is_some(),
            "position": self.position(),
            "cursor": self.cursor(),
            "cursor_track": self.cursor_track(),
            "selected_clips": self.engine.session().edit.selected_clips,
            "dialog": self.dialog.as_ref().map(|d| match d {
                Dialog::SaveChanges { .. } => "save_changes",
                Dialog::DeleteTracks { .. } => "delete_tracks",
                Dialog::Bounce => "bounce",
                Dialog::DawprojectExport => "dawproject_export",
                Dialog::BounceInPlace => "bounce_in_place",
                Dialog::HiddenTracks => "hidden_tracks",
                Dialog::DeleteTake { .. } => "delete_take",
            }),
            "frame_ms": self.frame_ms,
            "audio_device": self.player.as_ref().map(|p| p.device_name.clone()),
            "editors_open": open,
            "main_meter": self.main_meter.level,
            "status": self.status,
            "update_notice": self.update.notice,
            "update_checking": self.update.checking(),
            "layout": self.arrange.layout,
            "focus_track": self.focus_track(),
            "mixer": self.mixer.layout,
            "devices": self.mixer.devices,
            "hardware_picker": self.mixer.hw_picker_layout,
            "track_menu": self.arrange.menu,
            "bar_menu": self.arrange.bar_menu,
            "menu_state": {"filter_rect": self.mixer.filter_rect},
            "readout_edit": self.mixer.edit.as_ref().map(|e| json!({
                "control": format!("{:?}", e.target),
                "text": e.text,
                "rect": self.mixer.edit_rect,
            })),
            "wheel_merge": self.mixer.wheel.is_some(),
            "plugin_scan": plugin_scan::inspect(),
            "keys": self.keys.layout,
            "browser": self.browser.layout,
            "imports": self.imports.layout,
            "shortcuts": self.shortcuts_window,
            "about": self.about,
            "splash": self.splash,
            "telemetry": self.telemetry.layout,
            "finish": self.finish.inspect(),
            "automation": self.auto.layout,
            "audio_devices": self.audio_devices.layout,
            "midi_monitor": midi_monitor::inspect(self),
            "recent_menu": self.recent_menu,
            "theme": if theme::current() == egui::Theme::Light { "light" } else { "dark" },
        });
        v["bounce_in_place"] = json!(self.bounce_place.layout);
        // Added apart from the literal: it is at the macro's recursion limit.
        v["midi_learn"] = midi_learn::inspect(self);
        v["help"] = json!(self.help.layout);
        v["editor_windows"] = editor_group::inspect(self);
        v["recording_pass"] = self.recording_pass_inspect();
        v["monitor"] = self.monitor_state();
        v["middle_c"] = json!(self.ui.middle_c);
        v
    }
}

#[cfg(test)]
mod audio_devices_tests;
#[cfg(test)]
mod automation_picker_tests;
#[cfg(test)]
mod automation_tests;
#[cfg(test)]
mod browser_tests;
#[cfg(test)]
mod builtin_editor_tests;
#[cfg(test)]
mod chrome_pick_tests;
#[cfg(test)]
mod chrome_tests;
#[cfg(test)]
mod context_menu_tests;
#[cfg(test)]
mod dialog_chrome_tests;
#[cfg(test)]
mod external_measure_tests;
#[cfg(test)]
mod groups_tests;
#[cfg(test)]
mod help_tests;
#[cfg(test)]
mod inactive_hatch_tests;
#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod loop_hue_tests;
#[cfg(test)]
mod main_monitor_tests;
#[cfg(test)]
mod menu_gutter_tests;
#[cfg(test)]
mod menu_padding_tests;
#[cfg(test)]
mod middle_c_display_tests;
#[cfg(test)]
mod middle_c_param_tests;
#[cfg(test)]
mod middle_c_tests;
#[cfg(test)]
mod midi_fx_panel_tests;
#[cfg(test)]
mod midi_learn_hosted_tests;
#[cfg(test)]
mod midi_learn_live_tests;
#[cfg(test)]
mod midi_learn_plugin_tests;
#[cfg(test)]
mod midi_learn_tests;
#[cfg(test)]
mod midi_mappings_window_tests;
#[cfg(test)]
mod midi_output_header_tests;
#[cfg(test)]
mod midi_pickup_tests;
#[cfg(test)]
mod midi_record_tests;
#[cfg(test)]
mod modal_voice_panel_tests;
#[cfg(test)]
mod modulator_panel_tests;
#[cfg(test)]
mod param_capture_tests;
#[cfg(test)]
mod rack_panel_tests;
#[cfg(test)]
mod recent_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod resonator_panel_tests;
#[cfg(test)]
mod session_folder_tests;
#[cfg(test)]
mod slice5_tests;
#[cfg(test)]
mod slice6_tests;
#[cfg(test)]
mod texture_panel_tests;
#[cfg(test)]
mod texture_synth_panel_tests;
#[cfg(test)]
mod time_stretcher_panel_tests;
#[cfg(test)]
mod tokens_tests;
/// Services what hosted plug-ins asked of the host (audit F08, ADR 0012 G7): CLAP restarts,
/// callbacks and rescans, VST3 `restartComponent`. Main thread, every frame, also with no player
/// and no editor open (the instances were created on this thread). Idempotent: the playback
/// runtime may call the same functions. True while a request is still in progress, so the frame
/// loop comes back soon instead of after its idle second.
fn service_plugin_requests() -> bool {
    let clap = transit_clap_host::service_requests();
    let vst3 = transit_vst3_host::service_requests();
    clap.pending + vst3.pending > 0
}

#[cfg(test)]
mod arrange_copy_tests;
#[cfg(test)]
mod arrange_edit_tests;
#[cfg(test)]
mod arrange_ghost_tests;
#[cfg(test)]
mod arrange_loop_tests;
#[cfg(test)]
mod arrange_marquee_tests;
#[cfg(test)]
mod devices_chain_tests;
#[cfg(test)]
mod devices_sidechain_tests;
#[cfg(test)]
mod edit_tools_tests;
#[cfg(test)]
mod editor_diagnostics_tests;
#[cfg(test)]
mod emergency_tests;
#[cfg(test)]
mod fades_gesture_tests;
#[cfg(test)]
mod host_hang_tests;
#[cfg(test)]
mod loopbar_gesture_tests;
#[cfg(test)]
mod mixer_controls_tests;
#[cfg(test)]
mod mixer_hscroll_tests;
#[cfg(test)]
mod mixer_layout_tests;
#[cfg(test)]
mod mixer_meter_tests;
#[cfg(test)]
mod mixer_overflow_tests;
#[cfg(test)]
mod mixer_routing_tests;
#[cfg(test)]
mod mixer_strip_tests;
#[cfg(test)]
mod plugin_header_tests;
#[cfg(test)]
mod plugin_host_tests;
#[cfg(test)]
mod plugin_requests_tests;
#[cfg(test)]
mod plugin_retry_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod track_actions_tests;
#[cfg(test)]
mod unresponsive_tests;

#[cfg(test)]
mod tempo_tests;
#[cfg(test)]
mod transport_tests;
#[cfg(test)]
mod width_history_tests;

#[cfg(test)]
mod about_splash_tests;

#[cfg(test)]
mod window_title_tests;

#[cfg(test)]
mod bounce_in_place_tests;
