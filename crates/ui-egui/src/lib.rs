//! SoundCraft's egui user interface.
//!
//! The UI is a thin shell over [`transit_engine::Engine`]: it draws engine state and acts by
//! executing commands. It can be replaced without touching anything below it.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod automation;
mod autosave;
pub mod bounce;
pub mod control;
pub mod credits;
pub mod dialogs;
pub mod docking;
pub mod edit_window;
pub mod extra_windows;
pub mod fonts;
pub mod icons;
pub mod menus;
pub mod midi_editor;
mod midi_input;
pub mod midi_keyboard;
pub mod mix_window;
pub mod ops_windows;
pub mod palette;
pub mod panels;
pub mod plugin_folders;
mod recent;
mod recording;
pub mod recovery;
pub mod sampler_window;
pub mod score_editor;
pub mod shortcuts;
pub mod spot;
pub mod theme;
pub mod toolbar;
mod transport;
pub mod video_track;
pub mod video_window;
pub mod waveform;
pub mod widgets;
pub mod windows;

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use transit_engine::{Engine, TransportRequest};
use transit_model::TrackId;
use transit_playback::record::Trouble;
use transit_playback::{OutputTrouble, Player};
use transit_time::{Range, Samples};

pub use control::ControlRequest;

/// Which main window is in front (Window › Mix / Edit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MainWindow {
    Edit,
    Mix,
}

/// UI state (serde, so agents can read and set it).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiState {
    /// File › Open Recent, newest first (see `recent`).
    pub recent_sessions: Vec<String>,
    pub docking: docking::Workspace,
    #[serde(default = "docking::mix_default")]
    pub mix_docking: docking::Workspace,
    pub window: MainWindow,
    pub show_tracks_list: bool,
    pub show_clip_list: bool,
    pub show_transport: bool,
    pub show_memory_locations: bool,
    pub show_big_counter: bool,
    pub show_undo_history: bool,
    pub show_session_info: bool,
    pub show_audio_health: bool,
    pub show_about: bool,
    pub show_midi_editor: bool,
    pub show_universe: bool,
    pub narrow_mix: bool,
    /// Mix window sections: inserts_ae, inserts_fj, sends_ae, sends_fj, io, comments, eq_curve, color.
    pub mix_views: Vec<String>,
    /// Edit window track-header columns: io, inserts_ae, sends_ae, comments, color.
    pub edit_views: Vec<String>,
    /// How strongly Edit window tracks take on their colour, 0..1 (0: only the colour strip).
    pub track_tint: f32,
    /// Open plugin windows (track, slot).
    pub plugin_windows: Vec<(TrackId, usize)>,
    /// The send in the Send window (track, slot).
    #[serde(skip)]
    pub send_window: Option<(TrackId, usize)>,
    pub audiosuite: Option<String>,
    pub status: String,
    pub show_automation: bool,
    pub show_color_palette: bool,
    pub show_disk_usage: bool,
    pub show_system_usage: bool,
    pub show_task_manager: bool,
    pub show_metadata: bool,
    pub show_event_list: bool,
    pub show_midi_keyboard: bool,
    /// Window › MIDI Keyboard's octave, velocity, transpose and mode.
    pub midi_keyboard: midi_keyboard::KeyboardPrefs,
    pub show_workspace: bool,
    pub show_configurations: bool,
    pub show_playback_engine: bool,
    pub show_plugin_folders: bool,
    /// The user's own VST3 and CLAP folders (Setup › Plug-in Folders), kept with the preferences.
    pub plugin_dirs: plugin_folders::PluginDirs,
    /// The audio device choice (Setup › Playback Engine), kept with the preferences: per machine.
    pub audio: transit_playback::AudioConfig,
    /// The Mix window lives in its own native window (Window › Mix in Separate Window); the main
    /// window then always shows Edit.
    pub mix_separate: bool,
    pub show_io_setup: bool,
    pub show_shortcuts: bool,
    pub show_clip_effects: bool,
    pub show_beat_detective: bool,
    pub show_tempo_ops: bool,
    pub show_time_ops: bool,
    pub show_midi_ops: bool,
    pub show_rtp: bool,
    pub show_score: bool,
    pub show_search: bool,
    /// Window › Video, Window › Video Universe, timecode burn-in in the Video window.
    pub show_video: bool,
    pub show_video_universe: bool,
    pub video_burn_in: bool,
    pub workspace_dir: String,
    pub configurations: Vec<(String, Value)>,
    /// Appearance (`ui.theme`): dark by default, light or follow the system on request.
    pub theme: theme::ThemeMode,
    /// The main window's size (logical points) and maximized state, and the interface scale, as
    /// the desktop app last saw them; it restores them at launch.
    pub window_size: [f32; 2],
    pub window_maximized: bool,
    pub ui_scale: f32,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            recent_sessions: Vec::new(),
            docking: docking::Workspace::default(),
            mix_docking: docking::mix_default(),
            window: MainWindow::Edit,
            show_tracks_list: true,
            show_clip_list: true,
            show_transport: false,
            show_memory_locations: false,
            show_big_counter: false,
            show_undo_history: false,
            show_session_info: false,
            show_audio_health: false,
            show_about: false,
            show_midi_editor: false,
            show_universe: false,
            narrow_mix: false,
            mix_views: vec!["inserts_ae".into(), "sends_ae".into(), "io".into(), "color".into()],
            edit_views: vec!["color".into()],
            track_tint: 0.5,
            plugin_windows: Vec::new(),
            send_window: None,
            audiosuite: None,
            status: String::new(),
            show_automation: false,
            show_color_palette: false,
            show_disk_usage: false,
            show_system_usage: false,
            show_task_manager: false,
            show_metadata: false,
            show_event_list: false,
            show_midi_keyboard: false,
            midi_keyboard: midi_keyboard::KeyboardPrefs::default(),
            show_workspace: false,
            show_configurations: false,
            show_playback_engine: false,
            show_plugin_folders: false,
            plugin_dirs: plugin_folders::PluginDirs::default(),
            audio: transit_playback::AudioConfig::default(),
            mix_separate: false,
            show_io_setup: false,
            show_shortcuts: false,
            show_clip_effects: false,
            show_beat_detective: false,
            show_tempo_ops: false,
            show_time_ops: false,
            show_midi_ops: false,
            show_rtp: false,
            show_score: false,
            show_search: false,
            show_video: false,
            show_video_universe: false,
            video_burn_in: false,
            workspace_dir: String::new(),
            configurations: Vec::new(),
            theme: theme::ThemeMode::Dark,
            window_size: [1600.0, 1000.0],
            window_maximized: false,
            ui_scale: 1.0,
        }
    }
}

/// Host services (file dialogs) injected by the app; absent in tests and on the web.
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn(&str, &[&str]) -> Option<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str, &str) -> Option<String>>>,
    pub pick_folder: Option<Box<dyn Fn(&str) -> Option<String>>>,
}

/// File-system-safe folder name for a plugin id (`clap:com.x.y` → `clap_com.x.y`).
pub fn preset_folder_name(plugin: &str) -> String {
    plugin.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// On the first frame, maximizes a window too big for the screen it opened on (sized on a larger
/// monitor) instead of letting it run off the edge; its saved size is kept for that monitor.
pub fn fit_window_to_screen(ctx: &egui::Context) {
    let (inner, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
    if ctx.cumulative_pass_nr() == 0 && inner.zip(monitor).is_some_and(|(r, m)| (r.size() - m).max_elem() > 0.0) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// Drag-and-drop payload: an audio file (source id) dragged from the Clip List.
#[derive(Debug, Clone, Copy)]
pub struct DragSource(pub u64);

/// Ballistic meter display state per strip.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterDisplay {
    pub level: [f32; 2],
    pub hold: [f32; 2],
    pub hold_age: f32,
    pub clip: bool,
    pub gr: f32,
    /// [`transit_mix::PluginFailures::count`] as last seen.
    pub plugin_failures: u32,
}

/// An in-progress mouse gesture in the Edit window.
#[derive(Debug, Clone)]
pub enum Gesture {
    Select { track: TrackId, anchor: Samples, tracks: Vec<TrackId> },
    MoveClips { clips: Vec<transit_model::ClipId>, grab_at: Samples, delta: Samples, from_track: TrackId, to_track: TrackId },
    TrimStart { clip: transit_model::ClipId, track: TrackId, to: Samples },
    TrimEnd { clip: transit_model::ClipId, track: TrackId, to: Samples },
    Fade { clip: transit_model::ClipId, track: TrackId, fade_in: bool, to: Samples },
    FadeCurve { clip: transit_model::ClipId, fade_in: bool, curvature: f32, anchor: (f32, f32), height: f32 },
    Scrub { last: Samples },
    Fader { track: TrackId, start_db: f32 },
    ClipGain { clip: transit_model::ClipId, track: TrackId, pos: f32, anchor: (f32, f32), fine: bool, fader_bottom: egui::Pos2 },
    Pencil { track: TrackId, points: Vec<(Samples, f32)> },
    MidiClip { track: TrackId, from: Samples, to: Samples },
    ZoomBox { start: f32 },
}

pub struct SoundApp {
    pub engine: Engine,
    /// The session path as of the last frame, and whether the recent list changed since the host
    /// last saved it (see `recent`).
    recent_seen: Option<String>,
    recent_changed: bool,
    /// When the recent sessions were last checked for existence, which list, and the result.
    recent_checked: Option<(f64, Vec<String>, Vec<bool>)>,
    pub player: Option<Player>,
    player_requests: PlayerRequests,
    /// Input capture, opened on first record.
    pub recorder: Option<transit_playback::record::Recorder>,
    record_start: Samples,
    /// Punch range to trim the take to (pre/post-roll recordings).
    punch: Option<Range>,
    recorder_failed: bool,
    /// Input this take lost to a slow disk, as last reported (ms).
    lost_shown: u64,
    /// Where the takes being recorded go, autosaved with the session so a crash can't lose them.
    take_plan: Option<transit_engine::io::Recording>,
    pub ui: UiState,
    pub audio_health_report: Option<Result<Value, String>>,
    pub services: Services,
    /// Set only by a desktop host after installing an OS menu bar.
    pub native_menu_bar: bool,
    /// Folder for crash-recovery autosaves (native apps set it).
    pub autosave_dir: Option<std::path::PathBuf>,
    /// Writes the open session's autosaves in the background, once there is something to write.
    autosaver: Option<transit_engine::autosave::Autosaver>,
    /// The document revision last handed to the autosaver (0 once that autosave is stale).
    autosaved_rev: u64,
    /// Unsaved work that earlier runs left in the autosave folder, offered for recovery.
    pub leftovers: Vec<transit_engine::autosave::Leftover>,
    /// The document revision last autosaved after a caught panic.
    panic_rev: u64,
    /// Folder for user plugin presets (native apps set it).
    pub preset_dir: Option<std::path::PathBuf>,
    last_autosave: Option<std::time::Instant>,
    pub dialogs: dialogs::Dialogs,
    pub meters: HashMap<TrackId, MeterDisplay>,
    pub main_meter: MeterDisplay,
    /// Reused multichannel snapshot; populated at an adaptive rate by `logic`.
    meter_snapshot: transit_playback::MeterSnapshot,
    pub gesture: Option<Gesture>,
    pub edit_layout: edit_window::EditLayout,
    pub midi: midi_editor::MidiEditorState,
    pub keyboard: midi_keyboard::KeyboardState,
    pub ops: ops_windows::OpsState,
    pub extra: extra_windows::ExtraState,
    pub palette: palette::PaletteState,
    /// Bring the separate Mix window to the front on the next frame.
    pub(crate) focus_mix: bool,
    pub synthetic: Vec<egui::Event>,
    /// Where a dropped audio file loads into a track's sampler (last frame's rects).
    pub sample_drop_targets: Vec<(egui::Rect, TrackId)>,
    sample_reads: Vec<sampler_window::SampleRead>,
    synthetic_grace: u32,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_shots: Vec<control::PendingShot>,
    last_rev: u64,
    /// A ResetSession boundary awaiting successful admission or device reopen.
    restore_pending: bool,
    fonts_ready: bool,
    fonts_installed: bool,
    /// The palette last applied to the context (light?), so visuals are set only on a change.
    theme_applied: Option<bool>,
    /// Last panel host context, used to cancel renderer-owned gestures on explicit replacement.
    pub(crate) docking_context: Option<egui::Context>,
    /// Transport simulation when there is no player (tests, offscreen renders).
    sim: Option<(Samples, Option<Samples>, Option<Range>)>,
    /// When Record last found nothing to record on, and why: the Record buttons show it for a moment.
    record_hint: Option<(f64, &'static str)>,
    pub quit_requested: bool,
    /// Reset floating-window positions next frame (Window › Arrange).
    pub arrange_request: bool,
    /// Tracks whose automation was touched during this pass (Touch/Latch writing).
    touched: std::collections::HashSet<TrackId>,
    /// The pass's last point; `i64::MIN` after a stop or a jump, so the next point erases nothing it skipped.
    last_write_at: Samples,
    pub frame_ms: f32,
    last_frame: Option<f64>,
    last_meter_poll: f64,
    /// Movies for the Video window and Video track (decoded in the background).
    pub video: video_track::VideoPool,
    /// The Bounce Mix in progress, rendered a slice per frame.
    pub bounce: Option<transit_engine::io::Bounce>,
    /// Test-only: owns the process-wide palette lock while this app exists, so frames painted
    /// and colour assertions in one test can never straddle another test's palette change.
    #[cfg(test)]
    _palette_lock: theme::TestPaletteGuard,
}

impl SoundApp {
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        SoundApp {
            recent_seen: engine.path.clone(),
            recent_changed: false,
            recent_checked: None,
            engine,
            player,
            player_requests: PlayerRequests::default(),
            recorder: None,
            record_start: 0,
            punch: None,
            recorder_failed: false,
            lost_shown: 0,
            take_plan: None,
            ui: UiState::default(),
            audio_health_report: None,
            services,
            native_menu_bar: false,
            autosave_dir: None,
            autosaver: None,
            autosaved_rev: 0,
            leftovers: Vec::new(),
            panic_rev: 0,
            preset_dir: None,
            last_autosave: None,
            dialogs: dialogs::Dialogs::default(),
            meters: HashMap::new(),
            main_meter: MeterDisplay::default(),
            meter_snapshot: transit_playback::MeterSnapshot::default(),
            gesture: None,
            edit_layout: edit_window::EditLayout::default(),
            midi: midi_editor::MidiEditorState::default(),
            keyboard: midi_keyboard::KeyboardState::default(),
            ops: ops_windows::OpsState::default(),
            extra: extra_windows::ExtraState::default(),
            palette: palette::PaletteState::default(),
            focus_mix: false,
            synthetic: Vec::new(),
            sample_drop_targets: Vec::new(),
            sample_reads: Vec::new(),
            synthetic_grace: 0,
            control_rx: None,
            pending_shots: Vec::new(),
            last_rev: 0,
            restore_pending: false,
            fonts_ready: false,
            fonts_installed: false,
            theme_applied: None,
            docking_context: None,
            sim: None,
            record_hint: None,
            quit_requested: false,
            arrange_request: false,
            touched: std::collections::HashSet::new(),
            last_write_at: i64::MIN,
            frame_ms: 0.0,
            last_frame: None,
            last_meter_poll: f64::NEG_INFINITY,
            video: video_track::VideoPool::default(),
            bounce: None,
            #[cfg(test)]
            _palette_lock: theme::lock_test_palette(),
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run a command; errors go to the status line. Returns the result.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if matches!(id, "session.new" | "session.open" | "session.close" | "session.revert") && bounce::refuses_session_switch(self) {
            return Err(self.ui.status.clone());
        }
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        if id.starts_with("session.save") || id.starts_with("file.bounce") || matches!(id, "track.commit" | "track.bounce") {
            // Third-party plugins keep state the session can't see: store it first.
            self.capture_plugin_states()?;
        }
        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert");
        if replacing && self.recording_active() {
            self.stop_play();
            self.close_autosave();
        }
        match self.engine.execute(id, &params) {
            Ok(v) => {
                if replacing || (id.starts_with("session.save") && !self.engine.is_dirty()) {
                    self.release_autosave(false);
                }
                if id == "app.quit" {
                    self.quit_requested = true;
                }
                if id == "options.scrolling" {
                    // Re-selecting a scrolling mode resumes the playhead follow.
                    self.edit_layout.follow_hold = false;
                }
                if id == "track.clear_clip_indicators" {
                    self.meters.values_mut().chain([&mut self.main_meter]).for_each(|m| m.clip = false);
                }
                Ok(v)
            }
            Err(e) => {
                self.ui.status = e.to_string();
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// Synchronize the immutable snapshot, including view-only COW changes.
    /// Revision alone cannot establish the lineage required by state capture.
    pub(crate) fn sync_player_session(&mut self) -> bool {
        let Some(p) = &self.player else {
            self.restore_pending = false;
            self.last_rev = self.engine.revision;
            return true;
        };
        // ResetSession must first stop/close old-document transport and input.
        if self.engine.transport_requests.contains(&TransportRequest::ResetSession) {
            return false;
        }
        let session = self.engine.session_arc();
        if !self.restore_pending && self.last_rev == self.engine.revision && p.has_requested_session(&session) {
            return true;
        }
        let result = if self.restore_pending { p.replace_session(session) } else { p.update_session(session) };
        if matches!(result, Err(transit_playback::PlayerError::NeedsReopen { .. })) {
            if let Err(error) = self.reconnect_audio_using(false, Player::with_config) {
                self.player_requests.fail(&mut self.ui.status, error);
                return false;
            }
            if self.player.as_ref().is_some_and(|p| p.has_requested_session(&self.engine.session_arc())) {
                self.restore_pending = false;
                self.last_rev = self.engine.revision;
                return true;
            }
            return false;
        }
        if self.player_requests.result(&mut self.ui.status, result).is_none() {
            return false;
        }
        self.restore_pending = false;
        self.last_rev = self.engine.revision;
        true
    }

    /// Reads every live third-party plugin's state and stores it on its insert
    /// (`mix.insert_state`), so saves and bounces include it.
    pub fn capture_plugin_states(&mut self) -> Result<(), String> {
        // Saving must not supersede the prerequisite of an admitted Play/Locate.
        let action_pending = self.player_requests.play.is_some() || self.player_requests.locate.is_some() || self.player_requests.recording.is_some();
        let changed = self.player.as_ref().is_some_and(|p| !p.has_requested_session(&self.engine.session_arc()));
        if (action_pending && changed) || !self.sync_player_session() {
            return Err("The audio session is still updating; try again after it applies".into());
        }
        let Some(p) = &self.player else { return Ok(()) };
        let states = p.capture_states(&self.engine.session_arc());
        self.store_plugin_states(states)
    }

    fn preserve_audio_state(&mut self) -> Result<bool, String> {
        let Some(player) = &self.player else { return Ok(true) };
        let states = player.capture_states_for_reopen(&self.engine.session_arc());
        let states = self.player_requests.result(&mut self.ui.status, states).ok_or_else(|| self.ui.status.clone())?;
        let Some(states) = states else { return Ok(false) };
        self.store_plugin_states(Ok(states))?;
        Ok(true)
    }

    fn store_plugin_states(&mut self, states: Result<transit_playback::PluginStates, transit_playback::PlayerError>) -> Result<(), String> {
        let states = self.player_requests.result(&mut self.ui.status, states).ok_or_else(|| self.ui.status.clone())?;
        for (t, slot, data) in states {
            let slot = if slot == transit_mix::INSTRUMENT_SLOT { json!("instrument") } else { json!(slot) };
            let state = transit_model::b64::encode(&data);
            if let Err(e) = self.engine.execute("mix.insert_state", &json!({"track": t.0, "slot": slot, "state": state})) {
                self.ui.status = format!("Cannot store plugin state: {e}");
                return Err(self.ui.status.clone());
            }
        }
        Ok(())
    }

    /// Parameter edits made in plugin editors go into the session (one undo step per gesture).
    fn apply_editor_edits(&mut self) {
        let discard = self.restore_pending || self.engine.transport_requests.contains(&TransportRequest::ResetSession);
        let Some(p) = &self.player else { return };
        let Some(edits) = self.player_requests.result(&mut self.ui.status, p.idle()) else { return };
        if discard {
            return;
        }
        for (t, slot, param, value) in edits {
            if slot == transit_mix::INSTRUMENT_SLOT {
                continue;
            }
            let key = format!("param:{}:{slot}:{param}", t.0);
            let _ = self.engine.execute_merged("mix.insert_param", &json!({"track": t.0, "slot": slot, "param": param, "value": value}), &key);
        }
    }

    /// Whether the meters are live: while playing, and while armed or input-monitored tracks are heard.
    fn meters_live(&self) -> bool {
        self.engine.transport.playing
            || (self.recorder.is_some() && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm))
    }

    /// Reopens the audio devices with `cfg` (Setup › Playback Engine › Apply), keeping the
    /// transport position, and keeps it as `ui.audio`. Plugin instances and their editors are
    /// recreated with the new player. Refused while recording, which would lose the take; `ui.audio`
    /// then stays the configuration of the open device.
    pub fn apply_audio_config(&mut self, cfg: transit_playback::AudioConfig) -> Result<(), String> {
        self.apply_audio_config_using(cfg, Player::with_config)
    }

    fn apply_audio_config_using(
        &mut self,
        cfg: transit_playback::AudioConfig,
        open: impl FnOnce(std::sync::Arc<transit_model::Session>, &transit_playback::AudioConfig) -> Player,
    ) -> Result<(), String> {
        if self.recording_active() {
            return Err("stop recording before changing the audio device".into());
        }
        let captured = self.preserve_audio_state()?;
        self.ui.audio = cfg;
        let at = self.position();
        if let Some(p) = &self.player {
            if let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.stop()) {
                self.player_requests.stop = Some(sequence);
            }
            // The old player's plugin instances go away with it; their editor windows must be
            // closed here, on the main thread, or they would stay on screen orphaned.
            if self.player_requests.result(&mut self.ui.status, p.close_editors()).is_none() {
                return Err(self.ui.status.clone());
            }
        }
        // The input reopens on the next record or monitor with the new choice.
        self.close_input();
        if self.recorder.is_some() {
            return Err(self.ui.status.clone());
        }
        self.recorder_failed = false;
        // Close the old device before opening the new one: an ASIO driver is process-wide, and
        // dropping the old stream after the new one opened unloaded the driver from under it.
        self.player = None;
        let player = open(self.engine.session_arc(), &self.ui.audio);
        if let Some(sequence) = self.player_requests.result(&mut self.ui.status, player.locate(at)) {
            self.player_requests.locate = Some(sequence);
        }
        self.player = Some(player);
        self.player_requests.play = None;
        self.player_requests.stop = None;
        self.player_requests.recording = None;
        self.player_requests.release = None;
        if !captured {
            self.ui.status = "Audio recovered from the document state; live plug-in state was unavailable after the audio fault.".into();
        }
        Ok(())
    }

    /// Done with the input, if open: it goes back to the player (`Player::close_input`), which
    /// keeps an input it opened with the output (ASIO) and closes any other.
    pub fn close_input(&mut self) {
        if self.recording_active() {
            self.finish_recording();
        }
        if let Some(p) = &self.player {
            self.player_requests.result(&mut self.ui.status, p.close_input(&mut self.recorder));
        } else {
            self.recorder = None;
        }
    }

    fn recording_active(&self) -> bool {
        self.engine.transport.recording
            || self.take_plan.is_some()
            || self.recorder.as_ref().is_some_and(transit_playback::record::Recorder::has_pending_take)
    }

    fn poll_player(&mut self) {
        let Some(p) = &self.player else { return };
        let polled = p.poll_notices(&mut self.player_requests.notices);
        let count = self.player_requests.result(&mut self.ui.status, polled);
        let mut collect = false;
        for index in 0..count.unwrap_or(0).min(self.player_requests.notices.len()) {
            if let Some(notice) = self.player_requests.notices.get_mut(index).and_then(Option::take) {
                collect |= self.player_requests.notice(notice, &mut self.ui.status, &mut self.engine.transport.recording);
            }
        }
        let stopped = self
            .player_requests
            .result(&mut self.ui.status, p.recovery_progress())
            .is_some_and(|frontiers| self.player_requests.recovery(frontiers, &mut self.ui.status));
        if stopped && !p.is_playing() {
            self.engine.transport.position = p.position();
            self.complete_stop();
        }
        if collect && self.recording_active() {
            self.finish_recording();
        }
        if let Some(point) = self.recorder.as_ref().and_then(transit_playback::record::Recorder::applied_start)
            && self.take_plan.as_ref().is_some_and(|plan| plan.start != point.position.sample)
        {
            self.record_start = point.position.sample;
            if let Some(plan) = &mut self.take_plan {
                plan.start = self.record_start;
            }
            if let Some(root) = self.autosave_dir.clone() {
                self.queue_autosave(root);
            }
        }
    }

    /// Take the input once (for recording and input monitoring) from the player and feed the
    /// player: on ASIO the one it opened with the output, elsewhere a newly opened device.
    fn ensure_input(&mut self) {
        if self.recorder.is_some() || self.recorder_failed {
            return;
        }
        let Some(p) = &self.player else { return };
        match p.open_input(&self.ui.audio) {
            Ok(r) => {
                self.recorder_failed = self.player_requests.result(&mut self.ui.status, p.set_input(std::sync::Arc::clone(&r.monitor))).is_none();
                self.recorder = Some(r);
            }
            Err(e) => {
                self.recorder_failed = true;
                self.ui.status = format!("No audio input: {e}");
            }
        }
    }

    /// Stop the transport, keeping a take in progress, when the audio output fails.
    fn watch_output(&mut self) {
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
        let (cause, then) = match trouble {
            OutputTrouble::Bug => (
                "the audio engine hit an internal error (see the log)".to_string(),
                "Use Reconnect audio device in Setup ▸ Playback Engine, or restart SoundCraft.",
            ),
            OutputTrouble::Gone => {
                (format!("“{name}” was disconnected"), "Plug it back in, then use Reconnect audio device in Setup ▸ Playback Engine.")
            }
            OutputTrouble::Stalled => (format!("“{name}” stopped asking for audio"), "Use Reconnect audio device in Setup ▸ Playback Engine."),
            OutputTrouble::ResetRequested => {
                (transit_playback::RESET_REQUESTED_TEXT.to_string(), "Use Reconnect audio device in Setup ▸ Playback Engine.")
            }
        };
        let (recording, playing) = (self.recording_active(), self.engine.transport.playing);
        if recording || playing {
            self.stop_play();
        }
        self.ui.status = if recording {
            format!("Recording stopped: {cause}. The take is kept up to there. {then}")
        } else if playing {
            format!("Playback stopped: {cause}. {then}")
        } else {
            format!("Audio output: {cause}. {then}")
        };
    }

    /// Stop recording, keeping the take, when the input device or the disk fails, and say as soon
    /// as a slow disk loses input.
    fn watch_input(&mut self) {
        let recording = self.recording_active();
        let Some(r) = &mut self.recorder else { return };
        let Some(trouble) = r.trouble() else {
            let lost = r.lost_ms();
            if recording && lost > self.lost_shown {
                self.lost_shown = lost;
                self.ui.status = format!("The disk is too slow: this take has lost {lost} ms of input so far.");
            }
            return;
        };
        if recording {
            self.stop_play();
        }
        let (what, then) = match trouble {
            Trouble::Device(what) => {
                self.close_input();
                self.recorder_failed = true;
                (what, "Reconnect it, then use Reconnect audio device in Setup ▸ Playback Engine.")
            }
            Trouble::Disk(what) => (what, "Check the disk, then record again."),
        };
        self.ui.status = if recording {
            format!("Recording stopped: {what}. The take is kept up to there. {then}")
        } else {
            format!("Audio input: {what}. {then}")
        };
    }

    /// Reopen the audio devices, keeping a take in progress.
    pub fn reconnect_audio(&mut self) {
        match self.reconnect_audio_using(true, Player::with_config) {
            Ok(false) => self.ui.status = "Audio recovered from the document state; live plug-in state was unavailable after the audio fault.".into(),
            Ok(true) => {}
            Err(error) => self.player_requests.fail(&mut self.ui.status, error),
        }
    }

    pub(crate) fn reconnect_audio_using(
        &mut self,
        preserve: bool,
        open: impl FnOnce(std::sync::Arc<transit_model::Session>, &transit_playback::AudioConfig) -> Player,
    ) -> Result<bool, String> {
        let captured = !preserve || self.preserve_audio_state()?;
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        if let Some(p) = &self.player {
            // Its plugin editors go with it; closed here, on the main thread (see `apply_audio_config`).
            if self.player_requests.result(&mut self.ui.status, p.close_editors()).is_none() {
                return Err(self.ui.status.clone());
            }
        }
        self.close_input();
        if self.recorder.is_some() {
            return Err(self.ui.status.clone());
        }
        self.recorder_failed = false;
        // The old device closes first (see `apply_audio_config`: an ASIO driver is process-wide).
        self.player = None;
        self.player = Some(open(self.engine.session_arc(), &self.ui.audio));
        self.player_requests.reset();
        Ok(captured)
    }

    /// Per-frame logic: control channel, transport, document sync.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if ctx.current_pass_index() > 0 {
            return;
        }
        // Fonts set now take effect next frame, so draw only from the frame after.
        if !self.fonts_ready {
            if self.fonts_installed {
                self.fonts_ready = true;
            } else {
                fonts::install(ctx);
                self.fonts_installed = true;
                ctx.request_repaint();
            }
        }
        let light = theme::wants_light(ctx, self.ui.theme);
        if self.theme_applied != Some(light) || theme::is_light() != light {
            theme::apply(ctx, light);
            self.theme_applied = Some(light);
        }
        let now = ctx.input(|i| i.time);
        let dt = self.last_frame.replace(now).map_or(1.0 / 60.0, |t| (now - t) as f32).clamp(0.0, 0.25);
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;
        control::drain(self, ctx);
        self.note_recent_session();
        let plugin_requests = service_plugin_requests();
        self.apply_editor_edits();
        self.poll_player();
        bounce::step(self, ctx);
        if !ctx.input(|i| i.pointer.any_down()) {
            if matches!(self.gesture, Some(Gesture::Fader { .. })) {
                self.gesture = None;
            }
            // Keep a running automation pass merged until playback stops.
            if !ctx.egui_wants_keyboard_input() && self.touched.is_empty() {
                self.engine.end_merge();
            }
        }
        self.watch_output();
        self.watch_input();
        // Stop the old playback before sending a replacement session to the audio thread.
        self.handle_transport(dt);
        if self.player_requests.play.is_none() && self.player_requests.locate.is_none() && self.player_requests.recording.is_none() {
            self.sync_player_session();
        }
        self.autosave_changes();
        self.write_automation(ctx);
        self.autosave();
        if self.recorder.is_none()
            && !self.recorder_failed
            && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm)
        {
            self.ensure_input();
        }
        // Autosave and the macOS menu bar only run on frames, and plugin editors are native windows egui never hears from.
        // egui takes its predicted frame time off every delay, so add it back.
        let editors = self.player.as_ref().is_some_and(Player::any_editor_open);
        ctx.request_repaint_after_secs(
            if editors || plugin_requests || self.player_requests.pending() { 0.03 } else { 1.0 } + ctx.input(|i| i.predicted_dt),
        );
        self.player_requests.show_error(&mut self.ui.status);
        control::collect_screenshots(self, ctx);
    }

    /// Feed queued synthetic input (from the control channel) into egui: one pointer event per
    /// frame (so drags span frames like real ones), keys as press+release pairs, text at once.
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        // While a synthetic gesture plays (and briefly after), the real pointer is ignored so it
        // cannot interleave with the scripted one.
        if !self.synthetic.is_empty() {
            self.synthetic_grace = 6;
        }
        if self.synthetic_grace > 0 {
            self.synthetic_grace -= 1;
            raw.events.retain(|e| {
                !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone | egui::Event::MouseMoved(_))
            });
        }
        let Some(first) = self.synthetic.first().cloned() else { return };
        let take = match first {
            egui::Event::Key { pressed: true, .. } => {
                // Press and the matching release together.
                if matches!(self.synthetic.get(1), Some(egui::Event::Key { pressed: false, .. })) { 2 } else { 1 }
            }
            _ => 1,
        };
        let n = take.min(self.synthetic.len());
        let evs: Vec<egui::Event> = self.synthetic.drain(..n).collect();
        for e in &evs {
            if let egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) = e {
                // Keep the hover position current for widgets that check it.
                raw.events.push(egui::Event::PointerMoved(*pos));
            }
        }
        raw.events.extend(evs);
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // The integration creates this Ui before logic resolves the current frame's palette.
        ui.set_style(ctx.global_style());
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        if self.arrange_request {
            self.arrange_request = false;
            ctx.memory_mut(|m| m.reset_areas());
        }
        // The MIDI Keyboard takes its keys before the shortcuts see them.
        midi_keyboard::capture(self, &ctx);
        shortcuts::handle(self, &ctx);
        self.sample_drop_targets.clear();
        if !self.native_menu_bar {
            menus::menu_bar(self, ui);
        }
        // With the Mix window in its own native window, the main window keeps Edit.
        match if self.ui.mix_separate { MainWindow::Edit } else { self.ui.window } {
            MainWindow::Edit => edit_window::show(self, ui),
            MainWindow::Mix => mix_window::show(self, ui),
        }
        if self.ui.mix_separate {
            mix_window::show_separate(self, &ctx);
        }
        panels::floating(self, &ctx);
        windows::show(self, &ctx);
        plugin_folders::scan_notice(&ctx);
        video_window::show(self, &ctx);
        ops_windows::show(self, &ctx);
        extra_windows::show(self, &ctx);
        recovery::show(self, &ctx);
        score_editor::show(self, &ctx);
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        // Here, not in logic(): a covered or minimized window runs only logic(), with no vsync to pace it.
        if self.meters_live() {
            let hz = if self.ui.window == MainWindow::Mix { 60 } else { 30 };
            ctx.request_repaint_after(std::time::Duration::from_millis(1000 / hz));
        } else if self.meters.values().any(|m| m.level[0] > 0.0001) {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else if !self.pending_shots.is_empty() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    /// UI state as JSON (control channel `ui.inspect`).
    pub fn inspect(&self, ctx: &egui::Context) -> Value {
        let size = ctx.content_rect().size();
        json!({
            "ui": self.ui,
            "window_size": [size.x, size.y],
            "playing": self.is_playing(),
            "position": self.position(),
            "dialog": self.dialogs.open_name(),
            "frame_ms": self.frame_ms,
            "audio_device": self.player.as_ref().map(|p| p.device_name.clone()),
            "status": self.ui.status,
        })
    }
}

fn feed_meter(d: &mut MeterDisplay, peak: [f32; 2], gr: f32, dt: f32) {
    // Instant attack, ~26 dB/s release, 2 s peak hold.
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    for c in 0..2 {
        let v = peak.get(c).copied().unwrap_or(0.0);
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < 1e-5 { 0.0 } else { nv };
        }
        if v >= d.hold.get(c).copied().unwrap_or(0.0) {
            if let Some(h) = d.hold.get_mut(c) {
                *h = v;
            }
            d.hold_age = 0.0;
        }
        if v >= 1.0 {
            d.clip = true;
        }
    }
    d.hold_age += dt;
    if d.hold_age > 2.0 {
        d.hold = d.level;
    }
    d.gr = if gr > d.gr { gr } else { d.gr * fall };
}

/// The status line naming the plugin in `slot` (an insert index or
/// [`transit_mix::INSTRUMENT_SLOT`]), which the engine took out of the signal path.
fn failed_plugin_status(s: &transit_model::Session, track: TrackId, slot: usize) -> Option<String> {
    let t = s.track(track)?;
    let ins = if slot == transit_mix::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(slot)?.as_ref() }?;
    let name = crate::mix_window::plugin_info(&ins.plugin).map_or(ins.plugin.as_str(), |p| p.name);
    Some(format!("{name} on {} put out invalid audio, so it is bypassed until playback stops or starts again.", t.name))
}

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

/// Admission is separate from callback application. Storage is reused every frame,
/// including idle frames, so terminal results always return admission credits.
struct PlayerRequests {
    notices: [Option<transit_playback::PlayerNotice>; 64],
    play: Option<transit_playback::action_lane::Sequence>,
    locate: Option<transit_playback::action_lane::Sequence>,
    stop: Option<transit_playback::action_lane::Sequence>,
    release: Option<transit_playback::action_lane::Sequence>,
    recording: Option<transit_playback::action_lane::Sequence>,
    error: Option<String>,
}

impl Default for PlayerRequests {
    fn default() -> Self {
        Self { notices: [None; 64], play: None, locate: None, stop: None, release: None, recording: None, error: None }
    }
}

impl PlayerRequests {
    fn reset(&mut self) {
        let error = self.error.take();
        *self = Self { error, ..Self::default() };
    }

    fn pending(&self) -> bool {
        self.play.is_some() || self.locate.is_some() || self.stop.is_some() || self.release.is_some() || self.recording.is_some()
    }

    fn fail(&mut self, status: &mut String, message: String) {
        log::warn!("{message}");
        let error = self.error.get_or_insert_with(String::new);
        if !error.is_empty() {
            error.push_str("; ");
        }
        error.push_str(&message);
        status.clone_from(error);
    }

    fn result<T>(&mut self, status: &mut String, result: Result<T, transit_playback::PlayerError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                if error == transit_playback::PlayerError::Faulted {
                    self.reset();
                }
                self.fail(status, error.to_string());
                None
            }
        }
    }

    /// Returns true when a refused start requires collecting an armed take.
    fn notice(&mut self, notice: transit_playback::PlayerNotice, status: &mut String, recording: &mut bool) -> bool {
        use transit_playback::{ActionResult, PlayerCommand, PlayerNotice, action_lane::Outcome, player::SessionOutcome};
        match notice {
            PlayerNotice::Session(n) => {
                if let SessionOutcome::Rejected(reason) = n.outcome {
                    self.fail(status, format!("Audio session {} rejected: {reason:?}", n.request.get()));
                }
            }
            PlayerNotice::Action(r) => {
                let applied = r.outcome == Outcome::Applied
                    && matches!(
                        r.handler_result,
                        Some(
                            ActionResult::Applied
                                | ActionResult::RecordingStarted(_)
                                | ActionResult::Event(
                                    transit_mix::EventOutcome::Queued
                                        | transit_mix::EventOutcome::QueuedLate { .. }
                                        | transit_mix::EventOutcome::ReleaseAdvanced { .. }
                                        | transit_mix::EventOutcome::Coalesced
                                )
                        )
                    );
                if self.locate == Some(r.sequence) {
                    self.locate = None;
                }
                let play = self.play == Some(r.sequence);
                let record = self.recording == Some(r.sequence);
                if play {
                    self.play = None;
                    if applied && status == "Play pending" {
                        *status = "Playing".into();
                    }
                }
                if record {
                    self.recording = None;
                    if applied {
                        match r.payload {
                            PlayerCommand::Recording(on) => *recording = on,
                            PlayerCommand::Record { .. } => {
                                *recording = true;
                                if status == "Record pending" {
                                    *status = "Recording".into();
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if !applied && !matches!(r.outcome, Outcome::Stopped | Outcome::NotesReleased) {
                    self.fail(status, format!("Audio action {} refused: {:?} ({:?})", r.sequence.get(), r.outcome, r.handler_result));
                }
                return (play || record) && !applied && self.stop.is_none();
            }
        }
        false
    }

    fn recovery(
        &mut self,
        frontiers: (transit_playback::action_lane::Sequence, transit_playback::action_lane::Sequence),
        status: &mut String,
    ) -> bool {
        let stopped = self.stop.is_some_and(|s| frontiers.0 >= s);
        if stopped {
            self.stop = None;
            if status == "Stop pending" {
                *status = "Stopped".into();
            }
        }
        if self.release.is_some_and(|s| frontiers.1 >= s) {
            self.release = None;
            *status = "All MIDI notes off".into();
        }
        stopped
    }

    fn show_error(&mut self, status: &mut String) {
        if let Some(error) = self.error.take() {
            *status = error;
        }
    }
}

#[cfg(test)]
#[path = "player_tests.rs"]
mod player_tests;

#[cfg(test)]
mod tests;
