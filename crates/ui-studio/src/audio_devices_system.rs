//! Audio › Audio Devices, I/O paths section: the System Audio input (#268), what the computer plays
//! as a stereo input path beside the interface's inputs. The switch and the output to capture are
//! part of the draft (Apply reopens the input with them); the status line and the level meter read
//! the open input's loopback capture every frame. On Linux the block says how the monitor source
//! of an output arrives instead (no capture code of transit's own there).

use crate::mixer::MenuItem;
use crate::theme::TOKENS;
use transit_playback::devices::{AudioConfig, DeviceInfo};
use transit_playback::system_audio::{self, Report};

/// The note under the switch: the help's rights sentence, then what happens to a surround output.
pub const SWITCH_NOTE: &str = "System Audio records what this computer plays, as a loopback cable would. You are responsible for the rights to what you record. A 5.1 or 7.1 output is folded to its front pair and the center channel (the dialogue) is dropped, so a stereo output is the better pick.";
/// What the block says where transit has no capture of its own (Linux, macOS).
pub const ELSEWHERE_NOTE: &str = if cfg!(target_os = "linux") {
    "On Linux what an output plays arrives as an input already: the PipeWire or PulseAudio monitor source of that output, listed as System Audio (monitor of <output>) where transit can tell. If none is listed, route the monitor to transit's input in pavucontrol (Recording tab) or qpwgraph."
} else if cfg!(target_os = "macos") {
    "On macOS System Audio needs a virtual audio device (a loopback driver) chosen as the input; transit's own capture comes with the Core Audio backend."
} else {
    "System Audio needs the native WASAPI backend in this build."
};
/// The picker's words for no choice.
pub const DEFAULT_OUTPUT: &str = "System default output";

/// What the block shows besides the draft.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// This build captures System Audio (Windows with native WASAPI).
    pub offered: bool,
    /// The outputs to pick from, by name, the default first.
    pub outputs: Vec<String>,
    /// The open input's System Audio, `None` while no input is open.
    pub report: Option<Report>,
    /// System Audio as the running configuration has it (what Apply last opened).
    pub running_on: bool,
}

/// The status line under the switch.
pub fn status_line(view: &View, draft_on: bool) -> String {
    if draft_on != view.running_on {
        return if draft_on { "Starts when you press Apply." } else { "Stops when you press Apply." }.into();
    }
    if !draft_on {
        return "Off.".into();
    }
    let Some(report) = view.report.as_ref() else { return "Opens with the input, when a track is armed or monitored.".into() };
    match report.state {
        "running" => {
            let mut line = format!("Capturing {}", report.text);
            if report.rate != 0 && report.rate != report.device_rate {
                line.push_str(&format!(" · {} Hz to {} Hz", report.rate, report.device_rate));
            } else if report.rate != 0 {
                line.push_str(&format!(" · {} Hz", report.rate));
            }
            if let Some(ms) = report.latency_ms {
                line.push_str(&format!(" · latency {ms:.1} ms"));
            }
            line.push_str(&format!(" · drift {:+.1} ppm", report.drift_ppm));
            if report.underrun_frames + report.overflow_frames > 0 {
                line.push_str(&format!(" · {} frames silent, {} dropped", report.underrun_frames, report.overflow_frames));
            }
            line
        }
        "refused" => format!("Not capturing: {}", report.text),
        "pending" => "Starting: the capture opens once the output runs.".into(),
        "stopped" => report.text.clone(),
        _ => "Off: the input opened before System Audio was switched on; press Reconnect.".into(),
    }
}

/// The Input list's label of a device: a Linux monitor source as System Audio, else as it is.
pub fn input_label(d: &DeviceInfo) -> String {
    match system_audio::monitor_label(&d.name, &[]) {
        Some(label) if d.max_channels > 0 => format!("{label} · {} ch", d.max_channels),
        Some(label) => label,
        None => crate::audio_devices::device_label(d),
    }
}

/// Draw the block: the switch, the output picker, the status line, the meter and the rights note.
/// `meter` is the shown level (it falls back slowly between peaks).
pub fn section(
    ui: &mut egui::Ui,
    draft: &mut AudioConfig,
    view: &View,
    meter: &mut [f32; 2],
    (controls, entries): (&mut Vec<MenuItem>, &mut Vec<MenuItem>),
) {
    ui.add_space(8.0);
    let title = ui.label(egui::RichText::new(system_audio::PATH_NAME).strong());
    crate::help::hover(ui, &title, crate::help::Id::AudioDevicesSystemAudio);
    if !view.offered {
        let r = ui.add(egui::Label::new(egui::RichText::new(ELSEWHERE_NOTE).color(TOKENS.text_dim).size(11.5)).wrap());
        controls.push(MenuItem::at(ui, ELSEWHERE_NOTE.into(), "system_audio_note".into(), r.rect));
        return;
    }
    let switch = crate::chrome::checkbox(ui, &mut draft.system_audio, "Record what the computer plays");
    crate::help::hover(ui, &switch, crate::help::Id::AudioDevicesSystemAudio);
    controls.push(MenuItem::at(ui, "Record what the computer plays".into(), "system_audio".into(), switch.rect));
    ui.horizontal(|ui| {
        ui.label("Output to capture");
        let options: Vec<(String, String)> = view.outputs.iter().map(|o| (o.clone(), o.clone())).collect();
        let (rect, text) = crate::audio_devices::window::choose_or(
            ui,
            "system_audio_output",
            draft.system_audio,
            &mut draft.system_audio_output,
            &options,
            entries,
            DEFAULT_OUTPUT,
        );
        controls.push(MenuItem::at(ui, text, "system_audio_output".into(), rect));
    });
    let line = status_line(view, draft.system_audio);
    let r = ui.add(egui::Label::new(egui::RichText::new(&line).color(TOKENS.text_dim).size(11.5)).wrap());
    controls.push(MenuItem::at(ui, line, "system_audio_status".into(), r.rect));
    if let Some(report) = view.report.as_ref().filter(|r| r.state == "running") {
        for (shown, peak) in meter.iter_mut().zip(report.peak) {
            *shown = peak.max(*shown * 0.85).clamp(0.0, 1.0);
        }
        ui.horizontal(|ui| {
            ui.label("Level");
            for (side, level) in meter.iter().enumerate() {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 6.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 1.0, TOKENS.lane);
                let lit = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * level, rect.height()));
                ui.painter().rect_filled(lit, 1.0, if *level >= 0.99 { TOKENS.record } else { TOKENS.accent });
                controls.push(MenuItem::at(
                    ui,
                    format!("{level:.2}"),
                    if side == 0 { "system_audio_level_l" } else { "system_audio_level_r" }.into(),
                    rect,
                ));
            }
        });
    }
    let r = ui.add(egui::Label::new(egui::RichText::new(SWITCH_NOTE).color(TOKENS.text_dim).size(11.0)).wrap());
    controls.push(MenuItem::at(ui, SWITCH_NOTE.into(), "system_audio_note".into(), r.rect));
}

impl crate::StudioApp {
    /// Whether tracks are offered the System Audio input: this build captures it and it is on.
    pub(crate) fn system_audio_on(&self) -> bool {
        system_audio::offered() && self.ui.audio.system_audio
    }

    /// What the System Audio block shows this frame. The outputs are listed once per listing (the
    /// fake devices' WASAPI outputs under the control channel and tests).
    pub(crate) fn system_audio_view(&mut self) -> View {
        let offered = system_audio::offered();
        if offered && self.audio_devices.system_outputs.is_none() {
            let listed = match &self.audio_devices.fake {
                Some(fake) => fake.systems.iter().filter(|s| s.name == "WASAPI").flat_map(|s| s.outputs.iter().map(|d| d.name.clone())).collect(),
                // Never under `--no-audio` (#363), never the machine's in a test build.
                None if self.may_list_machine() && !cfg!(test) => system_audio::outputs(),
                None => Vec::new(),
            };
            self.audio_devices.system_outputs = Some(listed);
        }
        let report = match &self.recorder {
            Some(r) => Some(r.system_audio_report()),
            None => self.player.as_ref().and_then(|p| p.with_held_input(transit_playback::record::Recorder::system_audio_report)),
        };
        View { offered, outputs: self.audio_devices.system_outputs.clone().unwrap_or_default(), report, running_on: self.ui.audio.system_audio }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> Report {
        Report {
            state: "running",
            text: "Speakers (Realtek)".into(),
            rate: 44_100,
            device_rate: 48_000,
            latency_ms: Some(40.0),
            drift_ppm: 12.34,
            ..Report::default()
        }
    }

    #[test]
    fn the_status_line_says_what_runs_and_what_waits_for_apply() {
        let view = |report: Option<Report>, running_on| View { offered: true, outputs: Vec::new(), report, running_on };
        assert_eq!(status_line(&view(None, false), false), "Off.");
        assert_eq!(status_line(&view(None, false), true), "Starts when you press Apply.");
        assert_eq!(status_line(&view(None, true), false), "Stops when you press Apply.");
        assert_eq!(status_line(&view(None, true), true), "Opens with the input, when a track is armed or monitored.");
        assert_eq!(
            status_line(&view(Some(running()), true), true),
            "Capturing Speakers (Realtek) · 44100 Hz to 48000 Hz · latency 40.0 ms · drift +12.3 ppm"
        );
        let refused =
            Report { state: "refused", text: "Windows refuses to capture Speakers while it is open in exclusive mode".into(), ..Report::default() };
        assert_eq!(
            status_line(&view(Some(refused), true), true),
            "Not capturing: Windows refuses to capture Speakers while it is open in exclusive mode"
        );
        let lossy = Report { underrun_frames: 120, overflow_frames: 3, rate: 48_000, ..running() };
        assert!(
            status_line(&view(Some(lossy), true), true).ends_with("· 48000 Hz · latency 40.0 ms · drift +12.3 ppm · 120 frames silent, 3 dropped")
        );
    }

    #[test]
    fn linux_monitor_inputs_are_listed_as_system_audio() {
        let device = |name: &str, max_channels| DeviceInfo {
            name: name.into(),
            is_default: false,
            max_channels,
            sample_rates: Vec::new(),
            buffer_min: None,
            buffer_max: None,
        };
        assert_eq!(input_label(&device("Monitor of Built-in Audio", 2)), "System Audio (monitor of Built-in Audio) · 2 ch");
        assert_eq!(input_label(&device("Scarlett 2i2 USB", 2)), "Scarlett 2i2 USB · 2 ch");
    }
}
