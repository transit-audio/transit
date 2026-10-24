//! The External Instrument in the device chain (ADR 0020, slice 1): picked from the instrument
//! menu, it is the instrument card of a track that plays a hardware synth. The card chooses the
//! return input path, its gain and the latency set by hand, and its bottom line says what the
//! device is doing (decision 7). The MIDI port and channel stay on the mixer strip's Out button
//! (#61). Every change is one `instrument.external` command, one undo step.

use crate::StudioApp;
use crate::mixer::{DeviceCard, MenuItem};
use crate::theme::TOKENS;
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_mix::{ExternalReport, ExternalState};
use soundcraft_model::{EXTERNAL_GAIN_DB, ExternalInstrument, IoDirection, MAX_EXTERNAL_LATENCY_MS, MidiOutput, TrackId};

/// What the instrument menu's entry reads.
pub const MENU_LABEL: &str = "External Instrument";

/// What the device card's state line says for `report` (the plan's view of the device),
/// `live` (an audio device is open), and `port` (`None`: a port is chosen and present; else its
/// label when missing, or an empty string when none is chosen). `rate` converts frames to ms.
#[must_use]
pub fn state_text(report: Option<&ExternalReport>, live: bool, port: Option<&str>, rate: u32) -> String {
    if !live {
        return "No audio device".into();
    }
    match port {
        Some("") => return "No MIDI port: choose one on the Out button".into(),
        Some(name) => return format!("{name} missing: notes are not sent"),
        None => {}
    }
    let Some(r) = report else { return String::new() };
    match r.state {
        ExternalState::NoReturn => "Choose the return input".into(),
        ExternalState::MissingReturn => format!("Return {} is not on this interface", r.ret),
        ExternalState::NoInput => "Needs a duplex device".into(),
        ExternalState::Unmeasured => "Not measured: notes sound late by the round trip".into(),
        // The port state comes from the output thread, not the plan.
        ExternalState::NoPort => "No MIDI port: choose one on the Out button".into(),
        ExternalState::Ready => r.latency.map_or_else(String::new, |f| format!("Manual: {:.1} ms", crate::hardware_ui::ms(f, rate))),
    }
}

/// The text of the manual latency field for `e`: the value in ms, empty when unset.
#[must_use]
pub fn manual_text(e: &ExternalInstrument) -> String {
    e.manual_ms().map_or_else(String::new, |ms| format!("{ms:.1}"))
}

/// The `manual_ms` a typed field means: empty clears it, a number in range sets it; `None` for text
/// that is neither (nothing is sent).
#[must_use]
pub fn parse_manual(text: &str) -> Option<Value> {
    let t = text.trim().trim_end_matches("ms").trim();
    if t.is_empty() {
        return Some(Value::Null);
    }
    t.parse::<f32>().ok().filter(|ms| ms.is_finite() && (0.0..=MAX_EXTERNAL_LATENCY_MS).contains(ms)).map(|ms| json!(ms))
}

impl StudioApp {
    /// Every External Instrument of the session as this machine and device see it, with the ports
    /// the MIDI output thread reports missing.
    pub fn external_reports(&self) -> Vec<ExternalReport> {
        let io = self.engine.io_map();
        let duplex = self.player.as_ref().is_some_and(|p| p.duplex());
        let view = crate::midi_output::OutputView::read(self.player.as_ref());
        let s = self.engine.session();
        let missing: Vec<TrackId> = s.tracks.iter().filter(|t| view.missing(t.id, &t.midi_output)).map(|t| t.id).collect();
        soundcraft_mix::external_report(s, io, io.inputs, duplex, s.sample_rate.hz(), &missing)
    }

    /// Put an External Instrument in `t`'s instrument slot, returning on the machine's first input
    /// path. The port and channel the track already sends to stay.
    pub fn set_external_instrument(&mut self, t: TrackId) -> Result<Value, String> {
        if self.engine.session().track(t).is_some_and(|tr| tr.external_instrument().is_some()) {
            return Ok(json!({"unchanged": true}));
        }
        let key = (t, soundcraft_mix::INSTRUMENT_SLOT);
        if let Some(p) = &self.player {
            self.player_requests.result(&mut self.status, p.close_editor(key.0, key.1)).ok_or_else(|| self.status.clone())?;
        }
        self.ui.plugin_windows.retain(|k| *k != key);
        self.editors.retain(|e| e.key != key);
        let ret = self.engine.io_map().paths_of(IoDirection::Input).next().map(|p| p.name.clone()).unwrap_or_default();
        self.run("instrument.external", json!({"track": t.0, "return": ret}))
    }

    /// Change one or more of the device's fields (`return`, `gain_db`, `manual_ms`).
    pub fn edit_external_instrument(&mut self, t: TrackId, mut change: Value) -> Result<Value, String> {
        change["track"] = json!(t.0);
        self.run("instrument.external", change)
    }
}

/// The body of an External Instrument's card under its title: the return dropdown, the gain, the
/// manual latency, and the state line in the place of the editor button.
#[allow(clippy::too_many_arguments)]
pub(crate) fn card(
    app: &mut StudioApp,
    ui: &mut Ui,
    pc: &egui::Painter,
    lane: Rect,
    t: TrackId,
    e: &ExternalInstrument,
    r: Rect,
    top: f32,
    eb: Rect,
    menu: &mut Vec<MenuItem>,
    c: &mut DeviceCard,
) {
    let row = |i: f32| Rect::from_min_size(pos2(r.min.x + 8.0, top + 3.0 + i * 20.0), vec2(r.width() - 16.0, 18.0));
    let font = FontId::proportional(10.5);
    let mut change: Option<Value> = None;
    // Return: a dropdown of the machine's input paths.
    let rb = row(0.0);
    let resp = ui.interact(rb.intersect(lane), ui.id().with(("ext_return", t.0)), Sense::click());
    pc.rect_filled(rb, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    let label = if e.ret.is_empty() { "Return: None".to_string() } else { format!("Return: {}", e.ret) };
    pc.with_clip_rect(rb.shrink2(vec2(6.0, 0.0)).intersect(lane)).text(
        pos2(rb.min.x + 6.0, rb.center().y),
        Align2::LEFT_CENTER,
        &label,
        font.clone(),
        TOKENS.text,
    );
    crate::icons::paint(pc, pos2(rb.max.x - 10.0, rb.center().y), crate::icons::Icon::ChevronDown, 12.0, TOKENS.text_dim);
    c.external_return = crate::arrange::r4(rb);
    let resp = resp.on_hover_text("The input the synth's audio comes back on");
    let inputs: Vec<String> = app.engine.io_map().paths_of(IoDirection::Input).map(|p| p.name.clone()).collect();
    egui::Popup::menu(&resp).width(180.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        for name in &inputs {
            let b = crate::menus::check_button(ui, e.ret.eq_ignore_ascii_case(name), name);
            menu.push(MenuItem::at(ui, name.clone(), format!("return:{name}"), b.rect));
            if b.clicked() {
                change = Some(json!({"return": name}));
                ui.close();
            }
        }
        if inputs.is_empty() {
            ui.weak("This device has no input paths (Audio Devices \u{203a} I/O paths)");
        }
    });
    // Gain on the return: edited locally, one command when the drag or typing ends.
    let gb = row(1.0);
    pc.text(pos2(gb.min.x, gb.center().y), Align2::LEFT_CENTER, "Gain", font.clone(), TOKENS.text_dim);
    let field = Rect::from_min_max(pos2(gb.min.x + 52.0, gb.min.y), gb.max);
    let id = ui.id().with(("ext_gain", t.0));
    let mut gain = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or_else(|| e.gain_db());
    let g = ui.put(
        field.intersect(lane),
        egui::DragValue::new(&mut gain).range(EXTERNAL_GAIN_DB.0..=EXTERNAL_GAIN_DB.1).speed(0.1).fixed_decimals(1).suffix(" dB"),
    );
    if g.changed() {
        ui.data_mut(|d| d.insert_temp(id, gain));
    }
    if (g.drag_stopped() || g.lost_focus()) && ui.data(|d| d.get_temp::<f32>(id)).is_some() {
        ui.data_mut(|d| d.remove::<f32>(id));
        if (gain - e.gain_db()).abs() > f32::EPSILON {
            change = Some(json!({"gain_db": gain}));
        }
    }
    g.on_hover_text("Gain on the return, before the inserts");
    c.external_gain = crate::arrange::r4(field);
    // Latency set by hand, in ms; empty clears it.
    let lb = row(2.0);
    pc.text(pos2(lb.min.x, lb.center().y), Align2::LEFT_CENTER, "Latency", font.clone(), TOKENS.text_dim);
    let field = Rect::from_min_max(pos2(lb.min.x + 52.0, lb.min.y), lb.max);
    let id = ui.id().with(("ext_latency", t.0));
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| manual_text(e));
    let l = ui.put(field.intersect(lane), egui::TextEdit::singleline(&mut text).hint_text("Set manually… ms").font(font.clone()));
    if l.changed() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    }
    if l.lost_focus() && ui.data(|d| d.get_temp::<String>(id)).is_some() {
        ui.data_mut(|d| d.remove::<String>(id));
        match parse_manual(&text) {
            Some(v) if v.as_f64().map(|x| x as f32) != e.manual_ms() => change = Some(json!({"manual_ms": v})),
            Some(_) => {}
            None => app.status = format!("Latency must be a number of ms in 0..{MAX_EXTERNAL_LATENCY_MS}, or empty"),
        }
    }
    l.on_hover_text("The synth's latency in ms: from the note to its sound on the return. Notes are not sent early; the rest of the mix waits.");
    c.external_latency = crate::arrange::r4(field);
    // The state line.
    let live = app.player.as_ref().is_some_and(|p| !p.silent);
    let s = app.engine.session_arc();
    let output = s.track(t).map_or(MidiOutput::Internal, |x| x.midi_output.clone());
    let view = crate::midi_output::OutputView::read(app.player.as_ref());
    let port = match &output {
        MidiOutput::Internal => Some(String::new()),
        out if view.missing(t, out) => Some(view.port_label(out)),
        _ => None,
    };
    let report = app.external_reports().into_iter().find(|x| x.track == t);
    let state = state_text(report.as_ref(), live, port.as_deref(), s.sample_rate.hz());
    let ok = report.as_ref().is_some_and(|x| x.state == ExternalState::Ready) && port.is_none() && live;
    let resp = ui.interact(eb.intersect(lane), ui.id().with(("ext_state", t.0)), Sense::hover());
    pc.rect_filled(eb, crate::theme::RADIUS, TOKENS.button);
    pc.with_clip_rect(eb.intersect(lane)).text(eb.center(), Align2::CENTER_CENTER, &state, font, if ok { TOKENS.text } else { TOKENS.text_dim });
    resp.on_hover_text(format!("{}\n{state}", view.label(t, &output)));
    c.external_state = state;
    if let Some(change) = change {
        let _ = app.edit_external_instrument(t, change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: ExternalState, latency: Option<u32>) -> ExternalReport {
        ExternalReport { track: TrackId(1), ret: "In 1".into(), state, latency }
    }

    #[test]
    fn the_state_line_names_each_missing_part_first_the_device_then_the_port_then_the_return() {
        let ready = report(ExternalState::Ready, Some(936));
        assert_eq!(state_text(Some(&ready), true, None, 48_000), "Manual: 19.5 ms");
        assert_eq!(state_text(Some(&ready), false, None, 48_000), "No audio device");
        assert_eq!(state_text(Some(&ready), true, Some("MODEL D"), 48_000), "MODEL D missing: notes are not sent");
        assert_eq!(state_text(Some(&ready), true, Some(""), 48_000), "No MIDI port: choose one on the Out button");
        assert_eq!(state_text(Some(&report(ExternalState::MissingReturn, None)), true, None, 48_000), "Return In 1 is not on this interface");
        assert_eq!(state_text(Some(&report(ExternalState::NoInput, None)), true, None, 48_000), "Needs a duplex device");
        assert_eq!(
            state_text(Some(&report(ExternalState::Unmeasured, None)), true, None, 48_000),
            "Not measured: notes sound late by the round trip"
        );
    }

    #[test]
    fn the_latency_field_clears_on_empty_and_refuses_what_is_not_a_latency() {
        assert_eq!(parse_manual(""), Some(Value::Null));
        assert_eq!(parse_manual(" 19.5 ms"), Some(json!(19.5)));
        assert_eq!(parse_manual("20"), Some(json!(20.0)));
        assert_eq!(parse_manual("600"), None);
        assert_eq!(parse_manual("-1"), None);
        assert_eq!(parse_manual("abc"), None);
        let e = ExternalInstrument { ret: "In 1".into(), gain_db: 0.0, manual_ms: Some(19.5) };
        assert_eq!(manual_text(&e), "19.5");
        assert_eq!(manual_text(&ExternalInstrument::default()), "");
    }
}
