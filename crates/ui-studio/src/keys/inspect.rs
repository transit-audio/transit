use super::*;

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
    /// The clip the note is in, when several clips are open (#329).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<u64>,
    /// Focus Mode dims notes outside the foreground clip (#329 slice 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimmed: Option<bool>,
}

/// One clip open in the roll when several are (#329), as last drawn.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct OpenClip {
    pub clip: u64,
    pub track: u64,
    /// Its start in the roll's ticks (the foreground's start is 0) and the span the roll edits.
    pub offset_ticks: i64,
    pub ticks: i64,
    pub foreground: bool,
    pub color: [u8; 3],
    /// Its bar in the ruler: click to make it the foreground.
    pub bar: R4,
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
    /// The clip being edited: the foreground clip when several are open.
    pub clip: Option<u64>,
    pub track: Option<u64>,
    /// Every clip open, the foreground first, when there are several (#329).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<OpenClip>,
    /// Focus Mode, when several clips are open (#329 slice 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<bool>,
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
