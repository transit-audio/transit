//! The External Instrument in the device chain (ADR 0020): picked from the instrument menu, it is
//! the instrument card of a track that plays a hardware synth. The card chooses the return input
//! path, its gain and the latency set by hand, measures the latency (slice 2,
//! [`crate::external_measure`]), and its bottom line says what the device is doing and where its
//! latency comes from (decisions 4 and 7). The MIDI port and channel stay on the mixer strip's Out
//! button (#61). Every change is one `instrument.external` command, one undo step.

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
/// label when missing, or an empty string when none is chosen), and whether it is amber (a
/// measurement of another rate or buffer in use). `rate` converts frames to ms and `buffer` is
/// the open device's, against which a measurement is "measure again".
#[must_use]
pub fn state_line(report: Option<&ExternalReport>, live: bool, port: Option<&str>, rate: u32, buffer: Option<u32>) -> (String, bool) {
    if !live {
        return ("No audio device".into(), false);
    }
    if let Some(r) = report
        && r.state == ExternalState::Ready
        && port.is_none()
        && let (Some(frames), Some(source)) = (r.latency, r.source.as_ref())
    {
        return crate::external_measure::latency_text(frames, source, rate, buffer);
    }
    (plain_state(report, port, rate), false)
}

fn plain_state(report: Option<&ExternalReport>, port: Option<&str>, rate: u32) -> String {
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
        // The table as the plan reads it: with the buffer the open device runs, which picks the
        // measurement of this key (ADR 0020 decision 5).
        let mut io = self.engine.io_map().clone();
        io.buffer = self.device_buffer();
        #[cfg(test)]
        let duplex = self.external_measure.fake.is_some() || self.player.as_ref().is_some_and(|p| p.duplex());
        #[cfg(not(test))]
        let duplex = self.player.as_ref().is_some_and(|p| p.duplex());
        let view = crate::midi_output::OutputView::read(self.player.as_ref());
        let s = self.engine.session();
        let missing: Vec<TrackId> = s.tracks.iter().filter(|t| view.missing(t.id, &t.midi_output)).map(|t| t.id).collect();
        soundcraft_mix::external_report(s, &io, io.inputs, duplex, s.sample_rate.hz(), &missing)
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

/// The card's width and height: it asks for the rows below and never less than the width at which
/// the Latency field and the Measure button sit side by side (the Devices row follows the tallest
/// card, issue #96).
pub(crate) const CARD_W: f32 = 272.0;
pub(crate) const CARD_H: f32 = 171.0;
/// The tooltip of Measure while the port or the return is not chosen.
pub(crate) const CHOOSE_FIRST: &str = "Choose a MIDI Out port and a Return first";

/// Where each widget of the card sits: Out, Return, Latency (label, field, Measure), the result
/// line under it (with Keep at its right end), Gain and the status row last. No two overlap, at
/// any card width from [`CARD_W`] up.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rows {
    pub out: Rect,
    pub ret: Rect,
    pub latency_label: Rect,
    pub latency: Rect,
    pub measure: Rect,
    pub result: Rect,
    pub keep: Rect,
    pub gain_label: Rect,
    pub gain: Rect,
    pub status: Rect,
}

impl Rows {
    /// The rows of the card in `r`, whose title ends at `top`.
    pub(crate) fn of(r: Rect, top: f32) -> Self {
        const LABEL: f32 = 48.0;
        const MEASURE: f32 = 100.0;
        const KEEP: f32 = 44.0;
        let (x0, x1) = (r.min.x + 8.0, r.max.x - 8.0);
        let y = |dy: f32, h: f32| (top + 3.0 + dy, top + 3.0 + dy + h);
        let band = |(y0, y1): (f32, f32), a: f32, b: f32| Rect::from_min_max(pos2(a, y0), pos2(b, y1));
        let (lat, res, gain) = (y(40.0, 18.0), y(60.0, 14.0), y(78.0, 18.0));
        Self {
            out: band(y(0.0, 18.0), x0, x1),
            ret: band(y(20.0, 18.0), x0, x1),
            latency_label: band(lat, x0, x0 + LABEL),
            latency: band(lat, x0 + LABEL, x1 - MEASURE - 4.0),
            measure: band(lat, x1 - MEASURE, x1),
            result: band(res, x0, x1 - KEEP - 4.0),
            keep: band(res, x1 - KEEP, x1),
            gain_label: band(gain, x0, x0 + LABEL),
            gain: band(gain, x0 + LABEL, x1),
            status: band(y(100.0, 40.0), x0, x1),
        }
    }

    #[cfg(test)]
    pub(crate) fn all(&self) -> [Rect; 10] {
        [self.out, self.ret, self.latency_label, self.latency, self.measure, self.result, self.keep, self.gain_label, self.gain, self.status]
    }
}

/// The body of an External Instrument's card under its title (rows in [`Rows`]): the MIDI Out
/// selector, the return dropdown, the latency field with Measure, the result line, the gain, and
/// the status row (a refusal or a notice; nothing else is ever drawn there).
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
    menu: &mut Vec<MenuItem>,
    c: &mut DeviceCard,
) {
    // Nothing of the card is painted outside it.
    let clip = r.intersect(lane);
    let pc = &pc.with_clip_rect(clip);
    let rows = Rows::of(r, top);
    let font = FontId::proportional(10.5);
    let mut change: Option<Value> = None;
    let s = app.engine.session_arc();
    let output = s.track(t).map_or(MidiOutput::Internal, |x| x.midi_output.clone());
    let view = crate::midi_output::OutputView::read(app.player.as_ref());
    let say = |text: &str, rect: Rect, color: egui::Color32| {
        pc.with_clip_rect(rect.intersect(clip)).text(pos2(rect.min.x, rect.center().y), Align2::LEFT_CENTER, text, font.clone(), color);
    };
    // Out: the track's MIDI port and channel, the same setting and the same popup as the chip on
    // its header (`arrange::midi_output_chip`).
    let rb = rows.out;
    let resp = ui.interact(rb.intersect(clip), ui.id().with(("ext_out", t.0)), Sense::click());
    pc.rect_filled(rb, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    let chip = view.header_chip(t, &output);
    let label = match &chip {
        Some(chip) => format!("Out: {}", chip.text),
        None => "Out: None".to_string(),
    };
    let ink = if chip.as_ref().is_some_and(|c| c.missing) { TOKENS.record } else { TOKENS.text };
    pc.with_clip_rect(rb.shrink2(vec2(6.0, 0.0)).intersect(clip)).text(
        pos2(rb.min.x + 6.0, rb.center().y),
        Align2::LEFT_CENTER,
        &label,
        font.clone(),
        ink,
    );
    crate::icons::paint(pc, pos2(rb.max.x - 10.0, rb.center().y), crate::icons::Icon::ChevronDown, 12.0, TOKENS.text_dim);
    c.external_out = crate::arrange::r4(rb);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "MIDI output"));
    if let Some(params) = view.popup(&resp, t, &output, true) {
        let _ = app.run("midi.output", params);
    }
    // Return: a dropdown of the machine's input paths.
    let rb = rows.ret;
    let resp = ui.interact(rb.intersect(clip), ui.id().with(("ext_return", t.0)), Sense::click());
    pc.rect_filled(rb, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    let label = if e.ret.is_empty() { "Return: None".to_string() } else { format!("Return: {}", e.ret) };
    pc.with_clip_rect(rb.shrink2(vec2(6.0, 0.0)).intersect(clip)).text(
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
    // Latency set by hand, in ms; empty clears it. Measure sits beside it.
    say("Latency", rows.latency_label, TOKENS.text_dim);
    let field = rows.latency;
    let id = ui.id().with(("ext_latency", t.0));
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| manual_text(e));
    let l = ui.put(field.intersect(clip), egui::TextEdit::singleline(&mut text).hint_text("Set manually\u{2026} ms").font(font.clone()));
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
    let set = matches!(output, MidiOutput::External { .. }) && !e.ret.is_empty();
    measure_button(app, ui, pc, clip, t, &rows, &font, set, c);
    // Gain on the return: edited locally, one command when the drag or typing ends.
    say("Gain", rows.gain_label, TOKENS.text_dim);
    let field = rows.gain;
    let id = ui.id().with(("ext_gain", t.0));
    let mut gain = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or_else(|| e.gain_db());
    let g = ui.put(
        field.intersect(clip),
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
    // The state: a result line under Latency, or a refusal or notice in the status row.
    let live = app.external_live();
    let port = match &output {
        MidiOutput::Internal => Some(String::new()),
        out if view.missing(t, out) => Some(view.port_label(out)),
        _ => None,
    };
    let report = app.external_reports().into_iter().find(|x| x.track == t);
    let (state, stale) = state_line(report.as_ref(), live, port.as_deref(), s.sample_rate.hz(), app.device_buffer());
    let ok = report.as_ref().is_some_and(|x| x.state == ExternalState::Ready) && port.is_none() && live;
    // A refusal or a result waiting for Keep is said where the state is, until the next Measure.
    let said = app.external_measure.outcome.as_ref().filter(|o| o.track == t && !o.stored).map(|o| o.text.clone());
    let running = app.external_measure.running.as_ref().is_some_and(|x| x.track == t);
    let (line, ink) = match &said {
        Some(text) => (text.clone(), TOKENS.solo),
        None if stale => (state.clone(), TOKENS.solo),
        None => (state.clone(), if ok { TOKENS.text } else { TOKENS.text_dim }),
    };
    // Where the latency comes from (manual or measured) is the result; the rest is status.
    let is_result = said.is_none() && ok;
    let result = if running {
        c.external_measure_text.clone()
    } else if is_result {
        line.clone()
    } else {
        String::new()
    };
    c.external_result = crate::arrange::r4(rows.result);
    say(&result, rows.result, if running { TOKENS.text } else { ink });
    if !running && app.external_measure.outcome.as_ref().is_some_and(|o| o.track == t && o.keep.is_some()) {
        let resp = ui.interact(rows.keep.intersect(clip), ui.id().with(("ext_keep", t.0)), Sense::click());
        pc.rect_filled(rows.keep, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        pc.with_clip_rect(rows.keep.intersect(clip)).text(rows.keep.center(), Align2::CENTER_CENTER, "Keep", font.clone(), TOKENS.text);
        c.external_keep = crate::arrange::r4(rows.keep);
        if resp.on_hover_text("Store this measurement although its spread is high").clicked() {
            let _ = app.keep_external_measure(t);
        }
    }
    let status = if is_result { String::new() } else { line };
    let area = rows.status;
    c.external_status = crate::arrange::r4(area);
    let resp = ui.interact(area.intersect(clip), ui.id().with(("ext_state", t.0)), Sense::hover());
    pc.rect_filled(area, crate::theme::RADIUS, TOKENS.button);
    if !status.is_empty() {
        let galley = pc.layout(status, FontId::proportional(10.0), ink, area.width() - 10.0);
        pc.with_clip_rect(area.intersect(clip)).galley(
            pos2(area.center().x - galley.size().x / 2.0, area.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
    resp.on_hover_text(format!("{}\n{state}", view.label(t, &output)));
    c.external_state = said.unwrap_or(state);
    c.external_stale = stale;
    if let Some(change) = change {
        let _ = app.edit_external_instrument(t, change);
    }
}

impl StudioApp {
    /// An audio device is open (a test's fake Measure counts as one).
    pub(crate) fn external_live(&self) -> bool {
        #[cfg(test)]
        if self.external_measure.fake.is_some() {
            return true;
        }
        self.player.as_ref().is_some_and(|p| !p.silent)
    }
}

/// The card's Measure button beside the latency field: Measure (off with the reason as its tooltip
/// while it cannot run, "Choose a MIDI Out port and a Return first" while either is unset), or
/// Cancel while it runs (its progress is the result line).
#[allow(clippy::too_many_arguments)]
fn measure_button(
    app: &mut StudioApp,
    ui: &mut Ui,
    pc: &egui::Painter,
    clip: Rect,
    t: TrackId,
    rows: &Rows,
    font: &FontId,
    set: bool,
    c: &mut DeviceCard,
) {
    let running = app.external_measure.running.as_ref().is_some_and(|r| r.track == t);
    let button = |ui: &mut Ui, r: Rect, salt: &str, label: &str, enabled: bool| {
        let resp = ui.interact(r.intersect(clip), ui.id().with((salt, t.0)), Sense::click());
        let fill = if resp.hovered() && enabled { TOKENS.button_hover } else { TOKENS.button };
        pc.rect_filled(r, crate::theme::RADIUS, fill);
        let ink = if enabled { TOKENS.text } else { TOKENS.text_dim.gamma_multiply(0.6) };
        pc.with_clip_rect(r.intersect(clip)).text(r.center(), Align2::CENTER_CENTER, label, font.clone(), ink);
        resp
    };
    let rb = rows.measure;
    if running {
        let progress = app.external_measure_progress().map(crate::external_measure::progress_text).unwrap_or_default();
        let resp = button(ui, rb, "ext_cancel", "Cancel", true).on_hover_text("Stop measuring: the note is released and nothing is stored");
        c.external_measure = crate::arrange::r4(rb);
        c.external_measure_text = progress;
        if resp.clicked() {
            app.cancel_external_measure(crate::external_measure::CANCELLED);
        }
        return;
    }
    let blocked = if set { app.external_measure_blocker(t) } else { Some(CHOOSE_FIRST.to_string()) };
    let label = if app.external_measure.outcome.as_ref().is_some_and(|o| o.track == t) { "Measure again" } else { "Measure" };
    let resp = button(ui, rb, "ext_measure", label, blocked.is_none());
    let resp =
        resp.on_hover_text(blocked.clone().unwrap_or_else(|| {
            "Play a warm-up note and 8 notes (C4, velocity 100) to the synth and time their sound on the return: about 13 s".into()
        }));
    c.external_measure = crate::arrange::r4(rb);
    c.external_measure_text = label.into();
    c.external_measure_blocked = blocked.clone().unwrap_or_default();
    if resp.clicked() {
        match blocked {
            Some(why) => app.status = why,
            None => {
                if let Err(why) = app.start_external_measure(t) {
                    app.status = why;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_text(report: Option<&ExternalReport>, live: bool, port: Option<&str>, rate: u32, buffer: Option<u32>) -> String {
        state_line(report, live, port, rate, buffer).0
    }

    fn report(state: ExternalState, latency: Option<u32>) -> ExternalReport {
        let source = latency.map(|_| soundcraft_model::ExternalSource::Manual);
        ExternalReport { track: TrackId(1), ret: "In 1".into(), state, latency, source }
    }

    #[test]
    fn the_state_line_names_each_missing_part_first_the_device_then_the_port_then_the_return() {
        let ready = report(ExternalState::Ready, Some(936));
        assert_eq!(state_text(Some(&ready), true, None, 48_000, Some(64)), "Manual: 19.5 ms");
        assert_eq!(state_text(Some(&ready), false, None, 48_000, Some(64)), "No audio device");
        assert_eq!(state_text(Some(&ready), true, Some("MODEL D"), 48_000, Some(64)), "MODEL D missing: notes are not sent");
        assert_eq!(state_text(Some(&ready), true, Some(""), 48_000, Some(64)), "No MIDI port: choose one on the Out button");
        assert_eq!(
            state_text(Some(&report(ExternalState::MissingReturn, None)), true, None, 48_000, Some(64)),
            "Return In 1 is not on this interface"
        );
        assert_eq!(state_text(Some(&report(ExternalState::NoInput, None)), true, None, 48_000, Some(64)), "Needs a duplex device");
        assert_eq!(
            state_text(Some(&report(ExternalState::Unmeasured, None)), true, None, 48_000, Some(64)),
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
