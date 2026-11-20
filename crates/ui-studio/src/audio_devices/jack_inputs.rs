//! Audio › Audio Devices on native JACK (ADR 0015, amendment for #431, slice C): the Input list
//! with virtual devices behind Show virtual devices (rule 1), the chosen input always shown and an
//! absent one said by name (rules 3 and 4), the timing warning (rule 7), the monitor label with
//! "transit's output" (rule 13), and on the I/O paths page Use the current ports (rule 5).
//!
//! Show virtual devices only filters what the list offers: toggling it takes effect at once, is
//! kept with the audio preferences and reopens nothing (the window keeps it out of the pending
//! changes).

use super::{SYSTEM_DEFAULT, device_label};
use crate::audio_devices_io::IoEdit;
use crate::mixer::MenuItem;
use crate::theme::TOKENS;
use transit_model::IoMap;
use transit_playback::devices::{AudioConfig, AudioSystem, DeviceInfo};

/// The checkbox under the Input list.
pub const SHOW_VIRTUAL: &str = "Show virtual devices";
/// Its note (rule 1).
pub const VIRTUAL_NOTE: &str = "An application playing several streams appears as one device whose ports can change between opens; paths over changed ports stay off until you use the current ports.";
/// The timing warning, under the checkbox and on the hover of each virtual device (rule 7).
pub const TIMING_WARNING: &str =
    "Choosing a virtual device can change the server's buffer size or rate; transit then stops, keeps the take and reopens.";
/// The hover of the monitor of transit's own output (rule 13).
pub const FEEDBACK_HOVER: &str = "Monitoring this input plays the output back into itself.";
/// The I/O paths page's action (rule 5).
pub const USE_CURRENT_PORTS: &str = "Use the current ports";
/// The line beside it.
pub const PORTS_CHANGED: &str = "The input's ports changed since its paths were made: paths over them are off.";
/// Its hover.
pub const USE_PORTS_TIP: &str = "Make the ports open now the ones the paths read. Measurements through this input need measuring again.";

/// One entry of the JACK Input list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEntry {
    pub name: String,
    pub label: String,
    pub hover: Option<String>,
}

/// How the list shows a chosen input that is not on the server (rule 3).
pub fn absent_text(name: &str) -> String {
    format!("{name} (not present: no input opens)")
}

/// The label of an input group: a sink's monitor as System Audio, with ", transit's output" when it
/// monitors the output transit plays to (`output`), else the device as it is.
pub fn input_label(d: &DeviceInfo, output: Option<&str>) -> String {
    let Some(mut label) = transit_playback::system_audio::monitor_label(&d.name, &d.channel_names) else { return device_label(d) };
    if output == Some(d.name.as_str()) {
        label.push_str(", transit's output");
    }
    if d.max_channels > 0 { format!("{label} · {} ch", d.max_channels) } else { label }
}

/// What the Input list offers on the JACK system `sys`: the physical groups, then the virtual
/// devices when Show virtual devices is on; the chosen input always, when it is present (rule 3).
/// `output` is the output transit plays to (the chosen one, else the one open).
pub fn entries(sys: &AudioSystem, draft: &AudioConfig, output: Option<&str>) -> Vec<InputEntry> {
    let monitors_output =
        |d: &DeviceInfo| output == Some(d.name.as_str()) && transit_playback::system_audio::monitor_label(&d.name, &d.channel_names).is_some();
    let physical = sys.inputs.iter().map(|d| InputEntry {
        name: d.name.clone(),
        label: input_label(d, output),
        hover: monitors_output(d).then(|| FEEDBACK_HOVER.to_owned()),
    });
    let chosen = draft.input.as_deref();
    let shown = sys.virtual_inputs.iter().filter(|d| draft.jack_show_virtual || chosen == Some(d.name.as_str()));
    let virtuals = shown.filter(|d| !sys.inputs.iter().any(|p| p.name == d.name)).map(|d| InputEntry {
        name: d.name.clone(),
        label: input_label(d, output),
        hover: Some(if monitors_output(d) { format!("{TIMING_WARNING} {FEEDBACK_HOVER}") } else { TIMING_WARNING.to_owned() }),
    });
    physical.chain(virtuals).collect()
}

/// The JACK Input list: "System default", then `entries`, each with its hover. A chosen input
/// that is not present shows by name, marked (rule 3). Returns the combo's rect and text.
pub fn choose(
    ui: &mut egui::Ui,
    enabled: bool,
    value: &mut Option<String>,
    entries: &[InputEntry],
    items: &mut Vec<MenuItem>,
) -> (egui::Rect, String) {
    let text = match value.as_deref() {
        None => SYSTEM_DEFAULT.to_owned(),
        Some(name) => entries.iter().find(|e| e.name == name).map_or_else(|| absent_text(name), |e| e.label.clone()),
    };
    let r = ui.add_enabled_ui(enabled, |ui| {
        crate::chrome::field(ui, |ui| {
            crate::chrome_pick::Combo::new(("audio_devices", "input"), &text).width(360.0).show(ui, |ui| {
                let b = crate::menus::check_button(ui, value.is_none(), SYSTEM_DEFAULT);
                items.push(MenuItem::at(ui, SYSTEM_DEFAULT.into(), "input".into(), b.rect));
                if b.clicked() {
                    *value = None;
                }
                for e in entries {
                    let b = crate::menus::check_button(ui, value.as_deref() == Some(e.name.as_str()), &e.label);
                    items.push(MenuItem::at(ui, e.label.clone(), "input".into(), b.rect));
                    let b = match &e.hover {
                        Some(hover) => b.on_hover_text(hover),
                        None => b,
                    };
                    if b.clicked() {
                        *value = Some(e.name.clone());
                    }
                }
            })
        })
    });
    (r.inner.response.rect, text)
}

/// The rows under the Input list in the window's grid: the checkbox, its note and the timing
/// warning. Returns whether the checkbox was clicked.
pub fn virtual_rows(ui: &mut egui::Ui, enabled: bool, on: &mut bool, controls: &mut Vec<(String, &'static str, egui::Rect)>) -> bool {
    ui.label("");
    let b = crate::chrome::checkbox_enabled(ui, enabled, on, SHOW_VIRTUAL);
    controls.push((SHOW_VIRTUAL.into(), "jack_show_virtual", b.rect));
    ui.end_row();
    for (text, key) in [(VIRTUAL_NOTE, "jack_virtual_note"), (TIMING_WARNING, "jack_timing_warning")] {
        ui.label("");
        let shown = ui.add(egui::Label::new(egui::RichText::new(text).color(TOKENS.text_dim).size(11.0)).wrap());
        controls.push((text.into(), key, shown.rect));
        ui.end_row();
    }
    b.changed()
}

/// The I/O paths page's port-mapping guard (rule 5): when the open virtual input's ports differ
/// from the ones its paths were made over, a warning with both lists and Use the current ports,
/// which works only while the transport is stopped (`stopped`). Nothing otherwise.
pub fn ports_row(ui: &mut egui::Ui, io: &IoMap, stopped: bool, items: &mut Vec<MenuItem>) -> Option<IoEdit> {
    if !io.ports_mismatch() {
        return None;
    }
    let mut edit = None;
    ui.add_space(crate::theme::SPACE);
    let r = crate::chrome::status_label(ui, PORTS_CHANGED, 11.5, crate::chrome::Severity::Warning);
    items.push(MenuItem::at(ui, PORTS_CHANGED.into(), "io_ports_changed".into(), r.rect));
    let detail = ports_detail(io);
    let r = ui.add(egui::Label::new(egui::RichText::new(&detail).color(TOKENS.text_dim).size(11.5)).wrap());
    items.push(MenuItem::at(ui, detail, "io_ports_detail".into(), r.rect));
    let b = ui.add_enabled(stopped, egui::Button::new(USE_CURRENT_PORTS));
    items.push(MenuItem::at(ui, USE_CURRENT_PORTS.into(), "io_use_ports".into(), b.rect));
    if b.on_hover_text(USE_PORTS_TIP).on_disabled_hover_text(transit_engine::USE_PORTS_STOPPED).clicked() {
        edit = Some(IoEdit::UseCurrentPorts);
    }
    edit
}

/// "Paths read: output_FL, output_FR. Open now: output_FR, output_FL."
pub fn ports_detail(io: &IoMap) -> String {
    let list = |ports: &[String]| if ports.is_empty() { "none recorded".to_owned() } else { ports.join(", ") };
    format!("Paths read: {}. Open now: {}.", list(&io.input_baseline), list(&io.input_ports))
}
