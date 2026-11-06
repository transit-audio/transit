//! The Tuner insert's panel (#266), laid out like Ableton's Tuner: the note large, the cents meter
//! under it, the frequency small, then the Reference knob and Hold.
//!
//! The pitch estimate is made here, on the UI thread, from the insert's sample tap
//! (`transit_mix::SampleTap`, switched on while this window is open): at most every
//! [`UPDATE_S`] the newest window is read and handed to [`transit_dsp::plugins::tuner::TunerModel`].
//! The audio thread only copies samples into the ring.

use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::help::{Id, hover};
use crate::panel::Item::{Knob, Toggle};
use crate::panel::{Row, Section};
use crate::theme::{RADIUS_SM, TOKENS};
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui, vec2};
use transit_dsp::plugins::tuner::{TunerModel, signed_tenths};
use transit_model::{Insert, TrackId};

/// The Tuner's id (#266).
pub const TUNER: &str = "tuner";

/// Seconds between analyses (the brief's 40 ms: 25 readings a second).
const UPDATE_S: f64 = 0.040;
/// Width of the display, and the heights of its note area and meter.
pub const W: f32 = 300.0;
const NOTE_H: f32 = 64.0;
const METER_H: f32 = 28.0;
const FREQ_H: f32 = 18.0;
/// The meter spans -50 to +50 cents; within this many cents it reads in tune.
const SPAN_CENTS: f32 = 50.0;
const IN_TUNE_CENTS: f32 = 3.0;

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 1] = [&[sec("Tuning", 2, &[Knob("reference_hz", "Reference"), Toggle("hold", "Hold")])]];

/// The help entry of a Tuner control.
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "reference_hz" => Id::TunerReference,
        "hold" => Id::TunerHold,
        _ => return None,
    })
}

/// What the display showed as last drawn, for `ui.inspect`-style checks and tests.
#[derive(Debug, Clone, Default)]
pub struct TunerLayout {
    /// The note with its octave ("A4"), or "--" with no signal.
    pub note: String,
    /// The cents offset ("+0.0"), empty with no signal.
    pub cents: String,
    /// The frequency ("440.0 Hz"), or "No signal".
    pub freq: String,
    /// The reading as one line ("A4 +0.0 cents"), or "no signal".
    pub text: String,
    pub note_rect: [f32; 4],
    pub meter: [f32; 4],
    pub freq_rect: [f32; 4],
    /// The needle's x position, `None` with no signal.
    pub needle_x: Option<f32>,
    /// The meter reads in tune (within the green zone).
    pub in_tune: bool,
}

/// A Tuner's model, when it last analyzed and how many frames its tap had received then.
pub struct TunerState {
    model: TunerModel,
    at: f64,
    written: Option<usize>,
}

impl std::fmt::Debug for TunerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunerState").field("reading", &self.model.reading()).finish_non_exhaustive()
    }
}

fn r4(r: Rect) -> [f32; 4] {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

fn value(app: &StudioApp, key: (TrackId, usize), ins: &Insert, id: &str) -> f32 {
    let Some(p) = transit_dsp::plugin_info(TUNER).and_then(|i| i.param(id)) else { return 0.0 };
    p.clamp(app.followed_param(key, id).or_else(|| ins.params.get(id).copied()).unwrap_or(p.default))
}

/// Reads the tap and analyzes when [`UPDATE_S`] has passed (the model keeps the last reading between).
fn update(app: &mut StudioApp, ui: &Ui, key: (TrackId, usize), uid: u64, reference_hz: f32, hold: bool) {
    let rate = app.engine.session().sample_rate.as_f64() as f32;
    let hub = app.scope_hub();
    let now = ui.input(|i| i.time);
    let st =
        app.builtin_editor.tuners.entry(uid).or_insert_with(|| TunerState { model: TunerModel::new(rate), at: f64::NEG_INFINITY, written: None });
    if (st.model.sample_rate() - rate).abs() > f32::EPSILON {
        *st = TunerState { model: TunerModel::new(rate), at: f64::NEG_INFINITY, written: None };
    }
    if now - st.at < UPDATE_S {
        return;
    }
    st.at = now;
    if hold {
        return;
    }
    // Nothing new since the last round (bypassed, stopped, no tap): a miss, not the old audio again.
    let written = hub.as_ref().and_then(|h| h.samples_written(key.0, uid));
    let fresh = written.is_some() && written != st.written;
    st.written = written;
    if !fresh {
        st.model.miss();
        return;
    }
    let window = st.model.window_mut();
    if hub.as_ref().map_or(0, |h| h.samples(key.0, uid, window)) == 0 {
        window.fill(0.0);
    }
    st.model.analyze(reference_hz, hold);
}

/// Draws the display and the controls' row is drawn by the caller (`panel::show`).
pub fn show(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, lay: &mut EditorLayout) {
    let uid = crate::scope_strip::hub_uid(key, ins);
    let reference_hz = value(app, key, ins, "reference_hz");
    let hold = value(app, key, ins, "hold") >= 0.5;
    update(app, ui, key, uid, reference_hz, hold);
    let reading = app.builtin_editor.tuners.get(&uid).and_then(|s| s.model.reading());
    let t = &mut lay.tuner;
    match reading {
        Some(r) => {
            t.note = r.name();
            t.cents = signed_tenths(r.cents);
            t.freq = format!("{:.1} Hz", r.hz);
            t.text = r.text();
            t.in_tune = r.cents.abs() <= IN_TUNE_CENTS;
        }
        None => {
            t.note = "--".to_string();
            t.cents.clear();
            t.freq = "No signal".to_string();
            t.text = "no signal".to_string();
            t.in_tune = false;
        }
    }
    let color = if reading.is_none() {
        TOKENS.text_dim
    } else if t.in_tune {
        TOKENS.meter_ok
    } else {
        TOKENS.text
    };
    ui.spacing_mut().item_spacing.y = 2.0;
    let (card, card_resp) = ui.allocate_exact_size(vec2(W, NOTE_H + METER_H + FREQ_H + 6.0), Sense::hover());
    hover(ui, &card_resp, Id::TunerDevice);
    let p = ui.painter_at(card);
    p.rect_filled(card, RADIUS_SM, TOKENS.lane);
    // The note, large, with the cents beside it.
    let note_rect = Rect::from_min_size(card.min, vec2(W, NOTE_H));
    p.text(note_rect.center(), Align2::CENTER_CENTER, &t.note, FontId::proportional(52.0), color);
    if reading.is_some() {
        p.text(
            Pos2::new(note_rect.max.x - 8.0, note_rect.max.y - 14.0),
            Align2::RIGHT_CENTER,
            format!("{} cents", t.cents),
            FontId::monospace(11.0),
            TOKENS.text_dim,
        );
    }
    t.note_rect = r4(note_rect);
    let note_resp = ui.interact(note_rect, ui.id().with(("tuner_note", uid)), Sense::hover());
    hover(ui, &note_resp, if reading.is_some() { Id::TunerNote } else { Id::TunerNoSignal });
    // The meter: -50 to +50 cents, the green zone at the center, a tick every 10 cents.
    let meter = Rect::from_min_size(Pos2::new(card.min.x + 8.0, note_rect.max.y + 2.0), vec2(W - 16.0, METER_H));
    p.rect_filled(meter, RADIUS_SM, TOKENS.panel_raised);
    let x_of = |cents: f32| meter.center().x + cents.clamp(-SPAN_CENTS, SPAN_CENTS) / SPAN_CENTS * (meter.width() * 0.5 - 4.0);
    let zone = Rect::from_x_y_ranges(x_of(-IN_TUNE_CENTS)..=x_of(IN_TUNE_CENTS), meter.y_range());
    p.rect_filled(zone, 0.0, TOKENS.meter_ok.gamma_multiply(if reading.is_some() { 0.45 } else { 0.2 }));
    for k in -5_i32..=5 {
        let x = x_of(k as f32 * 10.0);
        let len = if k == 0 || k.abs() == 5 { 0.6 } else { 0.3 };
        let h = meter.height() * len;
        p.line_segment([Pos2::new(x, meter.max.y - h), Pos2::new(x, meter.max.y)], Stroke::new(1.0, TOKENS.grid_bar));
    }
    t.needle_x = reading.map(|r| x_of(r.cents));
    if let Some(x) = t.needle_x {
        p.line_segment([Pos2::new(x, meter.min.y + 2.0), Pos2::new(x, meter.max.y - 2.0)], Stroke::new(3.0, color));
    }
    t.meter = r4(meter);
    let meter_resp = ui.interact(meter, ui.id().with(("tuner_meter", uid)), Sense::hover());
    hover(ui, &meter_resp, Id::TunerMeter);
    // The frequency, small.
    let freq_rect = Rect::from_min_size(Pos2::new(card.min.x, meter.max.y + 2.0), vec2(W, FREQ_H));
    p.text(freq_rect.center(), Align2::CENTER_CENTER, &t.freq, FontId::monospace(11.0), TOKENS.text_dim);
    t.freq_rect = r4(freq_rect);
    let freq_resp = ui.interact(freq_rect, ui.id().with(("tuner_freq", uid)), Sense::hover());
    hover(ui, &freq_resp, if reading.is_some() { Id::TunerFrequency } else { Id::TunerNoSignal });
}
