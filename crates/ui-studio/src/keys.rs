//! The piano roll (key editor, plan 06 slice 3): a MIDI clip opened by a double-click in the
//! arrangement, shown in the bottom pane instead of the mixer and device chain (Ableton's clip
//! view). A tab strip in its header switches back; Ctrl+Alt+K toggles it; Escape closes it.
//!
//! No draw tool (Ableton): a double-click on empty grid adds a note, a double-click on a note
//! deletes it; a click selects, Shift-click adds, a drag on empty grid selects with a rubber band;
//! dragging notes moves them (Ctrl copies, Alt+Shift moves without snap), either edge resizes (Alt:
//! without snap), Alt+drag on a body or the velocity lane sets velocity. Every edit is a `midi.*` / `event.*` command, one undo step per gesture.
//! Notes are addressed by index and indices shift after each edit (plan 06, section 1), so the
//! selection is kept as note values and resolved to indices again before each command.
//! Zoom, scroll, snap, pane height, new-note velocity and the audition switch are view state: [`KeysPrefs`] in
//! [`crate::UiState`] and [`KeysState`] here, never the document.

mod view;
pub use view::{pane, switcher_panel};

mod cc;
mod pitch;
mod quantize;
pub use quantize::QuantizePrefs;
mod scale;
#[cfg(test)]
mod scale_tests;
mod step;
pub use step::StepState;

use crate::StudioApp;
use crate::arrange::R4;
use crate::grid::{self, GridMode, GridView};
use crate::mixer::MenuItem;
use crate::theme::{self, TOKENS};
use egui::{Align2, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use transit_midi::Note;
use transit_model::{ClipContent, ClipId, Session, TrackId};
use transit_time::{Samples, TICKS_PER_QUARTER as PPQ};

/// Width of the keyboard column, heights of the pane's rows (points).
pub const KEY_W: f32 = 52.0;
const HANDLE_H: f32 = 5.0;
const HEADER_H: f32 = 28.0;
const RULER_H: f32 = 20.0;
const VEL_H: f32 = 52.0;
/// How close (points) to either note edge a press resizes instead of moving.
const EDGE: f32 = 6.0;
/// Pane height limits.
const MIN_H: f32 = 160.0;

/// The Grid menu's entries. Each one sets the piano roll's [`GridView`]: "Adaptive" follows the
/// zoom, the note values pin [`GridMode::Fixed`] (triplets also turn the triplet scaling on),
/// "Off" switches snapping off. Ctrl+1 and Ctrl+2 can pin divisions the menu does not list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    Adaptive,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
    Off,
}

impl Snap {
    pub const ALL: [Snap; 8] =
        [Snap::Adaptive, Snap::Quarter, Snap::Eighth, Snap::Sixteenth, Snap::ThirtySecond, Snap::EighthTriplet, Snap::SixteenthTriplet, Snap::Off];

    pub fn label(self) -> &'static str {
        match self {
            Snap::Adaptive => "Adaptive",
            Snap::Quarter => "1/4",
            Snap::Eighth => "1/8",
            Snap::Sixteenth => "1/16",
            Snap::ThirtySecond => "1/32",
            Snap::EighthTriplet => "1/8T",
            Snap::SixteenthTriplet => "1/16T",
            Snap::Off => "Off",
        }
    }

    /// The pinned division (before the triplet scaling) and whether it is a triplet; `None` for
    /// the entries that are not a note value.
    fn fixed(self) -> Option<(i64, bool)> {
        match self {
            Snap::Quarter => Some((PPQ, false)),
            Snap::Eighth => Some((PPQ / 2, false)),
            Snap::Sixteenth => Some((PPQ / 4, false)),
            Snap::ThirtySecond => Some((PPQ / 8, false)),
            Snap::EighthTriplet => Some((PPQ / 2, true)),
            Snap::SixteenthTriplet => Some((PPQ / 4, true)),
            Snap::Adaptive | Snap::Off => None,
        }
    }

    /// Choose this entry on the piano roll's grid.
    pub fn apply(self, g: &mut GridView) {
        match (self, self.fixed()) {
            (Snap::Off, _) => g.snap_enabled = false,
            (_, Some((base, triplet))) => *g = GridView { mode: GridMode::Fixed(base), triplet, snap_enabled: true },
            _ => *g = GridView::default(),
        }
    }

    /// The entry that matches `g`, when there is one (a division pinned by Ctrl+1 may have none).
    pub fn of(g: &GridView) -> Option<Snap> {
        if !g.snap_enabled {
            return Some(Snap::Off);
        }
        match g.mode {
            GridMode::Adaptive if !g.triplet => Some(Snap::Adaptive),
            GridMode::Adaptive => None,
            GridMode::Fixed(t) => Snap::ALL.into_iter().find(|s| s.fixed() == Some((t, g.triplet))),
        }
    }
}

/// Points per tick at the piano roll's horizontal zoom (the lines, the snap and the step length all ask this).
fn keys_ppt(prefs: &KeysPrefs) -> f64 {
    f64::from(prefs.px_per_quarter.clamp(8.0, 800.0)) / PPQ as f64
}

/// The piano roll's grid step in ticks at its zoom: what the lines are drawn at, what notes snap
/// to (when snapping is on), and the length of a new or step-entered note.
pub fn keys_grid_step(s: &Session, prefs: &KeysPrefs) -> i64 {
    crate::grid::ticks(keys_ppt(prefs), crate::grid::bar_ticks(s), &prefs.grid)
}

/// The piano roll's view preferences (saved with the UI state; never in the document).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KeysPrefs {
    /// The piano roll replaces the mixer and device chain panes (when a clip is open).
    pub show: bool,
    /// Pane height in points.
    pub height: f32,
    /// Horizontal zoom: points per quarter note (60 = a 4/4 bar in 240 points).
    pub px_per_quarter: f32,
    /// Vertical zoom: key row height in points.
    pub key_h: f32,
    /// Play notes on the clip's instrument as they are placed, clicked or dragged.
    pub audition: bool,
    /// The grid: adaptive to the zoom or pinned, triplets, snap on or off (the arrangement has its own).
    pub grid: GridView,
    /// Step entry: enter every note at `step_velocity` instead of the velocity it was played at.
    pub step_fixed: bool,
    /// The velocity of step-entered notes when `step_fixed`.
    #[serde(deserialize_with = "read_velocity")]
    pub step_velocity: u8,
    /// Hold threshold and repeat floor for step entry.
    pub step_hold: transit_engine::step_input::HoldPrefs,
    /// Opt-in Space binding, scoped to the focused piano roll in step mode.
    pub step_tie_rest_space: bool,
    pub step_bindings: transit_engine::step_input::StepBindings,
    /// Legacy UI preference retained for settings compatibility. Editing uses the clip's key.
    pub key_scale: transit_midi::scale::KeyScale,
    pub quantize: QuantizePrefs,
    /// Explicit velocity for new notes and audition; note edits never change it.
    #[serde(deserialize_with = "read_velocity")]
    pub velocity: u8,
}

fn read_velocity<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let value: i64 = serde::Deserialize::deserialize(d)?;
    Ok(value.clamp(1, 127) as u8)
}

impl Default for KeysPrefs {
    fn default() -> Self {
        KeysPrefs {
            show: false,
            height: 340.0,
            px_per_quarter: 60.0,
            key_h: 16.0,
            audition: true,
            grid: GridView::default(),
            step_fixed: false,
            step_velocity: 100,
            step_hold: transit_engine::step_input::HoldPrefs::default(),
            step_tie_rest_space: false,
            step_bindings: transit_engine::step_input::StepBindings::default(),
            key_scale: transit_midi::scale::KeyScale::default(),
            quantize: QuantizePrefs::default(),
            velocity: 100,
        }
    }
}

/// What a drag on notes does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum NoteDrag {
    Move,
    Resize,
    ResizeStart,
    /// Alt+drag on a note body: velocity by vertical distance.
    Velocity,
    /// A drag in the velocity lane: velocity from the pointer's height.
    Lane,
}

/// A gesture in progress in the pane.
#[derive(Debug, Clone)]
pub enum KeysGesture {
    Notes {
        kind: NoteDrag,
        anchor: Note,
        press: Pos2,
        copy: bool,
        sounding: u8,
    },
    Band {
        press: Pos2,
        additive: bool,
    },
    Height {
        start: f32,
        press: Pos2,
    },
    Ruler {
        ppq: f32,
        anchor: f64,
        press: Pos2,
    },
    Keyboard {
        key_h: f32,
        anchor: f32,
        press: Pos2,
    },
    /// Dragging the clip's end in the ruler (slice 6).
    ClipEnd,
}

/// One note as last drawn (for `ui.inspect`; scripts click and drag them).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct NoteRect {
    pub index: usize,
    pub pitch: u8,
    pub start: i64,
    pub length: i64,
    pub velocity: u8,
    pub selected: bool,
    /// Starts at or past the clip's end: it never sounds (drawn dimmed and hatched).
    pub outside: bool,
    pub rect: R4,
    /// The note's stem in the velocity lane.
    pub vel_rect: R4,
}

/// One repeat of a looped clip's cycle drawn as a dimmed ghost note (not interactive).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GhostNote {
    pub pitch: u8,
    pub start: i64,
    pub length: i64,
}

/// Last frame's piano roll geometry and state.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct KeysLayout {
    pub cc: cc::Layout,
    pub visible: bool,
    pub clip: Option<u64>,
    pub track: Option<u64>,
    /// The pane takes the keyboard (Delete, arrows, Ctrl+A/D/U, Escape).
    pub active: bool,
    pub pane: R4,
    /// The pane's top edge: drag to resize.
    pub handle: R4,
    pub tab_keys: R4,
    pub tab_mixer: R4,
    pub tab_devices: R4,
    pub snap: R4,
    pub snap_menu: Vec<MenuItem>,
    pub quantize: R4,
    pub quantize_apply: R4,
    pub quantize_cancel: R4,
    pub quantize_preview: Vec<Note>,
    pub pitch: R4,
    pub pitch_menu: Vec<MenuItem>,
    pub audition: R4,
    /// The Step toggle, whether Step entry is on, and its cursor and step length in ticks.
    pub step: R4,
    pub step_on: bool,
    pub step_cursor: i64,
    pub step_ticks: i64,
    pub new_note_velocity: R4,
    pub use_selected_velocity: R4,
    /// Exact length inspector for a single selected note, in ticks.
    pub note_length: R4,
    /// Lengths painted beside the pointer during a note gesture.
    pub drag_lengths: Vec<i64>,
    pub ruler: R4,
    pub keyboard: R4,
    /// Visible pitches and keyboard row rectangles, including folded accidental rows.
    pub pitch_rows: Vec<(u8, R4)>,
    pub grid: R4,
    pub velocity: R4,
    /// Mapping: x = grid[0] + (tick - scroll_ticks) * px_per_tick, ticks relative to the clip;
    /// in chromatic view y = grid[1] + (top_pitch - p) * key_h; folded views use pitch_rows.
    pub px_per_tick: f64,
    pub scroll_ticks: f64,
    pub top_pitch: f32,
    pub key_h: f32,
    /// The clip's start (absolute ticks) and length in ticks.
    pub clip_start_ticks: i64,
    pub clip_ticks: i64,
    /// A looped clip (ADR 0019): the arrangement length in ticks, the loop cycle `(start, end)` in ticks
    /// (`clip_ticks` is then `end`, the first pass the roll edits), and the repeats drawn as ghosts.
    pub looped_to_ticks: Option<i64>,
    pub loop_cycle: Option<(i64, i64)>,
    /// The one-shot lead-in before the cycle's start, in ticks (0 when the cycle starts at the clip start).
    pub lead_in_ticks: i64,
    pub loop_ghosts: Vec<GhostNote>,
    /// The clip-end handle in the ruler (slice 6): drag to lengthen or shorten the clip.
    pub end_handle: R4,
    pub grid_ticks: Option<i64>,
    /// The grid step in force (ticks) even with snap off, and the corner's text.
    pub grid_step: i64,
    pub grid_text: String,
    /// Step velocity control.
    pub step_velocity: R4,
    pub notes: Vec<NoteRect>,
    /// Instrument/MIDI tracks with no clips, which show the "create a clip" hint (set by the arrangement).
    pub hint_tracks: Vec<u64>,
    pub selected: usize,
    /// Notes auditioned so far, and the last one as (pitch, velocity): with or without a player.
    pub auditions: u64,
    pub last_audition: Option<(u8, u8)>,
    /// Auditioned pitches still sounding.
    pub sounding: Vec<u8>,
    /// Keys held on an armed MIDI input, as (pitch, velocity), lowest first.
    pub live: Vec<(u8, u8)>,
}

#[derive(Debug)]
pub struct KeysState {
    pub cc: cc::State,
    pub quantize_open: bool,
    /// The clip being edited.
    pub clip: Option<ClipId>,
    /// Selected notes, by value (indices shift after every edit).
    pub selection: Vec<Note>,
    pub gesture: Option<KeysGesture>,
    /// Left edge of the grid in ticks from the clip start.
    pub scroll: f64,
    /// The pitch whose row is at the grid's top edge (fractional when scrolled).
    pub top: f32,
    /// Center the view on the notes on the next frame (after opening a clip).
    fit: bool,
    pub active: bool,
    /// Auditioned notes still sounding: (track, pitch, earliest release time).
    held: Vec<(TrackId, u8, f64)>,
    pub auditions: u64,
    pub last_audition: Option<(u8, u8)>,
    /// Step entry (session state, never saved).
    pub step: StepState,
    /// Fractional velocity carried between frames of a drag on the step velocity control.
    step_vel_drag: f32,
    length_edit: Option<(Note, String)>,
    /// Velocity per pitch of the keys held on an armed MIDI input (0 = up). Session state.
    pub live: [u8; 128],
    /// The tracks' MIDI inputs as last seen: a change lets go of every live key.
    live_inputs: Vec<transit_model::MidiInput>,
    serial: u64,
    /// The last click on the grid (time, position), for double-clicks.
    last_click: Option<(f64, Pos2)>,
    pub layout: KeysLayout,
    /// The Piano Roll / Mixer / Devices switcher as last drawn above the mixer or device panes.
    pub switcher: [R4; 3],
}

impl Default for KeysState {
    fn default() -> Self {
        KeysState {
            cc: cc::State::default(),
            quantize_open: false,
            clip: None,
            selection: Vec::new(),
            gesture: None,
            scroll: 0.0,
            top: 0.0,
            fit: false,
            active: false,
            held: Vec::new(),
            auditions: 0,
            last_audition: None,
            step: StepState::default(),
            step_vel_drag: 0.0,
            length_edit: None,
            live: [0; 128],
            live_inputs: Vec::new(),
            serial: 0,
            last_click: None,
            switcher: Default::default(),
            layout: KeysLayout::default(),
        }
    }
}

fn r4(r: Rect) -> R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

fn is_black(p: u8) -> bool {
    matches!(p % 12, 1 | 3 | 6 | 8 | 10)
}

/// The clip's track, start tick, length in ticks, notes and name.
struct ClipView {
    track: TrackId,
    c0: i64,
    /// The span the roll edits: the arrangement length, or the first pass `0..loop.end` of a looped clip.
    len: i64,
    /// The arrangement length in ticks (equal to `len` unless looped).
    full: i64,
    /// The loop cycle `(start, end)` in ticks from the clip start, when the clip has a valid loop.
    cycle: Option<(i64, i64)>,
    notes: Vec<Note>,
    ctrls: Vec<transit_midi::CtrlEvent>,
    name: String,
}

fn clip_view(s: &Session, clip: ClipId) -> Option<ClipView> {
    let (track, c) = s.find_clip(clip)?;
    let ClipContent::Midi { sequence } = &c.content else { return None };
    let c0 = s.tempo.samples_to_ticks(c.start, s.sample_rate);
    let full = s.tempo.samples_to_ticks(c.end(), s.sample_rate) - c0;
    // A valid loop: the roll edits the first pass and the cycle (the tick frame playback and the arranger use).
    let cycle = c
        .looping
        .filter(|l| l.is_valid())
        .map(|l| {
            let (origin, f) = transit_mix::loop_frame(s, c.start, l);
            let o = s.tempo.samples_to_ticks(origin, s.sample_rate);
            let tick = |x: i64| s.tempo.samples_to_ticks(origin.saturating_add(x), s.sample_rate) - o;
            (tick(f.start), tick(f.end))
        })
        .filter(|(a, b)| b > a && *b > 0);
    let len = cycle.map_or(full, |(_, end)| end.min(full));
    Some(ClipView { track, c0, len, full, cycle, notes: sequence.notes.clone(), ctrls: sequence.ctrls.clone(), name: c.name.clone() })
}

/// Most ghost notes drawn for a clip's repeats (a degenerate cycle could ask for millions).
const MAX_GHOSTS: usize = 4096;
/// Most repeat cycles one frame walks for the ghosts.
const MAX_GHOST_CYCLES: usize = 4096;
/// A ghost repeat is a faint fill with a thin outline. The arranger's `GHOST_ALPHA` reads dim over a dark clip
/// body but near full over the grid (a real note is never fainter than 0.45), so the roll uses its own.
const GHOST_FILL_ALPHA: f32 = 0.16;
const GHOST_LINE_ALPHA: f32 = 0.5;
/// The faintest fill a real note draws with (velocity 0 in `paint_notes`).
const NOTE_MIN_ALPHA: f32 = 0.45;

/// The fill and outline of a ghost repeat.
fn ghost_colors() -> (egui::Color32, egui::Color32) {
    (TOKENS.accent.gamma_multiply(GHOST_FILL_ALPHA), TOKENS.accent.gamma_multiply(GHOST_LINE_ALPHA))
}

impl ClipView {
    /// The repeats of a looped clip's cycle from `loop.end` to the arrangement length (ADR 0019): the
    /// notes that start inside the cycle, shifted one cycle per repeat, each cut at its seam and at the end.
    /// Only the repeats that touch the view `view.0..view.1` (ticks) are built, at most `MAX_GHOST_CYCLES`
    /// cycles and `MAX_GHOSTS` notes; the second value is the number of cycles walked.
    fn ghosts(&self, view: (i64, i64)) -> (Vec<GhostNote>, usize) {
        let Some((start, end)) = self.cycle else { return (Vec::new(), 0) };
        let cycle = end - start;
        let (lo, hi) = (view.0.max(end), view.1.min(self.full));
        if lo >= hi {
            return (Vec::new(), 0);
        }
        let cycle_notes: Vec<&Note> = self.notes.iter().filter(|n| n.start >= start && n.start < end).collect();
        let mut out = Vec::new();
        // Repeat k covers `end + (k - 1) * cycle .. end + k * cycle`; start at the first one in view.
        let first = ((lo - end) / cycle + 1).max(1);
        let mut walked = 0;
        for k in first.. {
            if end + (k - 1) * cycle >= hi || walked >= MAX_GHOST_CYCLES {
                break;
            }
            walked += 1;
            for n in &cycle_notes {
                let at = n.start + k * cycle;
                if at >= self.full {
                    continue;
                }
                let seam = end + k * cycle;
                out.push(GhostNote { pitch: n.pitch, start: at, length: n.length.min(seam - at).min(self.full - at).max(1) });
                if out.len() >= MAX_GHOSTS {
                    return (out, walked);
                }
            }
        }
        (out, walked)
    }
}

/// The clip-length readout: "Clip length 1.00 bars", and for a looped clip ", looped to 4.00".
pub fn clip_length_text(len_ticks: i64, looped_to: Option<i64>) -> String {
    let bars = |t: i64| t as f64 / (PPQ * 4) as f64;
    match looped_to {
        Some(full) if full != len_ticks => format!("Clip length {:.2} bars, looped to {:.2}", bars(len_ticks), bars(full)),
        _ => format!("Clip length {:.2} bars", bars(len_ticks)),
    }
}

/// Indices of `want` in `notes`, each note matched once (identical notes are interchangeable).
pub fn resolve(notes: &[Note], want: &[Note]) -> Vec<usize> {
    let mut used = vec![false; notes.len()];
    let mut out = Vec::new();
    for w in want {
        if let Some(i) = (0..notes.len()).find(|i| !used[*i] && notes[*i] == *w) {
            used[i] = true;
            out.push(i);
        }
    }
    out
}

/// Grid mapping for one frame.
#[derive(Clone, Copy)]
struct Map {
    gx0: f32,
    gy0: f32,
    ppt: f64,
    scroll: f64,
    top: f32,
    kh: f32,
    c0: i64,
    /// The snap step: `None` when snapping is off.
    grid: Option<i64>,
    /// The grid step in force whether or not it snaps: the lines, new-note and nudge length.
    step: i64,
    rows: scale::Rows,
    scale: transit_midi::scale::ScaleMode,
}

impl Map {
    fn x(&self, t: i64) -> f32 {
        self.gx0 + ((t as f64 - self.scroll) * self.ppt) as f32
    }
    fn tick(&self, x: f32) -> f64 {
        self.scroll + f64::from(x - self.gx0) / self.ppt
    }
    fn y(&self, p: u8) -> f32 {
        self.gy0 + (self.rows.rank(self.top) - self.rows.rank(f32::from(p))) * self.kh
    }
    fn pitch(&self, y: f32) -> i32 {
        self.rows.pitch((self.rows.rank(self.top) - (y - self.gy0) / self.kh).ceil()) as i32
    }
    /// The grid line at or before `rel` (ticks from the clip start; the grid is the session's).
    fn floor(&self, rel: f64) -> i64 {
        match self.grid {
            Some(g) => ((self.c0 as f64 + rel) / g as f64).floor() as i64 * g - self.c0,
            None => rel.floor() as i64,
        }
    }
    /// The nearest grid line.
    fn round(&self, rel: f64) -> i64 {
        match self.grid {
            Some(g) => ((self.c0 as f64 + rel) / g as f64).round() as i64 * g - self.c0,
            None => rel.round() as i64,
        }
    }
    fn note_rect(&self, n: &Note) -> Rect {
        let x0 = self.x(n.start);
        let y = self.y(n.pitch);
        Rect::from_min_max(pos2(x0, y + 1.0), pos2(self.x(n.end()).max(x0 + 3.0), y + self.kh - 1.0))
    }
}

impl KeysState {
    /// Pitches of auditioned notes still held.
    #[cfg(test)]
    pub fn held_pitches(&self) -> Vec<u8> {
        self.held.iter().map(|h| h.1).collect()
    }
}

impl StudioApp {
    /// The piano roll is on screen.
    pub fn keys_shown(&self) -> bool {
        self.ui.keys.show && self.keys.clip.is_some() && !self.ui.mixer_full
    }

    /// Open `clip` in the piano roll (MIDI clips only). View state only.
    pub fn open_keys(&mut self, clip: ClipId) -> bool {
        if clip_view(self.engine.session(), clip).is_none() {
            return false;
        }
        if self.keys.clip != Some(clip) {
            self.keys.quantize_open = false;
            self.keys.selection.clear();
            self.keys.fit = true;
            self.keys.step = StepState::default();
        }
        self.keys.cc.cancel();
        self.keys.length_edit = None;
        self.keys.clip = Some(clip);
        self.keys.gesture = None;
        self.keys.active = true;
        self.ui.keys.show = true;
        true
    }

    /// Ctrl+Alt+K: show or hide the piano roll; with no clip open, open the selected MIDI clip.
    pub fn toggle_keys(&mut self) {
        if self.keys_shown() {
            self.close_keys();
            return;
        }
        let s = self.engine.session();
        let open = self.keys.clip.filter(|c| clip_view(s, *c).is_some());
        let pick = open.or_else(|| s.edit.selected_clips.iter().copied().find(|c| clip_view(s, *c).is_some()));
        match pick {
            Some(c) => {
                self.open_keys(c);
            }
            None => self.status = "Piano roll: double-click a MIDI clip to edit it.".into(),
        }
    }

    /// Hide the piano roll; the panes shown before it come back.
    pub fn close_keys(&mut self) {
        self.keys.cc.cancel();
        self.keys.length_edit = None;
        self.live_clear();
        self.ui.keys.show = false;
        self.keys.gesture = None;
        self.keys.active = false;
        self.release_auditions(f64::INFINITY, false);
    }

    fn keys_track(&self) -> Option<TrackId> {
        self.engine.session().find_clip(self.keys.clip?).map(|(t, _)| t)
    }

    /// Start a gesture's undo step: a key of its own, so it never merges with the one before.
    fn keys_merge_key(&mut self) -> String {
        self.engine.end_merge();
        self.keys.serial += 1;
        format!("keys:{}", self.keys.serial)
    }

    /// The grid step in ticks at the piano roll's zoom ([`keys_grid_step`], whether or not it snaps).
    pub fn keys_step(&self) -> i64 {
        keys_grid_step(self.engine.session(), &self.ui.keys)
    }

    fn keys_exec(&mut self, id: &str, params: Value, key: &str) -> Result<Value, String> {
        self.engine.execute_merged(id, &params, key).map_err(|e| {
            self.status = e.to_string();
            e.to_string()
        })
    }

    fn keys_notes(&self) -> Vec<Note> {
        self.keys.clip.and_then(|c| clip_view(self.engine.session(), c)).map(|v| v.notes).unwrap_or_default()
    }

    /// Play `pitch` at the toolbar velocity (when the audition switch is on).
    pub fn audition(&mut self, pitch: u8, now: f64) {
        if !self.ui.keys.audition {
            return;
        }
        let Some(track) = self.keys_track() else { return };
        let velocity = self.ui.keys.velocity.clamp(1, 127);
        self.keys.auditions += 1;
        self.keys.last_audition = Some((pitch, velocity));
        if let Some(p) = &self.player
            && self.player_requests.result(&mut self.status, p.note(track, pitch, velocity)).is_none()
        {
            return;
        }
        self.keys.held.push((track, pitch, now + 0.15));
    }

    /// Release auditioned notes that have sounded long enough, once the pointer is up.
    fn release_auditions(&mut self, now: f64, pointer_down: bool) {
        let mut keep = Vec::new();
        for (t, pitch, at) in std::mem::take(&mut self.keys.held) {
            if pointer_down || now < at {
                keep.push((t, pitch, at));
            } else if let Some(p) = &self.player
                && self.player_requests.result(&mut self.status, p.note(t, pitch, 0)).is_none()
            {
                keep.push((t, pitch, at));
            }
        }
        self.keys.held = keep;
    }

    /// Add notes to the open clip as one undo step; returns them as stored (the new selection).
    pub fn keys_add(&mut self, notes: &[Note]) -> Result<Vec<Note>, String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let key = self.keys_merge_key();
        let mut out = Vec::new();
        for n in notes {
            let stored = Note { release_velocity: 64, ..*n };
            self.keys_exec(
                "midi.note_add",
                json!({"clip": clip.0, "pitch": n.pitch, "start_ticks": n.start, "length_ticks": n.length, "velocity": n.velocity, "channel": n.channel}),
                &key,
            )?;
            out.push(stored);
        }
        Ok(out)
    }

    /// Change notes (old value, new value) as one undo step, each found again before its edit.
    pub fn keys_edit(&mut self, pairs: &[(Note, Note)]) -> Result<Vec<Note>, String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let key = self.keys_merge_key();
        let mut out = Vec::new();
        for (old, new) in pairs {
            if old == new {
                out.push(*new);
                continue;
            }
            let Some(i) = resolve(&self.keys_notes(), &[*old]).first().copied() else { continue };
            self.keys_exec(
                "midi.note_edit",
                json!({"clip": clip.0, "index": i, "pitch": new.pitch, "start_ticks": new.start, "length_ticks": new.length, "velocity": new.velocity}),
                &key,
            )?;
            out.push(*new);
        }
        Ok(out)
    }

    /// Move the selection by `ticks` and `semitones` (clamped so no note leaves the range).
    pub fn keys_move(&mut self, ticks: i64, semitones: i64) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let sel = self.keys.selection.clone();
        if self.keys_scale().enabled {
            let degrees = if matches!(semitones, -12 | 12) { semitones.signum() * 7 } else { semitones };
            let pairs = self.scale_move(&sel, ticks, degrees, None);
            self.keys.selection = self.scale_apply_move(&pairs)?;
            return Ok(());
        }
        let (dt, ds) = clamp_move(&sel, ticks, semitones);
        let idx = resolve(&self.keys_notes(), &sel);
        if idx.is_empty() || (dt == 0 && ds == 0) {
            return Ok(());
        }
        let key = self.keys_merge_key();
        self.keys_exec("midi.notes_move", json!({"clip": clip.0, "indices": idx, "ticks": dt, "semitones": ds}), &key)?;
        self.keys.selection = sel.iter().map(|n| shifted(n, dt, ds)).collect();
        Ok(())
    }

    /// Delete the selected notes (one undo step).
    pub fn keys_delete(&mut self) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let idx = resolve(&self.keys_notes(), &self.keys.selection);
        self.keys.selection.clear();
        if idx.is_empty() {
            return Ok(());
        }
        let key = self.keys_merge_key();
        self.keys_exec("midi.note_delete", json!({"clip": clip.0, "indices": idx}), &key).map(|_| ())
    }

    /// Ctrl+D: copies of the selection right after it (its span rounded up to the grid).
    pub fn keys_duplicate(&mut self) -> Result<(), String> {
        let sel = self.keys.selection.clone();
        let (Some(a), Some(b)) = (sel.iter().map(|n| n.start).min(), sel.iter().map(Note::end).max()) else { return Ok(()) };
        let g = self.keys_step();
        let by = ((b - a + g - 1) / g).max(1) * g;
        let copies: Vec<Note> = sel.iter().map(|n| shifted(n, by, 0)).collect();
        self.keys.selection = self.keys_add(&copies)?;
        Ok(())
    }

    /// Quick apply with saved strength/swing to the current piano-roll grid.
    pub fn keys_quantize(&mut self) -> Result<(), String> {
        self.keys_apply_quantize(self.keys_step())
    }

    /// Ctrl+Up/Down: transpose the selection (all notes when none or all are selected:
    /// `event.transpose`; otherwise `midi.notes_transpose`).
    pub fn keys_transpose(&mut self, semitones: i64) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let notes = self.keys_notes();
        let sel = self.keys.selection.clone();
        let whole = sel.is_empty() || resolve(&notes, &sel).len() == notes.len();
        if self.keys_scale().enabled {
            let degrees = if matches!(semitones, -12 | 12) { semitones.signum() * 7 } else { semitones };
            let pairs = self.scale_move(if whole { &notes } else { &sel }, 0, degrees, None);
            let edited = self.scale_apply_move(&pairs)?;
            if !sel.is_empty() {
                self.keys.selection = edited;
            }
            return Ok(());
        }
        if !whole {
            return self.keys_pitch_edit("midi.notes_transpose", json!({"semitones": semitones}));
        }
        let key = self.keys_merge_key();
        self.ensure_clip_selected(clip);
        self.keys_exec("event.transpose", json!({"clips": [clip.0], "semitones": semitones}), &key)?;
        if !sel.is_empty() {
            self.keys.selection = self.keys_notes();
        }
        Ok(())
    }

    /// `event.*` commands need a selection to be enabled; the clip being edited is it.
    fn ensure_clip_selected(&mut self, clip: ClipId) {
        let s = self.engine.session();
        if s.edit.selected_clips.is_empty() && s.edit.selected_tracks.is_empty() {
            let _ = self.engine.execute("edit.select", &json!({"clips": [clip.0]}));
        }
    }
}

/// Shift alone is the scale-mode chromatic override; with Alt it means free move instead.
fn chromatic(m: egui::Modifiers) -> bool {
    m.shift && !m.alt
}

fn shifted(n: &Note, dt: i64, ds: i64) -> Note {
    Note { start: (n.start + dt).max(0), pitch: (i64::from(n.pitch) + ds).clamp(0, 127) as u8, ..*n }
}

/// Clamp a move so every note stays at or after the clip start and inside 0..=127.
fn clamp_move(sel: &[Note], dt: i64, ds: i64) -> (i64, i64) {
    let min_start = sel.iter().map(|n| n.start).min().unwrap_or(0);
    let lo = sel.iter().map(|n| i64::from(n.pitch)).min().unwrap_or(0);
    let hi = sel.iter().map(|n| i64::from(n.pitch)).max().unwrap_or(127);
    (dt.max(-min_start), ds.clamp(-lo, 127 - hi))
}

/// The piano roll's keys. Runs before the arrangement's shortcuts, so Delete, Ctrl+A/D, the
/// arrows and Escape go to the notes while the pane has the keyboard.
pub fn shortcuts(app: &mut StudioApp, ctx: &egui::Context) {
    use crate::shortcuts::{Action as A, pressed};
    if !app.step_active() || !app.keys.active || ctx.egui_wants_keyboard_input() || app.dialog.is_some() {
        app.step_keyboard_cancel();
    }
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() {
        return;
    }
    if pressed(ctx, A::TogglePianoRoll) {
        app.toggle_keys();
    }
    if !app.keys_shown() || !app.keys.active {
        return;
    }
    if pressed(ctx, A::NotesClose) {
        if app.keys.gesture.take().is_none() {
            app.close_keys();
        }
        return;
    }
    // The grid (Ableton's Ctrl+1 to 4, as in the arrangement; the piano roll has its own view).
    let (finer, wider) = (pressed(ctx, A::NotesGridNarrow), pressed(ctx, A::NotesGridWiden));
    if finer || wider {
        let (ppt, bar) = (keys_ppt(&app.ui.keys), grid::bar_ticks(app.engine.session()));
        grid::step(&mut app.ui.keys.grid, ppt, bar, finer);
    }
    if pressed(ctx, A::NotesGridTriplet) {
        app.ui.keys.grid.triplet = !app.ui.keys.grid.triplet;
    }
    if pressed(ctx, A::NotesGridSnap) {
        app.ui.keys.grid.snap_enabled = !app.ui.keys.grid.snap_enabled;
    }
    if pressed(ctx, A::StepToggle) {
        app.step_toggle();
    }
    if app.step_active() {
        for down in crate::shortcuts::step_control_edges(ctx, A::StepTieRest, app.ui.keys.step_tie_rest_space) {
            app.step_keyboard_control(down);
        }
        if pressed(ctx, A::StepBack) {
            app.step_back();
            return;
        }
        if pressed(ctx, A::StepRest) {
            app.step_rest();
        }
        if pressed(ctx, A::StepCursorLeft) {
            app.step_move(-1);
        }
        if pressed(ctx, A::StepCursorRight) {
            app.step_move(1);
        }
        if pressed(ctx, A::StepHome) {
            app.step_home();
        }
        if pressed(ctx, A::StepEnd) {
            app.step_end();
        }
        for (action, ticks) in [
            (A::StepWhole, PPQ * 4),
            (A::StepHalf, PPQ * 2),
            (A::StepQuarter, PPQ),
            (A::StepEighth, PPQ / 2),
            (A::StepSixteenth, PPQ / 4),
            (A::StepThirtySecond, PPQ / 8),
        ] {
            if pressed(ctx, action) {
                app.step_set_len(ticks);
            }
        }
    }
    if pressed(ctx, A::NotesSelectAll) {
        app.keys.selection = app.keys_notes();
    }
    if pressed(ctx, A::NotesDuplicate) {
        let _ = app.keys_duplicate();
    }
    if pressed(ctx, A::NotesQuantize) {
        let _ = app.keys_quantize();
    }
    if pressed(ctx, A::NotesTransposeUp) {
        let _ = app.keys_transpose(12);
    }
    if pressed(ctx, A::NotesTransposeDown) {
        let _ = app.keys_transpose(-12);
    }
    let step = app.keys_step();
    let nudges = [
        (A::NotesOctaveUp, 0, 12),
        (A::NotesOctaveDown, 0, -12),
        (A::NotesUp, 0, 1),
        (A::NotesDown, 0, -1),
        (A::NotesEarlier, -step, 0),
        (A::NotesLater, step, 0),
    ];
    for (action, dt, ds) in nudges {
        if pressed(ctx, action) {
            let _ = app.keys_move(dt, ds);
        }
    }
    if pressed(ctx, A::NotesDelete) {
        let _ = app.keys_delete();
    }
}

/// Where the notes of the gesture in progress would land: (old, new) pairs, and whether they are copies.
fn preview(app: &StudioApp, ui: &Ui, map: &Map, vel: Rect, notes: &[Note]) -> Option<(Vec<(Note, Note)>, bool)> {
    let Some(KeysGesture::Notes { kind, anchor, press, copy, .. }) = app.keys.gesture.clone() else { return None };
    let pos = ui.input(|i| i.pointer.latest_pos())?;
    let sel: Vec<Note> = resolve(notes, &app.keys.selection).into_iter().map(|i| notes[i]).collect();
    let raw = f64::from(pos.x - press.x) / map.ppt;
    let mods = ui.input(|i| i.modifiers);
    // Edges: Alt alone frees the snap. Body moves: Alt+Shift (Alt alone is velocity).
    let free = if kind == NoteDrag::Move { mods.alt && mods.shift } else { mods.alt };
    let round = |tick: f64| if free { tick.round() as i64 } else { map.round(tick) };
    let pairs = match kind {
        NoteDrag::Move => {
            let dt = round(anchor.start as f64 + raw) - anchor.start;
            let ds = ((press.y - pos.y) / map.kh).round() as i64;
            let (dt, ds) = clamp_move(&sel, dt, ds);
            if map.scale.enabled && !chromatic(mods) {
                app.scale_move(&sel, dt, ds, Some(anchor.pitch))
            } else {
                sel.iter().map(|n| (*n, shifted(n, dt, ds))).collect()
            }
        }
        NoteDrag::Resize => {
            let min = if free { 1 } else { map.grid.unwrap_or(1).max(1) };
            let dl = round(anchor.end() as f64 + raw).saturating_sub(anchor.end());
            sel.iter().map(|n| (*n, Note { length: n.length.saturating_add(dl).max(min), ..*n })).collect()
        }
        NoteDrag::ResizeStart => {
            let min = if free { 1 } else { map.grid.unwrap_or(1).max(1) };
            let dt = round(anchor.start as f64 + raw).saturating_sub(anchor.start);
            sel.iter()
                .map(|n| {
                    let start = n.start.saturating_add(dt).clamp(0, n.end().saturating_sub(min).max(0));
                    (*n, Note { start, length: n.end().saturating_sub(start).max(min), ..*n })
                })
                .collect()
        }
        NoteDrag::Velocity | NoteDrag::Lane => {
            let target = if kind == NoteDrag::Lane {
                ((vel.max.y - pos.y) / (vel.height() - 6.0) * 127.0).round() as i64
            } else {
                i64::from(anchor.velocity) + ((press.y - pos.y) / 1.5).round() as i64
            };
            let dv = target.clamp(1, 127) - i64::from(anchor.velocity);
            sel.iter().map(|n| (*n, Note { velocity: (i64::from(n.velocity) + dv).clamp(1, 127) as u8, ..*n })).collect()
        }
    };
    Some((pairs, copy && kind == NoteDrag::Move))
}

/// The note under `pos`, with separate edge zones and a body even for short notes.
fn hit(layout: &KeysLayout, pos: Pos2) -> Option<(usize, NoteDrag)> {
    layout.notes.iter().rev().find(|n| pos.x >= n.rect[0] && pos.x <= n.rect[2] && pos.y >= n.rect[1] && pos.y <= n.rect[3]).map(|n| {
        let w = n.rect[2] - n.rect[0];
        let edge = EDGE.min(w / 3.0);
        let kind = if n.rect[2] - pos.x <= edge {
            NoteDrag::Resize
        } else if pos.x - n.rect[0] <= edge {
            NoteDrag::ResizeStart
        } else {
            NoteDrag::Move
        };
        (n.index, kind)
    })
}

#[allow(clippy::too_many_arguments)]
fn grid_input(app: &mut StudioApp, ui: &mut Ui, map: &Map, grid: Rect, vel: Rect, cv: &ClipView, now: f64, layout: &mut KeysLayout) {
    let resp = ui.interact(grid, ui.id().with("keys_grid"), Sense::click_and_drag());
    let mods = ui.input(|i| i.modifiers);
    if let Some(pos) = resp.hover_pos()
        && app.keys.gesture.is_none()
    {
        match hit(layout, pos) {
            Some((_, NoteDrag::Resize | NoteDrag::ResizeStart)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
            Some(_) if mods.alt && !mods.shift => ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical),
            Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
            None => {}
        }
    }
    let notes = &cv.notes;
    // Our own double-click: egui counts a quick second double-click elsewhere as a triple click.
    let click = resp.clicked().then(|| resp.interact_pointer_pos()).flatten();
    let double = click.is_some_and(|pos| {
        let d = app.keys.last_click.is_some_and(|(t, p)| now - t < 0.35 && p.distance(pos) < 6.0);
        app.keys.last_click = if d { None } else { Some((now, pos)) };
        d
    });
    if double && let Some(pos) = click {
        match hit(layout, pos) {
            // Double-click a note: delete it (Ableton).
            Some((i, _)) => {
                app.keys.selection = vec![notes[i]];
                let _ = app.keys_delete();
            }
            // Double-click empty grid: a note in that cell, one grid long, at the toolbar velocity.
            None => {
                let pitch = map.pitch(pos.y);
                let start = map.floor(map.tick(pos.x));
                if start >= cv.len {
                    app.status = "Outside the clip: drag the clip end to extend".into();
                } else if (0..=127).contains(&pitch) && start >= 0 {
                    let n = Note {
                        pitch: app.scale_entry(pitch as u8, mods.shift),
                        velocity: app.ui.keys.velocity.clamp(1, 127),
                        release_velocity: 64,
                        channel: 0,
                        start,
                        length: map.step,
                    };
                    if let Ok(sel) = app.keys_add(&[n]) {
                        app.keys.selection = sel;
                        app.audition(n.pitch, now);
                    }
                }
            }
        }
    } else if let Some(pos) = click {
        match hit(layout, pos) {
            Some((i, _)) => {
                let n = notes[i];
                if mods.shift {
                    if let Some(k) = app.keys.selection.iter().position(|x| *x == n) {
                        app.keys.selection.remove(k);
                    } else {
                        app.keys.selection.push(n);
                    }
                } else {
                    app.keys.selection = vec![n];
                }
                app.audition(n.pitch, now);
            }
            None => {
                if !mods.shift {
                    app.keys.selection.clear();
                }
                // The cursor (where Play starts), as in the arrangement.
                let s = app.engine.session();
                let at = s.tempo.tick_to_samples(cv.c0 + map.tick(pos.x).round().max(-(cv.c0 as f64)) as i64, s.sample_rate);
                app.set_cursor(at, None);
            }
        }
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        match hit(layout, press) {
            Some((i, edge)) => {
                let n = notes[i];
                if !app.keys.selection.contains(&n) {
                    if mods.shift {
                        app.keys.selection.push(n);
                    } else {
                        app.keys.selection = vec![n];
                    }
                }
                let kind = if mods.alt && !mods.shift && edge == NoteDrag::Move { NoteDrag::Velocity } else { edge };
                if kind == NoteDrag::Move {
                    app.audition(n.pitch, now);
                }
                app.keys.gesture = Some(KeysGesture::Notes { kind, anchor: n, press, copy: mods.command, sounding: n.pitch });
            }
            None => app.keys.gesture = Some(KeysGesture::Band { press, additive: mods.shift }),
        }
    }
    if resp.dragged() {
        drag_update(app, ui, map, now);
    }
    if resp.drag_stopped() {
        finish(app, ui, map, vel, notes, layout);
    }
    resp.on_hover_text("Drag notes to move (Ctrl: copy), either edge to resize. Alt on an edge: resize without snap. Alt+Shift on the body: move without snap. Shift on the body: off-scale pitch in scale mode. Alt on the body: velocity.");
}

/// While dragging notes: Ctrl makes a copy; a pitch change auditions the new pitch.
fn drag_update(app: &mut StudioApp, ui: &Ui, map: &Map, now: f64) {
    let mods = ui.input(|i| i.modifiers);
    let pos = ui.input(|i| i.pointer.latest_pos());
    let mut play = None;
    let scale = app.keys_scale();
    let notes = app.keys_notes();
    let app_sel: Vec<Note> = resolve(&notes, &app.keys.selection).into_iter().map(|i| notes[i]).collect();
    if let Some(KeysGesture::Notes { kind, anchor, press, copy, sounding }) = &mut app.keys.gesture
        && *kind == NoteDrag::Move
    {
        *copy = mods.command;
        ui.ctx().set_cursor_icon(if *copy { CursorIcon::Copy } else { CursorIcon::Grabbing });
        if let Some(pos) = pos {
            let ds = ((press.y - pos.y) / map.kh).round() as i64;
            let pitch = if scale.enabled && !chromatic(mods) && ds != 0 {
                // Same group rule as the applied move, so the audition matches the result.
                let sel: Vec<u8> = app_sel.iter().map(|n| n.pitch).collect();
                let at = sel.iter().position(|&p| p == anchor.pitch).unwrap_or(0);
                scale.key.transpose_group(&sel, anchor.pitch, ds).get(at).copied().unwrap_or(anchor.pitch)
            } else {
                (i64::from(anchor.pitch) + ds).clamp(0, 127) as u8
            };
            if pitch != *sounding {
                play = Some((*sounding, pitch));
                *sounding = pitch;
            }
        }
    }
    if let Some((old, new)) = play {
        if let (Some(p), Some(t)) = (&app.player, app.keys_track()) {
            app.player_requests.result(&mut app.status, p.note(t, old, 0));
        }
        app.keys.held.retain(|h| h.1 != old);
        app.audition(new, now);
    }
}

/// The end of a drag on notes or a rubber band: one command (or one merged step).
fn finish(app: &mut StudioApp, ui: &Ui, map: &Map, vel: Rect, notes: &[Note], layout: &KeysLayout) {
    let preview = preview(app, ui, map, vel, notes);
    let Some(g) = app.keys.gesture.take() else { return };
    match g {
        KeysGesture::Notes { kind, .. } => {
            let Some((pairs, copy)) = preview else { return };
            match kind {
                NoteDrag::Move => {
                    if pairs.iter().all(|(a, b)| a == b) {
                        return;
                    }
                    if copy {
                        let new: Vec<Note> = pairs.iter().map(|(_, b)| *b).collect();
                        if let Ok(sel) = app.keys_add(&new) {
                            app.keys.selection = sel;
                        }
                    } else if let Ok(sel) = app.scale_apply_move(&pairs) {
                        app.keys.selection = sel;
                    }
                }
                NoteDrag::Resize | NoteDrag::ResizeStart | NoteDrag::Velocity | NoteDrag::Lane => {
                    if let Ok(sel) = app.keys_edit(&pairs) {
                        app.keys.selection = sel;
                    }
                }
            }
        }
        KeysGesture::Band { press, additive } => {
            let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else { return };
            let band = Rect::from_two_pos(press, pos);
            let inside: Vec<Note> = layout
                .notes
                .iter()
                .filter(|n| band.intersects(Rect::from_min_max(pos2(n.rect[0], n.rect[1]), pos2(n.rect[2], n.rect[3]))))
                .map(|n| notes[n.index])
                .collect();
            if !additive {
                app.keys.selection.clear();
            }
            for n in inside {
                if !app.keys.selection.contains(&n) {
                    app.keys.selection.push(n);
                }
            }
        }
        _ => {}
    }
}

/// The velocity lane: drag a note's stem (or click at a height) to set its velocity.
fn lane_input(app: &mut StudioApp, ui: &mut Ui, map: &Map, vel: Rect, cv: &ClipView, layout: &KeysLayout) {
    let resp = ui.interact(vel, ui.id().with("keys_vel"), Sense::click_and_drag());
    let notes = &cv.notes;
    // The note whose stem is nearest the press (within 6 points).
    let near = |x: f32| -> Option<usize> {
        layout
            .notes
            .iter()
            .map(|n| (n.index, (n.vel_rect[0] + 1.5 - x).abs()))
            .filter(|(_, d)| *d <= 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let begin = |app: &mut StudioApp, press: Pos2| -> bool {
        let Some(i) = near(press.x) else { return false };
        let n = notes[i];
        if !app.keys.selection.contains(&n) {
            app.keys.selection = vec![n];
        }
        app.keys.gesture = Some(KeysGesture::Notes { kind: NoteDrag::Lane, anchor: n, press, copy: false, sounding: n.pitch });
        true
    };
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        begin(app, press);
    }
    if resp.drag_stopped() && matches!(app.keys.gesture, Some(KeysGesture::Notes { kind: NoteDrag::Lane, .. })) {
        finish(app, ui, map, vel, notes, layout);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && begin(app, pos)
    {
        finish(app, ui, map, vel, notes, layout);
    }
}

/// The keyboard column: a press plays the key; dragging sideways zooms the rows, up and down scrolls.
fn keyboard_input(app: &mut StudioApp, ui: &mut Ui, map: &Map, kb: Rect, now: f64) {
    let resp = ui.interact(kb, ui.id().with("keys_keyboard"), Sense::click_and_drag());
    if resp.contains_pointer()
        && ui.input(|i| i.pointer.primary_pressed())
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
    {
        let pitch = map.pitch(pos.y);
        if (0..=127).contains(&pitch) {
            // Played even with the audition switch off: the keyboard is an instrument.
            let on = app.ui.keys.audition;
            app.ui.keys.audition = true;
            app.audition(pitch as u8, now);
            app.ui.keys.audition = on;
        }
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        let anchor = map.rows.rank(map.top) - (press.y - map.gy0) / map.kh;
        app.keys.gesture = Some(KeysGesture::Keyboard { key_h: app.ui.keys.key_h, anchor, press });
    }
    if resp.dragged()
        && let Some(KeysGesture::Keyboard { key_h, anchor, press }) = app.keys.gesture.clone()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        let kh = (key_h * ((pos.x - press.x) * 0.01).exp()).clamp(6.0, 40.0);
        app.ui.keys.key_h = kh;
        // The pitch grabbed stays under the pointer.
        app.keys.top = map.rows.pitch(anchor + (pos.y - map.gy0) / kh);
    }
    if resp.drag_stopped() {
        app.keys.gesture = None;
    }
}

/// The ruler: a click sets the cursor; a drag up or down zooms time around the pointer, sideways scrolls.
fn ruler_input(app: &mut StudioApp, ui: &mut Ui, map: &Map, ruler: Rect, s: &Session) {
    let resp = ui.interact(ruler, ui.id().with("keys_ruler"), Sense::click_and_drag());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let abs = (map.c0 as f64 + map.tick(pos.x)).max(0.0) as i64;
        app.click_ruler(s.tempo.tick_to_samples(abs, s.sample_rate));
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        app.keys.gesture = Some(KeysGesture::Ruler { ppq: app.ui.keys.px_per_quarter, anchor: map.tick(press.x), press });
    }
    if resp.dragged()
        && let Some(KeysGesture::Ruler { ppq, anchor, press }) = app.keys.gesture.clone()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        let z = (ppq * ((pos.y - press.y) * 0.01).exp()).clamp(8.0, 800.0);
        app.ui.keys.px_per_quarter = z;
        app.keys.scroll = anchor - f64::from(pos.x - map.gx0) / (f64::from(z) / PPQ as f64);
    }
    if resp.drag_stopped() {
        app.keys.gesture = None;
    }
}

/// Slice 6: the clip's end as a handle in the ruler. Dragging it moves the end, snapped to the
/// grid (one grid step at least); on release the clip is trimmed or extended there
/// (`edit.trim_clip`, one undo step), so notes placed past the old end now sound.
#[allow(clippy::too_many_arguments)]
fn clip_end_handle(app: &mut StudioApp, ui: &mut Ui, map: &Map, ruler: Rect, grid: Rect, cv: &ClipView, clip: ClipId, layout: &mut KeysLayout) {
    // A looped clip's handle sits at its arrangement end: dragging it changes the length, not the cycle.
    let x = map.x(cv.full);
    let h = Rect::from_min_max(pos2(x - 5.0, ruler.min.y), pos2(x + 5.0, ruler.max.y));
    layout.end_handle = r4(h);
    if h.max.x < ruler.min.x || h.min.x > ruler.max.x {
        return;
    }
    let resp = ui.interact(h.intersect(ruler), ui.id().with("keys_clip_end"), Sense::click_and_drag());
    let dragging = matches!(app.keys.gesture, Some(KeysGesture::ClipEnd));
    let step = map.step.max(1);
    let target = ui.input(|i| i.pointer.latest_pos()).map(|p| map.round(map.tick(p.x)).max(step));
    let p = ui.painter().with_clip_rect(Rect::from_min_max(ruler.min, grid.max));
    let ink = if resp.hovered() || dragging { TOKENS.accent } else { TOKENS.text_dim };
    p.add(egui::Shape::convex_polygon(
        vec![pos2(x - 5.0, ruler.min.y + 4.0), pos2(x + 1.0, ruler.min.y + 4.0), pos2(x + 1.0, ruler.max.y), pos2(x - 5.0, ruler.max.y)],
        ink,
        Stroke::NONE,
    ));
    if resp.hovered() || dragging {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started() {
        app.keys.gesture = Some(KeysGesture::ClipEnd);
    }
    if dragging && let Some(t) = target {
        let tx = map.x(t);
        p.line_segment([pos2(tx, ruler.min.y), pos2(tx, grid.max.y)], Stroke::new(2.0, TOKENS.accent));
    }
    let resp = resp.on_hover_text("Drag to change the clip's length");
    if resp.drag_stopped() && dragging {
        app.keys.gesture = None;
        if let Some(t) = target
            && t != cv.full
        {
            let s = app.engine.session();
            let end = s.tempo.tick_to_samples(cv.c0 + t, s.sample_rate);
            if app.trim_clip(clip, false, end).is_ok() {
                app.status = clip_length_text(cv.cycle.map_or(t, |(_, e)| e.min(t)), cv.cycle.map(|_| t));
            }
        }
    }
}

/// Wheel: scroll pitch (Shift: time); Ctrl+wheel zooms time, Ctrl+Shift+wheel or Alt+wheel zooms keys.
fn wheel(app: &mut StudioApp, ui: &Ui, map: &Map, body: Rect) {
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else { return };
    if !body.contains(pos) {
        return;
    }
    // The wheel event's own modifiers when there is one (synthetic input sends no key changes).
    let wheel_mods = |i: &egui::InputState| {
        i.events
            .iter()
            .rev()
            .find_map(|e| if let egui::Event::MouseWheel { modifiers, .. } = e { Some(*modifiers) } else { None })
            .unwrap_or(i.modifiers)
    };
    let (delta, zoom, mods) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), wheel_mods(i)));
    let zoom_keys = |app: &mut StudioApp, f: f32| {
        let anchor = map.rows.rank(map.top) - (pos.y - map.gy0) / map.kh;
        let kh = (app.ui.keys.key_h * f).clamp(6.0, 40.0);
        app.ui.keys.key_h = kh;
        app.keys.top = map.rows.pitch(anchor + (pos.y - map.gy0) / kh);
    };
    if (zoom - 1.0).abs() > 1e-4 {
        if mods.shift {
            zoom_keys(app, zoom);
        } else {
            let anchor = map.tick(pos.x);
            let z = (app.ui.keys.px_per_quarter * zoom).clamp(8.0, 800.0);
            app.ui.keys.px_per_quarter = z;
            app.keys.scroll = anchor - f64::from(pos.x - map.gx0) / (f64::from(z) / PPQ as f64);
        }
        return;
    }
    if mods.alt || (mods.command && mods.shift) {
        let d = if delta.y.abs() > delta.x.abs() { delta.y } else { delta.x };
        if d.abs() > 0.1 {
            zoom_keys(app, (d * 0.005).exp());
        }
        return;
    }
    // Shift+vertical arrives as a horizontal delta already (see `arrange::wheel_pan`).
    if delta.x.abs() > 0.1 {
        app.keys.scroll -= crate::arrange::wheel_pan(ui, delta.x) * f64::from(body.max.x - map.gx0) / map.ppt;
    }
    if delta.y.abs() > 0.1 {
        app.keys.top = map.rows.pitch(map.rows.rank(app.keys.top) + delta.y / map.kh);
    }
}

/// The arrangement's empty-lane hint: instrument and MIDI tracks with no clip say how to make one.
pub fn lane_hints(app: &mut StudioApp, ui: &Ui, tl: Rect, tracks: &[crate::arrange::TrackRow]) {
    let s = app.engine.session_arc();
    let mut ids = Vec::new();
    for row in tracks {
        let Some(t) = s.track(TrackId(row.id)) else { continue };
        if !t.kind.is_midi() || !t.clips().is_empty() {
            continue;
        }
        ids.push(row.id);
        let lane = Rect::from_min_max(pos2(row.lane[0], row.lane[1]), pos2(row.lane[2], row.lane[3])).intersect(tl);
        if lane.height() > 12.0 {
            ui.painter().with_clip_rect(tl).text(
                pos2(lane.min.x + 12.0, lane.center().y),
                Align2::LEFT_CENTER,
                "Double-click a lane to create a MIDI clip",
                FontId::proportional(11.5),
                TOKENS.text_dim,
            );
        }
    }
    app.keys.layout.hint_tracks = ids;
}

#[cfg(test)]
mod ghost_tests {
    use super::*;

    #[test]
    fn a_degenerate_cycle_on_a_long_clip_walks_a_bounded_number_of_cycles() {
        let bar = PPQ * 4;
        let notes: Vec<Note> =
            (0..500).map(|i| Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: i % 4, length: 1 }).collect();
        let cv = ClipView { track: TrackId(1), c0: 0, len: 4, full: 600 * bar, cycle: Some((0, 4)), notes, ctrls: Vec::new(), name: String::new() };
        let (ghosts, walked) = cv.ghosts((0, 600 * bar));
        assert!(walked <= MAX_GHOST_CYCLES, "walked {walked} cycles");
        assert!(ghosts.len() <= MAX_GHOSTS);
        // Off screen (before the repeats or past the end): nothing is built.
        assert_eq!(cv.ghosts((0, 4)), (Vec::new(), 0));
        assert_eq!(cv.ghosts((600 * bar, 601 * bar)), (Vec::new(), 0));
    }
}

#[cfg(test)]
mod ghost_style_tests {
    use super::*;

    #[test]
    fn a_ghost_paints_fainter_and_outlined_next_to_the_faintest_real_note() {
        let (fill, line) = ghost_colors();
        let faintest_note = TOKENS.accent.gamma_multiply(NOTE_MIN_ALPHA);
        assert_ne!(fill, faintest_note);
        assert!(u32::from(fill.a()) * 2 < u32::from(faintest_note.a()), "the ghost fill is well under any real note's");
        assert!(line.a() > fill.a(), "outlined, so the repeat stays legible");
    }
}
