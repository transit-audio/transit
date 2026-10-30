//! The mixer: one channel strip per track plus the master, as a pane under the arrangement
//! (Ableton's Ctrl+Alt+M) or filling the window (Cubase's F3). A strip has the track's name and
//! color, input and output dropdowns, a balance knob, Mute/Solo/Record/Monitor, a
//! fader and a meter. Flat, like the arrangement: fills, no borders or gradients, one accent.
//!
//! Everything a strip changes goes through engine commands. A fader or knob drag is one undo step
//! (`Engine::execute_merged` with a per-track key). Which strip is focused, the panes and the
//! scroll are view state in [`crate::UiState`] and [`MixerState`], never in the document.
//!
//! Model leaks handled here (plan 06, section 2):
//! - L10: stereo tracks pan dual-mono. The knob is a balance control that drives both panners
//!   through [`pans_for`] (two `mix.pan` calls under one merge key).
//! - L5: busses are named paths. Outputs name the FX/group channel (the aux track
//!   whose input is the bus), not the bus. "Add FX Channel" is four commands, one undo step.
//! - L12: the master is an optional track. Without one the main-out strip shows the main meter
//!   and offers to add a master fader.
//! - L4: sends are ten fixed slots. A strip lists the occupied ones as rows (target, level,
//!   remove) under its input, Logic style, and "+ Send" fills the first free slot.
//!
//! Routing menus use the same route-change checks as the engine (ADR 0015), including
//! sidechain keys. The engine validates the chosen route again on execution.

mod actions;
mod controls;
mod menus;
mod meters;
mod routing;
mod sends;
mod strip;

use controls::{Readout, control, draw_knob, fader_widget, readout};
pub use controls::{balance_of, db_to_k, k_to_db, pans_for};
use menus::{choice_menu, dropdown, small_button};
use meters::draw_meter;
pub use meters::{CLIP_CAP, peak_text};
use routing::{NOT_HERE, push_unused_entry, same_route, unresolved_output};
pub use routing::{
    RoutingChoice, bus_label, feeding_loops, input_choices, listening_loops, orphan_busses, output_choices, path_choices, reaches, route_label,
    send_targets, takes_sends,
};
pub use sends::SEND_ROW_H;
pub(crate) use sends::{add_send_row, send_row};
use sends::{send_rows_of, sends_block};
pub use strip::{panes, show_full};

use crate::StudioApp;
use crate::arrange::R4;
use crate::fine::{self, Target};
use crate::theme::{self, TOKENS};
use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use transit_model::{BusId, IoDirection, IoMap, IoPath, KeyFrom, Route, SendSlot, Session, Track, TrackId, TrackKind};

/// Strip width and gap (points).
pub const STRIP_W: f32 = 88.0;
const GAP: f32 = 2.0;
/// Space separating ordinary tracks (including groups) from aux buses and returns.
const FX_GAP: f32 = 12.0;
/// The fader's top, in dB.
pub const FADER_MAX_DB: f32 = 12.0;

/// A drag in progress on a fader, knob or send level. `raw` is where the pointer has taken the
/// control (fader position 0..1 for a fader or send, balance -1..1 for a knob), before snapping,
/// so the snap is a detent the drag leaves again rather than a place it sticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixGesture {
    pub target: Target,
    pub raw: f32,
    /// The fader's height in points (pointer travel to fader travel).
    pub height: f32,
}

/// The value a "+ Send" menu entry carries for "New FX Channel…".
pub const NEW_FX: &str = "\u{1}new-fx-channel";
/// The value of the "Remove unused buses (n)" entry that ends the send, output and input menus.
pub const REMOVE_UNUSED: &str = "\u{1}remove-unused-busses";

/// One send row as last drawn (a strip's or the device chain's).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SendRow {
    pub slot: usize,
    /// What the target reads as: the FX channel's name, or "Bus: name".
    pub target: String,
    pub level_db: f32,
    pub pre_fader: bool,
    pub rect: R4,
    /// Click: a menu to send somewhere else.
    pub target_rect: R4,
    /// The thin level bar along the row's bottom: drag changes the level (one undo step),
    /// double-click resets it to 0 dB.
    pub level_rect: R4,
    /// The level as text: wheel nudges it, double-click types a value, drag changes it.
    pub readout_rect: R4,
    /// What the readout shows.
    pub level_text: String,
    pub remove: R4,
    /// The pre/post toggle (device chain only; strips have it in the row's right-click menu).
    pub pre: R4,
    /// Slice 7: the send's mute toggle and whether it is muted.
    pub mute: R4,
    pub muted: bool,
    /// Slice 7: the send's pan (device chain only) and what it shows.
    pub pan: R4,
    pub pan_text: String,
}

/// One dropdown or menu entry as last drawn (for `ui.inspect`; scripts click them).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MenuItem {
    pub label: String,
    pub value: String,
    pub rect: R4,
    /// False while the entry is scrolled out of its menu's view (a click there would miss).
    pub visible: bool,
}

impl MenuItem {
    /// An entry drawn in `ui` at `rect`, visible when its center is inside the ui's clip.
    pub fn at(ui: &Ui, label: String, value: String, rect: Rect) -> Self {
        MenuItem { label, value, rect: r4(rect), visible: ui.clip_rect().contains(rect.center()) }
    }
}

/// One strip as last drawn, with the values it shows.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StripLayout {
    pub track: u64,
    pub name: String,
    pub kind: Option<TrackKind>,
    pub rect: R4,
    pub name_rect: R4,
    pub input: R4,
    pub output: R4,
    pub input_label: String,
    pub output_label: String,
    /// MIDI tracks: the output destination button (#61), what it says, and whether its device is
    /// missing.
    pub midi_output: R4,
    pub midi_output_label: String,
    pub midi_output_missing: bool,
    pub knob: R4,
    pub mute: R4,
    pub solo: R4,
    pub arm: R4,
    pub monitor: R4,
    pub fader: R4,
    /// The y of the line through the fader cap, and of the 0 dB tick, as painted last frame.
    /// Both are on physical pixel centers, so at 0 dB they are one y.
    pub cap_line_y: Option<f32>,
    pub zero_tick_y: Option<f32>,
    /// The physical pixel rows [top, bottom) each of the two covers; the cap line is as thick as the
    /// tick and covers exactly its rows at 0 dB.
    pub cap_line_span: Option<[i32; 2]>,
    pub zero_tick_span: Option<[i32; 2]>,
    pub meter: R4,
    /// The dB readout under the fader (double-click types a level) and its text.
    pub readout_rect: R4,
    pub volume_text: String,
    /// The held peak above the meter ("-3.2", "+1.4"; click resets, Ctrl-click resets all).
    pub peak_rect: R4,
    pub peak_text: String,
    /// The pan readout under the knob and its text ("C", "L20", "R35").
    pub pan_readout_rect: R4,
    pub pan_text: String,
    pub volume_db: f32,
    /// The knob's value, -1 (left) .. 1 (right), read back from the track's panners.
    pub balance: f32,
    /// The track's panners as the engine holds them.
    pub pan: Vec<f32>,
    pub level: [f32; 2],
    /// An FX (aux) channel: tagged "FX", its name muted.
    pub fx: bool,
    pub sends: Vec<SendRow>,
    /// "+ Send" (zero when the strip takes no sends or all ten are in use).
    pub add_send: R4,
    /// The occupied insert slots listed under the input (at most three rows).
    pub inserts: Vec<InsertRow>,
    /// The "+" row that adds an effect to this track (zero when all ten slots are in use).
    pub add_insert: R4,
    /// The strip is the focused track's (the device chain follows it).
    pub focused: bool,
    /// The group this track sits in (slice 2c).
    pub folder: Option<u64>,
}

/// One insert row of a strip.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct InsertRow {
    pub slot: usize,
    pub name: String,
    pub bypass: bool,
    pub rect: R4,
    /// What the row reads after the name while the plug-in is on its way ("Loading…",
    /// "Loading… (slow) 12 s", "Not responding"); empty once it is live.
    pub status: String,
    /// The Retry button of a "Not responding" row (zero otherwise, issue #82).
    pub retry: R4,
}

/// Last frame's mixer geometry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MixerLayout {
    pub visible: bool,
    pub area: R4,
    pub strips: Vec<StripLayout>,
    /// The master strip (`track` 0 when there is no master track yet).
    pub master: Option<StripLayout>,
    pub add_master: R4,
    pub add_fx: R4,
    /// Divider before the aux block, clipped to the scrolling lane; absent without both blocks.
    pub fx_divider: Option<R4>,
    /// The open dropdown's entries.
    pub menu: Vec<MenuItem>,
    /// The bars above a group's strip and its members' strips (slice 2c).
    pub groups: Vec<GroupLink>,
}

/// The thin accent bar that links a group's strip to its members' strips.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GroupLink {
    pub folder: u64,
    pub members: Vec<u64>,
    pub rect: R4,
}

/// One device card as last drawn.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DeviceCard {
    /// Insert slot, or `None` for the instrument.
    pub slot: Option<usize>,
    pub plugin: String,
    pub name: String,
    pub bypass: bool,
    pub rect: R4,
    pub bypass_button: R4,
    pub remove_button: R4,
    pub editor_button: R4,
    pub editor_enabled: bool,
    /// The plug-in's editor window is open or wanted (the button then reads "Close").
    pub editor_open: bool,
    /// The title area a press-and-drag on grabs to reorder the device (inserts only).
    pub drag_handle: R4,
    /// The instrument card's name (or "Choose…" when there is none): opens the instrument menu.
    pub pick_button: R4,
    /// The sidechain key dropdown of an insert that takes a key (zero when it has none).
    pub key_button: R4,
    /// What the key dropdown reads, e.g. "Key: Kick (post)" or "Key: None".
    pub key_label: String,
    /// The track is frozen: the card is greyed and its plug-in is off.
    pub frozen: bool,
    /// A hardware insert's pair as the card shows it ("HW: Out 3-4 → In 3-4"); empty otherwise.
    pub hardware: String,
    /// A hardware insert's state as the card shows it ("12.8 ms", "Unmeasured", "No input", ...).
    pub hardware_state: String,
    /// The state area of a hardware insert (a button to enable it again when it is disabled).
    pub hardware_button: R4,
    /// An External Instrument's return dropdown, gain and manual latency fields (ADR 0020).
    pub external_return: R4,
    pub external_gain: R4,
    pub external_latency: R4,
    /// The Out selector (port and channel), the result line under Latency and the status row.
    pub external_out: R4,
    pub external_result: R4,
    pub external_status: R4,
    /// An External Instrument's state line as the card shows it ("Manual: 19.5 ms", ...).
    pub external_state: String,
    /// The line under the name: the plug-in's kind, or while it is on its way "Loading…",
    /// "Loading… (slow) 12 s" or "Not responding" (issue #82).
    pub status: String,
    /// The Retry button beside "Not responding" (zero otherwise).
    pub retry_button: R4,
    /// The state line is amber: a measurement made at another rate or buffer ("measure again").
    pub external_stale: bool,
    /// The Measure button (Cancel while it runs) and Keep (a result with a high spread).
    pub external_measure: R4,
    pub external_keep: R4,
    /// What the Measure row says: "Measure", "Measuring… 3/8", or the last result or refusal.
    pub external_measure_text: String,
    /// Why Measure is off ("" when it can run).
    pub external_measure_blocked: String,
}

/// Last frame's device chain geometry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DevicesLayout {
    pub visible: bool,
    pub track: Option<u64>,
    pub area: R4,
    pub cards: Vec<DeviceCard>,
    pub add: R4,
    /// While a device is dragged: the x of the drop indicator (`None` when releasing now would change nothing).
    pub drop_x: Option<f32>,
    /// The open menu's entries: "+" or the instrument menu (`value` is the plug-in id), or a
    /// send menu (`value` is the bus name, or [`NEW_FX`]).
    pub menu: Vec<MenuItem>,
    /// The focused track's sends.
    pub sends: Vec<SendRow>,
    pub add_send: R4,
    /// A rack's own card (ADR 0034): its macros and member zones.
    pub rack: Option<crate::rack_panel::RackLayout>,
}

/// A plug-in the "+" menu offers.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub instrument: bool,
}

#[derive(Debug, Default)]
pub struct MixerState {
    pub gesture: Option<MixGesture>,
    /// Horizontal scroll of the strips (points).
    pub hscroll: f32,
    pub layout: MixerLayout,
    pub devices: DevicesLayout,
    /// The device being dragged in the chain (see `devices::chain`).
    pub(crate) device_drag: Option<crate::devices::DeviceDrag>,
    /// Built-in plug-ins (from `engine.plugins`), read once.
    pub(crate) builtin: Option<Vec<PluginEntry>>,
    /// Display names of hosted plug-ins by id, filled from the scan caches as cards need them.
    pub(crate) names: std::collections::HashMap<String, String>,
    /// The instrument menu's filter text (shown when the list is long).
    pub instrument_filter: String,
    /// The "+" effects menu's filter text.
    pub effect_filter: String,
    /// Where the instrument menu's filter box was last drawn (for `ui.inspect`).
    pub filter_rect: R4,
    /// Gestures started, to give each composite its own merge key.
    pub(crate) serial: u64,
    /// A readout being edited in place.
    pub edit: Option<fine::ReadoutEdit>,
    /// Where the editing field was last drawn (for `ui.inspect`).
    pub edit_rect: R4,
    /// The wheel gesture in progress (its nudges are one undo step).
    pub wheel: Option<fine::WheelMerge>,
    /// The control under the pointer as last drawn (Delete resets it).
    pub hover: Option<Target>,
    /// A control took the wheel this frame, so the strips don't scroll.
    wheel_taken: bool,
    /// The key dropdown's tap choice while an insert has no key yet: pre-fader when true.
    pub(crate) key_pre: bool,
    /// The track a "Hardware Insert…" picker is open for.
    pub(crate) hw_picker: Option<TrackId>,
    /// The picker as last drawn.
    pub hw_picker_layout: crate::hardware_ui::PickerLayout,
}

pub(crate) fn r4(r: Rect) -> R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

/// Shared by the arrangement and mixer so one fader gesture has one undo key.
pub(crate) fn fader_key(track: TrackId) -> String {
    format!("fader:{}", track.0)
}

/// The in-place text field of a readout being edited: Enter applies, Escape and losing focus
/// cancel.
fn edit_field(app: &mut StudioApp, ui: &mut Ui, draw: Rect, id: egui::Id, font: FontId) {
    let Some(mut e) = app.mixer.edit.take() else { return };
    let rect = Rect::from_center_size(draw.center(), vec2(draw.width().max(46.0), draw.height().max(16.0)));
    app.mixer.edit_rect = r4(rect);
    let (esc, enter) = ui.input(|i| {
        use crate::shortcuts::{Action, hit};
        (hit(i, Action::ReadoutCancel), hit(i, Action::ReadoutApply))
    });
    if e.fresh {
        e.fresh = false;
        // Focused with the whole text selected, so typing replaces it.
        let n = e.text.chars().count();
        let mut st = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
        st.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
    }
    let field = egui::TextEdit::singleline(&mut e.text)
        .id(id)
        .font(font)
        .margin(vec2(3.0, 1.0))
        .horizontal_align(egui::Align::Center)
        .desired_width(rect.width());
    let resp = ui.put(rect, field);
    if esc {
        return;
    }
    if resp.lost_focus() {
        if enter {
            let v = if e.target.is_pan() { fine::parse_pan(&e.text) } else { fine::parse_db(&e.text) };
            match v {
                Some(v) => {
                    let _ = app.set_value(e.target, v);
                }
                None => app.status = format!("\"{}\" is not a {}", e.text.trim(), if e.target.is_pan() { "pan position" } else { "level" }),
            }
        }
        return;
    }
    app.mixer.edit = Some(e);
}
