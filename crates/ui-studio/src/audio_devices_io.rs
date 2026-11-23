//! Audio › Audio Devices, I/O paths section (ADR 0015 slice a): the channels the open device
//! has, and this machine's named paths over them (name, channels, width) for each direction, with
//! Main's path. The table is the engine's (`Engine::io_map`), read again every frame; every edit
//! is one `setup.io_map` command (no undo entry, session untouched) that the window runs through
//! `StudioApp::run`, so its refusals reach the status line. The only state kept here is what the
//! user is in the middle of typing or editing.

use crate::icons::{self, Icon};
use crate::mixer::MenuItem;
use crate::theme::{SPACE, TOKENS};
use serde_json::{Value, json};
use transit_model::{IoDirection, IoMap, IoPath, MAX_PATH_WIDTH};

/// One edit of the path table: one `setup.io_map` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoEdit {
    Add {
        direction: IoDirection,
        name: String,
        channels: Vec<u16>,
    },
    /// The edits of one path name its direction: an input path may share an output path's name
    /// until one is renamed (ADR 0015, amendment for #431, rule 8).
    Rename {
        name: String,
        direction: IoDirection,
        new_name: String,
    },
    Remove {
        name: String,
        direction: IoDirection,
    },
    SetChannels {
        name: String,
        direction: IoDirection,
        channels: Vec<u16>,
    },
    SetMain {
        name: String,
    },
    SetLive {
        name: String,
        live: bool,
    },
    /// Add default paths: the default input paths the table lacks (rule 14), idempotent.
    AddDefaultInputs,
    /// Use the current ports: a virtual input's ports open now become the ones its paths read
    /// (rule 5).
    UseCurrentPorts,
}

fn direction_text(direction: IoDirection) -> &'static str {
    if direction == IoDirection::Input { "input" } else { "output" }
}

impl IoEdit {
    /// The `setup.io_map` parameters (0-based device channels).
    pub fn params(&self) -> Value {
        match self {
            IoEdit::Add { direction, name, channels } => {
                json!({"action": "add_path", "direction": direction_text(*direction), "name": name, "channels": channels})
            }
            IoEdit::Rename { name, direction, new_name } => {
                json!({"action": "rename_path", "name": name, "direction": direction_text(*direction), "new_name": new_name})
            }
            IoEdit::Remove { name, direction } => json!({"action": "remove_path", "name": name, "direction": direction_text(*direction)}),
            IoEdit::SetChannels { name, direction, channels } => {
                json!({"action": "set_channels", "name": name, "direction": direction_text(*direction), "channels": channels})
            }
            IoEdit::SetMain { name } => json!({"action": "set_main", "name": name}),
            IoEdit::SetLive { name, live } => json!({"action": "set_live", "name": name, "live": live}),
            IoEdit::AddDefaultInputs => json!({"action": "add_default_inputs"}),
            IoEdit::UseCurrentPorts => json!({"action": "use_current_ports"}),
        }
    }
}

/// Device channels as people count them (1-based): "1", "3-4", "1-6", and "1-2, 5" when the path
/// is not contiguous.
pub fn channel_label(channels: &[u16]) -> String {
    let mut runs: Vec<(u16, u16)> = Vec::new();
    for &c in channels {
        match runs.last_mut() {
            Some((_, last)) if c == *last + 1 => *last = c,
            _ => runs.push((c, c)),
        }
    }
    let part = |(a, b): (u16, u16)| if a == b { format!("{}", a + 1) } else { format!("{}-{}", a + 1, b + 1) };
    runs.into_iter().map(part).collect::<Vec<_>>().join(", ")
}

/// `channels` with `c` added (kept in order) or removed.
pub fn toggled(channels: &[u16], c: u16) -> Vec<u16> {
    let mut next: Vec<u16> = channels.to_vec();
    if let Some(i) = next.iter().position(|&x| x == c) {
        next.remove(i);
    } else {
        next.push(c);
        next.sort_unstable();
    }
    next
}

/// The first channel no path of `direction` uses (0 when all are taken).
fn first_free(io: &IoMap, direction: IoDirection) -> u16 {
    let opened = if direction == IoDirection::Input { io.inputs } else { io.outputs };
    (0..opened).find(|c| !io.paths_of(direction).any(|p| p.channels.contains(c))).unwrap_or(0)
}

/// A sentence for each pair of output paths that share a device channel.
pub fn overlap_lines(io: &IoMap) -> Vec<String> {
    io.overlaps().into_iter().map(|(a, b, c)| format!("{a} and {b} share channel {} and add together there.", c + 1)).collect()
}

/// The status line for a path table that needs a look: routes naming paths this machine lacks,
/// and output paths that share a channel. `None` when both are fine.
pub fn warning_text(unresolved: &[String], overlaps: &[(String, String, u16)]) -> Option<String> {
    let mut parts = Vec::new();
    match unresolved.len() {
        0 => {}
        1 => parts.push("1 route not on this device".to_string()),
        n => parts.push(format!("{n} routes not on this device")),
    }
    if !overlaps.is_empty() {
        let mut names: Vec<&str> = Vec::new();
        for (a, b, _) in overlaps {
            for n in [a.as_str(), b.as_str()] {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        parts.push(format!("Output paths overlap: {}", names.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join("  \u{b7}  "))
}

/// The note under the counts: WASAPI shared mode offers one channel count.
pub const WASAPI_NOTE: &str = "WASAPI shared mode offers only the system mix format's channel count; more channels need ASIO.";

pub fn is_wasapi(system: &str) -> bool {
    system.eq_ignore_ascii_case("WASAPI")
}

/// Why the input has no channels to make a path on, said beside its disabled Add (#429).
pub const INPUT_NOT_OPEN: &str = "No input channels yet: the input device opens when a track is armed or monitored.";
/// Why the output has no channels to make a path on.
pub const OUTPUT_NOT_OPEN: &str = "No output channels: the output device is not open.";
/// The hover of a path's only channel in the picker: a path keeps at least one.
pub const LAST_CHANNEL: &str = "A path keeps at least one channel; Remove deletes the path.";

/// The note beside the inputs' Add when the device has no input channels open: the input's open
/// error when it failed (`failure`), else that it opens on demand. `None` while channels are open.
pub fn input_note(inputs: u16, failure: Option<&str>) -> Option<String> {
    (inputs == 0).then(|| failure.map_or_else(|| INPUT_NOT_OPEN.to_string(), |why| format!("The input device did not open: {why}")))
}

/// What the Inputs list says when inputs are open and no input path can be used (rule 14).
pub const NO_INPUT_PATHS: &str = "No input paths";
/// The hover of Add default paths.
pub const ADD_DEFAULTS_TIP: &str =
    "Add the default input paths this input lacks (In 1, In 2, ... and the stereo pairs); paths you renamed or made stay as they are.";

/// The input the table's input half belongs to, as the page names it: the capture endpoint's
/// display name and its own channel count (which can exceed the table's cap).
#[derive(Debug, Clone, Copy)]
pub struct InputDevice<'a> {
    pub name: &'a str,
    pub channels: usize,
    /// Two input devices share this name (cpal): it has no identity, so its paths are this open's
    /// defaults and nothing is saved for it (amendment for #431, rule 8).
    pub ambiguous: bool,
}

/// Said after the input in the heading when two input devices share its name.
pub const AMBIGUOUS_INPUT: &str = "two input devices share this name: its paths are not saved and no measurement applies";

/// The heading line: both devices and their channels (rule 14), "Input: <device>, N channels.
/// Output: <device>, M channels." A device wider than the 32-channel table says that only the
/// first 32 are used (ADR 0015 section 1.1).
pub fn heading(io: &IoMap, input: Option<InputDevice<'_>>, output_channels: usize) -> String {
    let count = |n: usize| if n == 1 { "1 channel".to_string() } else { format!("{n} channels") };
    let capped = |n: usize| {
        if n > transit_model::MAX_DEVICE_CHANNELS {
            format!("{} (the first {} are used)", count(n), transit_model::MAX_DEVICE_CHANNELS)
        } else {
            count(n)
        }
    };
    let input = match input {
        Some(i) if i.ambiguous => format!("Input: {}, {} ({AMBIGUOUS_INPUT}).", i.name, capped(i.channels)),
        Some(i) => format!("Input: {}, {}.", i.name, capped(i.channels)),
        None => "Input: not open.".to_string(),
    };
    format!("{input} Output: {}, {}.", io.device, capped(output_channels.max(usize::from(io.outputs))))
}

/// Why an input path is listed but unavailable, for its row; `None` when it can be used.
pub fn unavailable_text(io: &IoMap, p: &IoPath) -> Option<String> {
    if p.direction != IoDirection::Input {
        return None;
    }
    io.input_unavailable(p).map(|why| format!("Unavailable: {why}"))
}

/// The edit a click on channel `c` of `p`'s picker makes, or `None` when it would leave the path
/// without channels (the engine refuses a path 0 channels wide; #429 sent it eight times).
pub fn channel_edit(p: &IoPath, c: u16) -> Option<IoEdit> {
    let channels = toggled(&p.channels, c);
    (!channels.is_empty()).then(|| IoEdit::SetChannels { name: p.name.clone(), direction: p.direction, channels })
}

/// What the Hardware inserts subsection asks for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardwareAction {
    /// Run `io.measure_hardware_latency` (Measure and Remeasure).
    Measure { send: String, ret: String },
    /// Stop the measurement that is running.
    Cancel,
    /// Run `io.clear_hardware_latency`.
    Clear { send: String, ret: String },
}

/// What the subsection showed, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HardwareLayout {
    /// Why Measure is off; empty when it can run.
    pub blocked: String,
    pub send: String,
    pub ret: String,
    /// "Measuring… <n>/<total> clicks" while a measurement runs; empty otherwise.
    pub progress: String,
    /// "<n> frames (<x> ms) ±<m>" or the refusal of the last run.
    pub result: String,
    /// The measured pairs listed: "Out 3-4 → In 3-4: 612 frames (12.8 ms)".
    pub rows: Vec<String>,
}

/// What the user is typing or editing now (the table itself is the engine's).
#[derive(Default)]
pub struct IoEditor {
    /// The pair picked for Measure.
    hw_send: Option<String>,
    hw_ret: Option<String>,
    /// What the last measurement said.
    pub hw_result: String,
    /// A path being renamed: (its name, its direction, the text so far).
    rename: Option<(String, IoDirection, String)>,
    /// The path whose channels are being picked, and its direction.
    editing: Option<(String, IoDirection)>,
    new_output: String,
    new_input: String,
    /// The engine's refusal of the last edit, shown on the page in its words until an edit is
    /// accepted (#429: it reached only the status line under this window).
    pub refusal: String,
    /// The table (system, device) the refusal was about: another device's page drops it.
    refused_on: (String, String),
}

impl IoEditor {
    /// An edit was accepted: drop the text it came from.
    /// The engine refused an edit of `io`: say it on the page.
    pub fn refuse(&mut self, error: &str, io: &IoMap) {
        self.refusal = error.to_string();
        self.refused_on = (io.system.clone(), io.device.clone());
    }

    /// A refusal about another device's table (the device changed) is dropped.
    pub fn follow(&mut self, io: &IoMap) {
        if (self.refused_on.0.as_str(), self.refused_on.1.as_str()) != io.key() {
            self.refusal.clear();
        }
    }

    pub fn done(&mut self, edit: &IoEdit) {
        self.refusal.clear();
        match edit {
            IoEdit::Add { direction: IoDirection::Output, .. } => self.new_output.clear(),
            IoEdit::Add { direction: IoDirection::Input, .. } => self.new_input.clear(),
            IoEdit::Rename { .. } => self.rename = None,
            IoEdit::Remove { name, direction } if self.editing.as_ref().is_some_and(|(n, d)| n == name && d == direction) => self.editing = None,
            _ => {}
        }
    }
}

fn dim(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).color(TOKENS.text_dim).size(11.5));
}

/// The names the open device gives its channels (ASIO `getChannelInfo`), in channel order; empty
/// where the backend does not report them.
#[derive(Debug, Clone, Default)]
pub struct ChannelNames {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
}

impl ChannelNames {
    fn of(&self, direction: IoDirection) -> &[String] {
        match direction {
            IoDirection::Output => &self.outputs,
            IoDirection::Input => &self.inputs,
        }
    }
}

/// How the channel picker shows device channel `c` (0-based): its 1-based number, with the
/// device's name for it where there is one ("Analog In 12" as it is when it already ends in the
/// number, "1 Mic/Line" otherwise).
pub fn channel_text(c: u16, names: &[String]) -> String {
    let number = (c + 1).to_string();
    match names.get(usize::from(c)).map(|n| n.trim()).filter(|n| !n.is_empty()) {
        Some(name) if name.rsplit(' ').next() == Some(number.as_str()) => name.to_string(),
        Some(name) => format!("{number} {name}"),
        None => number,
    }
}

/// The section. Returns the edit the user made this frame, if any; controls are listed in `items`
/// (label shown, value the control) for `ui.inspect` and tests. `names` labels the channels;
/// `input_note` says why there are no input channels ([`input_note`]); `heading` names both
/// devices ([`heading`]).
#[allow(clippy::too_many_arguments)]
pub fn section(
    ui: &mut egui::Ui,
    io: &IoMap,
    wasapi: bool,
    names: &ChannelNames,
    input_note: Option<&str>,
    heading: &str,
    ed: &mut IoEditor,
    items: &mut Vec<MenuItem>,
) -> Option<IoEdit> {
    let mut edit = None;
    ui.label(egui::RichText::new("I/O paths").strong());
    let r = ui.label(egui::RichText::new(heading).color(TOKENS.text_dim).size(11.5));
    items.push(MenuItem::at(ui, heading.to_string(), "io_heading".into(), r.rect));
    if !ed.refusal.is_empty() {
        let r = crate::chrome::status_label(ui, ed.refusal.clone(), 11.5, crate::chrome::Severity::Error);
        items.push(MenuItem::at(ui, ed.refusal.clone(), "io_refusal".into(), r.rect));
    }
    if wasapi {
        let r = ui.label(egui::RichText::new(WASAPI_NOTE).color(TOKENS.text_dim).size(11.5));
        items.push(MenuItem::at(ui, WASAPI_NOTE.into(), "io_wasapi_note".into(), r.rect));
    }
    let outputs: Vec<&IoPath> = io.paths_of(IoDirection::Output).collect();
    crate::chrome::control_row(ui, |ui| {
        ui.label("Main plays on");
        let combo = crate::chrome::field(ui, |ui| {
            crate::chrome_pick::Combo::new("io_main", &io.main).show(ui, |ui| {
                for p in &outputs {
                    let b = crate::menus::check_button(ui, p.name.eq_ignore_ascii_case(&io.main), &p.name);
                    items.push(MenuItem::at(ui, p.name.clone(), "io_main_entry".into(), b.rect));
                    if b.clicked() {
                        edit = Some(IoEdit::SetMain { name: p.name.clone() });
                    }
                }
            })
        });
        items.push(MenuItem::at(ui, io.main.clone(), "io_main".into(), combo.response.rect));
    });
    for line in overlap_lines(io) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(icons::ICON, icons::ICON), egui::Sense::hover());
            icons::paint(ui.painter(), r.center(), Icon::Error, 14.0, TOKENS.record);
            ui.label(egui::RichText::new(line).color(TOKENS.text).size(11.5));
        });
    }
    for (direction, title) in [(IoDirection::Output, "Outputs"), (IoDirection::Input, "Inputs")] {
        let opened = if direction == IoDirection::Input { io.inputs } else { io.outputs };
        ui.add_space(SPACE);
        ui.label(egui::RichText::new(title).color(TOKENS.text_dim));
        egui::ScrollArea::vertical().id_salt(("io_paths", title)).max_height(100.0).show(ui, |ui| {
            egui::Grid::new(("io_paths_grid", title)).num_columns(4).spacing([SPACE, SPACE / 2.0]).show(ui, |ui| {
                for p in io.paths_of(direction) {
                    path_row(ui, io, p, ed, items, &mut edit);
                    ui.end_row();
                }
            });
        });
        if let Some((name, editing)) = ed.editing.clone()
            && let Some(p) = io.paths_of(direction).find(|p| editing == direction && p.name.eq_ignore_ascii_case(&name))
        {
            ui.horizontal_wrapped(|ui| {
                dim(ui, format!("{} channels (1 to {MAX_PATH_WIDTH}):", p.name));
                for c in 0..opened {
                    let text = channel_text(c, names.of(direction));
                    let b = crate::chrome_pick::option_label(ui, p.channels.contains(&c), &text);
                    items.push(MenuItem::at(ui, text, format!("io_channel:{}", p.name), b.rect));
                    let next = channel_edit(p, c);
                    let b = if next.is_none() { b.on_hover_text(LAST_CHANNEL) } else { b };
                    if b.clicked() && next.is_some() {
                        edit = next;
                    }
                }
            });
        }
        // Inputs open and no input path usable: say so and offer the defaults (rule 14).
        if direction == IoDirection::Input && io.lacks_input_paths() {
            ui.horizontal(|ui| {
                let r = crate::chrome::status_label(ui, NO_INPUT_PATHS, 11.5, crate::chrome::Severity::Warning);
                items.push(MenuItem::at(ui, NO_INPUT_PATHS.into(), "io_no_inputs".into(), r.rect));
                let b = ui.button("Add default paths");
                items.push(MenuItem::at(ui, "Add default paths".into(), "io_add_defaults".into(), b.rect));
                if b.on_hover_text(ADD_DEFAULTS_TIP).clicked() {
                    edit = Some(IoEdit::AddDefaultInputs);
                }
            });
        }
        // A disabled Add says why, beside it and on its hover, never silently (#429).
        let why = (opened == 0).then(|| if direction == IoDirection::Input { input_note.unwrap_or(INPUT_NOT_OPEN) } else { OUTPUT_NOT_OPEN });
        ui.horizontal(|ui| {
            let buf = if direction == IoDirection::Input { &mut ed.new_input } else { &mut ed.new_output };
            crate::chrome::text_field(ui, egui::TextEdit::singleline(buf).hint_text("New path name").desired_width(160.0));
            let b = ui.add_enabled(opened > 0, egui::Button::new("Add"));
            let key = if direction == IoDirection::Input { "io_add:input" } else { "io_add:output" };
            items.push(MenuItem::at(ui, "Add".into(), key.into(), b.rect));
            let b = b.on_hover_text("A path on the first free channel; pick its channels with Channels.");
            let b = match why {
                Some(why) => b.on_disabled_hover_text(why),
                None => b,
            };
            if b.clicked() && opened > 0 {
                edit = Some(IoEdit::Add { direction, name: buf.clone(), channels: vec![first_free(io, direction)] });
            }
        });
        if let Some(why) = why {
            let r = crate::chrome::status_label(ui, why, 11.5, crate::chrome::Severity::Warning);
            let key = if direction == IoDirection::Input { "io_add_reason:input" } else { "io_add_reason:output" };
            items.push(MenuItem::at(ui, why.to_string(), key.into(), r.rect));
        }
    }
    edit
}

/// The "Hardware inserts" subsection: pick a send path and a return path and Measure, then the
/// measured table with Remeasure and Clear. `blocked` is why Measure is off (no duplex device, the
/// transport running), when it is. Controls go to `items`; the layout says what was shown.
pub fn hardware_section(
    ui: &mut egui::Ui,
    io: &IoMap,
    (rate, timing): (u32, crate::hardware_ui::Timing),
    blocked: Option<&str>,
    progress: Option<(u32, u32)>,
    ed: &mut IoEditor,
    items: &mut Vec<MenuItem>,
) -> (Option<HardwareAction>, HardwareLayout) {
    use crate::hardware_ui::frames_text;
    let mut action = None;
    ui.label(egui::RichText::new("Hardware inserts").strong());
    dim(ui, "Time the round trip through outboard gear: a click plays on the send path and is found again on the return path.");
    let outputs: Vec<&str> = io.paths_of(IoDirection::Output).map(|p| p.name.as_str()).collect();
    // Only a return the open input can deliver (amendment for #431, rule 10).
    let inputs: Vec<&str> = io.paths_of(IoDirection::Input).filter(|p| io.input_unavailable(p).is_none()).map(|p| p.name.as_str()).collect();
    // A pick that no longer names a path is dropped.
    if ed.hw_send.as_deref().is_some_and(|n| !outputs.contains(&n)) {
        ed.hw_send = None;
    }
    if ed.hw_ret.as_deref().is_some_and(|n| !inputs.contains(&n)) {
        ed.hw_ret = None;
    }
    let running = progress.is_some();
    crate::chrome::control_row(ui, |ui| {
        ui.add_enabled_ui(!running, |ui| {
            for (label, key, names, pick) in [("Send", "hw_send", &outputs, &mut ed.hw_send), ("Return", "hw_ret", &inputs, &mut ed.hw_ret)] {
                ui.label(label);
                let combo = crate::chrome::field(ui, |ui| {
                    crate::chrome_pick::Combo::new(key, pick.as_deref().unwrap_or("Choose…")).show(ui, |ui| {
                        for n in names {
                            let b = crate::menus::check_button(ui, pick.as_deref() == Some(*n), *n);
                            items.push(MenuItem::at(ui, (*n).to_string(), format!("{key}_entry"), b.rect));
                            if b.clicked() {
                                *pick = Some((*n).to_string());
                            }
                        }
                    })
                });
                items.push(MenuItem::at(ui, pick.clone().unwrap_or_default(), key.into(), combo.response.rect));
            }
        });
        let ready = ed.hw_send.is_some() && ed.hw_ret.is_some();
        let b = ui.add_enabled(ready && blocked.is_none() && !running, egui::Button::new("Measure"));
        items.push(MenuItem::at(ui, "Measure".into(), "hw_measure".into(), b.rect));
        let tip = blocked.unwrap_or(if running {
            "A measurement is running."
        } else if ready {
            "Play a click out the send path and time it on the return path."
        } else {
            "Choose a send path and a return path."
        });
        if b.on_hover_text(tip).on_disabled_hover_text(tip).clicked()
            && let (Some(send), Some(ret)) = (ed.hw_send.clone(), ed.hw_ret.clone())
        {
            action = Some(HardwareAction::Measure { send, ret });
        }
    });
    let mut progress_text = String::new();
    if let Some((sent, total)) = progress {
        progress_text = format!("Measuring\u{2026} {sent}/{total} clicks");
        ui.horizontal(|ui| {
            let r = ui.label(egui::RichText::new(&progress_text).color(TOKENS.text).size(11.5));
            items.push(MenuItem::at(ui, progress_text.clone(), "hw_progress".into(), r.rect));
            let b = ui.button("Cancel");
            items.push(MenuItem::at(ui, "Cancel".into(), "hw_cancel".into(), b.rect));
            if b.on_hover_text("Stop measuring; nothing is stored.").clicked() {
                action = Some(HardwareAction::Cancel);
            }
        });
    }
    if let Some(why) = blocked {
        dim(ui, why);
    }
    if !ed.hw_result.is_empty() {
        let r = ui.label(egui::RichText::new(&ed.hw_result).color(TOKENS.text).size(11.5));
        items.push(MenuItem::at(ui, ed.hw_result.clone(), "hw_result".into(), r.rect));
    }
    let mut layout = HardwareLayout {
        blocked: blocked.unwrap_or_default().to_string(),
        progress: progress_text,
        send: ed.hw_send.clone().unwrap_or_default(),
        ret: ed.hw_ret.clone().unwrap_or_default(),
        result: ed.hw_result.clone(),
        rows: Vec::new(),
    };
    let measured: Vec<_> = io.hardware_latency.iter().filter(|m| m.rate == rate).collect();
    if measured.is_empty() {
        dim(ui, "Nothing measured yet.");
    } else {
        egui::Grid::new("hw_latency_grid").num_columns(3).spacing([SPACE, SPACE / 2.0]).show(ui, |ui| {
            for m in measured {
                let pair = format!("{} → {}", m.send, m.ret);
                ui.label(&pair);
                // A figure through another input is listed, not used: "measure again" (rule 13).
                // Through a JACK virtual input, also over other ports than now (rule 13).
                let figure = if io.figure_applies(&m.input_device, &m.input_ports) {
                    // Used as measured; with the hint when the device's rate differs or is not
                    // recorded (ADR 0015, Amendment 2026-11-16 on the #709 converter term).
                    match rate_row(m, timing) {
                        Some(why) => format!("{}, {why}", frames_text(m.round_trip_frames, rate)),
                        None => frames_text(m.round_trip_frames, rate),
                    }
                } else {
                    let why = if m.input_device.is_empty() {
                        MEASURE_AGAIN_UNKNOWN
                    } else if io.returns_through(&m.input_device) {
                        MEASURE_AGAIN_PORTS
                    } else {
                        MEASURE_AGAIN_ROW
                    };
                    format!("{}, {why}", frames_text(m.round_trip_frames, rate))
                };
                ui.label(&figure);
                layout.rows.push(format!("{pair}: {figure}"));
                ui.horizontal(|ui| {
                    let b = ui.add_enabled(blocked.is_none() && !running, egui::Button::new("Remeasure").small());
                    items.push(MenuItem::at(ui, "Remeasure".into(), format!("hw_remeasure:{pair}"), b.rect));
                    if b.clicked() {
                        action = Some(HardwareAction::Measure { send: m.send.clone(), ret: m.ret.clone() });
                    }
                    let b = ui.small_button("Clear");
                    items.push(MenuItem::at(ui, "Clear".into(), format!("hw_clear:{pair}"), b.rect));
                    if b.clicked() {
                        action = Some(HardwareAction::Clear { send: m.send.clone(), ret: m.ret.clone() });
                    }
                });
                ui.end_row();
            }
        });
    }
    (action, layout)
}

/// Beside a listed round trip measured through another input than the one open: it is not used.
pub const MEASURE_AGAIN_ROW: &str = "through another input: measure again";
/// The same for a figure stored before inputs were recorded, whose input is not established.
pub const MEASURE_AGAIN_UNKNOWN: &str = "through an unknown input: measure again";
/// The same, for a figure through a JACK virtual input whose ports changed since (rule 13).
pub const MEASURE_AGAIN_PORTS: &str = "through other ports of this input: measure again";

/// Beside a listed round trip that applies, used with the device and input `timing`: why it says
/// "measure again at this rate" or "timing changed", `None` when it says neither
/// ([`transit_model::measured_hint`]).
pub fn rate_row(m: &transit_model::HardwareLatency, timing: crate::hardware_ui::Timing) -> Option<String> {
    use crate::hardware_ui::{MEASURE_AGAIN_RATE, TIMING_CHANGED};
    // At equal rates "timing changed" never shows (#709 round 3): the hints read an invalid alignment as valid.
    let hint = m.hint(timing.device_rate, transit_model::hint_alignment(timing.alignment, m.rate, timing.device_rate))?;
    Some(if hint == transit_model::LatencyHint::TimingChanged {
        TIMING_CHANGED.to_string()
    } else if m.device_rate == 0 {
        format!("device rate not recorded: {MEASURE_AGAIN_RATE}")
    } else if timing.device_rate.is_some_and(|r| r != m.device_rate) {
        format!("measured with the device at {}: {MEASURE_AGAIN_RATE}", crate::external_measure::rate_text(m.device_rate))
    } else {
        format!("measured with another input timing: {MEASURE_AGAIN_RATE}")
    })
}

/// What a finished measurement says: "612 frames (12.8 ms) ±2".
pub fn measured_text(frames: u32, spread: u32, rate: u32) -> String {
    format!("{} ±{spread}", crate::hardware_ui::frames_text(frames, rate))
}

fn path_row(ui: &mut egui::Ui, io: &IoMap, p: &IoPath, ed: &mut IoEditor, items: &mut Vec<MenuItem>, edit: &mut Option<IoEdit>) {
    if let Some((_, _, buf)) = ed.rename.as_mut().filter(|(n, d, _)| *n == p.name && *d == p.direction) {
        let r = crate::chrome::text_field(ui, egui::TextEdit::singleline(buf).desired_width(130.0));
        r.request_focus();
        if r.lost_focus() {
            if crate::modal::reaches(ui) && ui.input(|i| i.key_pressed(egui::Key::Enter)) && *buf != p.name {
                *edit = Some(IoEdit::Rename { name: p.name.clone(), direction: p.direction, new_name: buf.clone() });
            } else {
                ed.rename = None;
            }
        }
    } else {
        let main = p.direction == IoDirection::Output && p.name.eq_ignore_ascii_case(&io.main);
        // An unavailable input is listed with why (rule 14): its definition is kept for repair.
        match unavailable_text(io, p) {
            Some(why) => {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&p.name).color(TOKENS.text_dim));
                    let r = crate::chrome::status_label(ui, why.clone(), 11.0, crate::chrome::Severity::Warning);
                    items.push(MenuItem::at(ui, why, format!("io_unavailable:{}", p.name), r.rect));
                });
            }
            None => {
                ui.label(if main { format!("{} (Main)", p.name) } else { p.name.clone() });
            }
        }
    }
    ui.label(channel_label(&p.channels));
    ui.label(format!("{} ch", p.channels.len()));
    let can = !(p.direction == IoDirection::Output && p.name.eq_ignore_ascii_case(&io.main));
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let b = ui.small_button("Channels");
            items.push(MenuItem::at(ui, "Channels".into(), format!("io_channels:{}", p.name), b.rect));
            if b.clicked() {
                let this = (p.name.clone(), p.direction);
                ed.editing = if ed.editing.as_ref() == Some(&this) { None } else { Some(this) };
            }
            let b = ui.small_button("Rename");
            items.push(MenuItem::at(ui, "Rename".into(), format!("io_rename:{}", p.name), b.rect));
            if b.clicked() {
                ed.rename = Some((p.name.clone(), p.direction, p.name.clone()));
            }
            let b = ui.add_enabled(can, egui::Button::new("Remove").small());
            items.push(MenuItem::at(ui, "Remove".into(), format!("io_remove:{}", p.name), b.rect));
            if b.on_disabled_hover_text("Main plays here: choose another path for Main first.").clicked() {
                *edit = Some(IoEdit::Remove { name: p.name.clone(), direction: p.direction });
            }
        });
        if can && p.direction == IoDirection::Output {
            let mut live = p.live;
            let b = crate::chrome::checkbox(ui, &mut live, "Live (no shared padding)");
            crate::help::hover(ui, &b, crate::help::Id::AudioDevicesPathLive);
            items.push(MenuItem::at(ui, "Live (no shared padding)".into(), format!("io_live:{}", p.name), b.rect));
            if b.changed() {
                *edit = Some(IoEdit::SetLive { name: p.name.clone(), live });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_read_one_based_and_contiguous_runs_collapse() {
        assert_eq!(channel_label(&[0]), "1");
        assert_eq!(channel_label(&[2, 3]), "3-4");
        assert_eq!(channel_label(&[0, 1, 2, 3, 4, 5]), "1-6");
        assert_eq!(channel_label(&[0, 1, 4]), "1-2, 5");
        assert_eq!(channel_label(&[3, 1]), "4, 2", "the path's own order is kept");
        assert_eq!(channel_label(&[]), "");
    }

    /// Rule 14's heading, and section 1.1's note for a device wider than the 32-channel table
    /// (#431 Q6: a JACK port group of 40 ports).
    #[test]
    fn the_heading_names_both_devices_and_says_when_only_32_channels_are_used() {
        let io = IoMap::for_device("JACK", "Big", 32, 32);
        assert_eq!(
            heading(&io, Some(InputDevice { name: "Big", channels: 40, ambiguous: false }), 40),
            "Input: Big, 40 channels (the first 32 are used). Output: Big, 40 channels (the first 32 are used)."
        );
        let io = IoMap::for_device("WASAPI", "Speakers", 0, 2);
        assert_eq!(
            heading(&io, Some(InputDevice { name: "Microphone", channels: 1, ambiguous: false }), 2),
            "Input: Microphone, 1 channel. Output: Speakers, 2 channels."
        );
        assert_eq!(heading(&io, None, 0), "Input: not open. Output: Speakers, 2 channels.");
        assert_eq!(
            heading(&io, Some(InputDevice { name: "USB Audio", channels: 2, ambiguous: true }), 2),
            format!("Input: USB Audio, 2 channels ({AMBIGUOUS_INPUT}). Output: Speakers, 2 channels.")
        );
    }

    #[test]
    fn toggling_a_channel_keeps_the_set_in_order() {
        assert_eq!(toggled(&[0, 1], 3), [0, 1, 3]);
        assert_eq!(toggled(&[0, 1, 3], 1), [0, 3]);
        assert_eq!(toggled(&[2], 0), [0, 2]);
    }

    #[test]
    fn each_edit_is_the_setup_io_map_command_it_names() {
        let add = IoEdit::Add { direction: IoDirection::Input, name: "Mic".into(), channels: vec![2] };
        assert_eq!(add.params(), json!({"action": "add_path", "direction": "input", "name": "Mic", "channels": [2]}));
        assert_eq!(IoEdit::SetMain { name: "Out 3-4".into() }.params(), json!({"action": "set_main", "name": "Out 3-4"}));
        assert_eq!(
            IoEdit::Remove { name: "Cue".into(), direction: IoDirection::Output }.params(),
            json!({"action": "remove_path", "name": "Cue", "direction": "output"})
        );
        assert_eq!(
            IoEdit::Rename { name: "Cue".into(), direction: IoDirection::Input, new_name: "Talkback".into() }.params(),
            json!({"action": "rename_path", "name": "Cue", "direction": "input", "new_name": "Talkback"})
        );
        assert_eq!(IoEdit::AddDefaultInputs.params(), json!({"action": "add_default_inputs"}));
    }

    #[test]
    fn a_new_path_starts_on_the_first_free_channel_and_overlaps_are_said() {
        let mut io = IoMap::for_device("ASIO", "x", 2, 4);
        assert_eq!(first_free(&io, IoDirection::Output), 0, "the defaults cover every channel");
        io.remove_path("Out 3-4").unwrap();
        assert_eq!(first_free(&io, IoDirection::Output), 2);
        assert!(overlap_lines(&io).is_empty());
        io.add_path(IoPath {
            live: false,
            name: "Cue".into(),
            direction: IoDirection::Output,
            channels: vec![1],
            format: transit_model::ChannelFormat::Mono,
        })
        .unwrap();
        assert_eq!(overlap_lines(&io), ["Out 1-2 and Cue share channel 2 and add together there."]);
        assert!(is_wasapi("wasapi") && !is_wasapi("ASIO"));
    }
}
