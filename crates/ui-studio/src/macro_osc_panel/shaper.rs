//! The Shaper (transit's addition, #243) behind its section bar (README, Section bar; the board's
//! Row C): Amp Envelope, Filter with Filter Env, LFO 1 and LFO 2 on the 12-column grid (3, 5, 2
//! and 2 columns). The row is as tall as its tallest card's content, and all four cards share
//! that height (326-X: none stretches to a taller neighbor's).
//!
//! The bar collapses (a per-user view setting, `UiState::device_sections`) and enables (the
//! `shaper` parameter, saved with the preset). Until the user collapses or expands it, it is open
//! while the shaper is on. A disabled but open Shaper dims its body to 40 percent and stays
//! editable.
//!
//! - **Amp Envelope** draws its shape in a display well (segment lengths from the knobs'
//!   positions, a fixed hold).
//! - **Filter:** mode (LP, BP, HP) and slope (12 dB, 24 dB) as segmented controls; the response
//!   well on the 20 Hz to 20 kHz log scale, −42 to +18 dB, its point at the cutoff (drag it across
//!   to set the cutoff); then one knob row: Cutoff, Resonance, Drive, Key track, Env amt, a 1 px
//!   `line` divider, and the filter envelope's Attack, Decay, Sustain, Release under a
//!   `FILTER ENV` caption (326-X; the envelope's own mini display is gone).
//! - **Modulation reach** (326-X): while anything modulates the cutoff (a route to Cutoff, or
//!   Env amt), the well fills between the responses at the lowest and the highest cutoff it can
//!   reach in `mod` at the band alpha, under the base response ([`cutoff_reach`]).
//! - **LFO:** RETRIG and SYNC / HZ as option toggles, the shape as a segmented control in text (the
//!   kit has no shape icons), the trace in `mod` with the fade-in as a dashed `ink.ghost` envelope;
//!   the Rate knob sets the division while SYNC is on and the rate in Hz otherwise.

use super::{Dev, MacroLayout, knob_row, panel, section_label};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::TOKENS;
use crate::tokens::layout as space;
use crate::widgets::{display_well, knob, r4, section_bar, segmented, toggle};
use egui::{Align2, Rect, Shape, Stroke, Ui, pos2, vec2};
use transit_dsp::plugins::filter::svf::MIN_CUTOFF;
use transit_dsp::plugins::macro_osc::matrix::{ROWS, dst};
use transit_dsp::plugins::macro_osc::shaper::MOD_OCTAVES;

/// Room above a section bar (the board's 6 px).
pub const BAR_TOP: f32 = 6.0;
const LABEL_H: f32 = 14.0;
const GAP: f32 = 10.0;
const ENV_WELL_H: f32 = 136.0;
const FILTER_WELL_H: f32 = 120.0;
const SCALE_H: f32 = 14.0;
const LFO_WELL_H: f32 = 68.0;
/// The space either side of the filter row's divider (the Output panel's divider gap).
const DIVIDER_GAP: f32 = crate::tokens::SPACE[1];
/// The room between the caption over the filter envelope's knobs and the knobs.
const CAPTION_GAP: f32 = 4.0;
/// The filter's mode labels (the parameter's choices are Low pass, Band pass, High pass).
pub const MODES: [&str; 3] = ["LP", "BP", "HP"];
pub const SLOPES: [&str; 2] = ["12 dB", "24 dB"];
/// The LFO shapes in the parameter's order, short.
pub const SHAPES: [&str; 6] = ["SIN", "TRI", "SAW", "SQR", "S&H", "RND"];
/// The response well's floor, and the room left over the filter's highest peak (#428).
const DB_FLOOR: f32 = -42.0;
const PEAK_ROOM_DB: f32 = 2.0;
/// The height over which the curve fades into the floor: a slope that runs past the floor fades
/// out instead of running flat along it to 20 kHz.
const FLOOR_FADE_DB: f32 = 12.0;

/// The well's top: the shaper filter's highest gain at any setting (full resonance) plus
/// [`PEAK_ROOM_DB`], so the resonance peak is always drawn as a peak with the cutoff's point on it.
pub fn db_top() -> f32 {
    transit_dsp::plugins::macro_osc::shaper_peak_db() + PEAK_ROOM_DB
}

/// The normalized height of `db` in the well (clamped to the floor and the top).
fn db_y(db: f32) -> f32 {
    let top = db_top();
    (db.clamp(DB_FLOOR, top) - DB_FLOOR) / (top - DB_FLOOR)
}
/// The scale's marks.
pub const SCALE: [(f32, &str); 5] = [(20.0, "20"), (100.0, "100"), (1000.0, "1k"), (10_000.0, "10k"), (20_000.0, "20k")];

/// The collapse key of a section in `UiState::device_sections`.
pub fn section_key(section: &str) -> String {
    format!("{}/{section}", super::MACRO_OSC)
}

/// Whether the Shaper is collapsed: the user's choice, else collapsed while it is off.
pub(crate) fn collapsed(app: &StudioApp, dev: Dev) -> bool {
    app.ui.device_sections.get(&section_key("shaper")).copied().unwrap_or(!dev.on(app, "shaper"))
}

/// Each card's content height: Amp Envelope, Filter, an LFO.
pub fn card_heights() -> [f32; 3] {
    let p = space::PANEL_PADDING[0];
    let (m, s) = (knob::cell(knob::Size::M).y, knob::cell(knob::Size::S).y);
    let amp = p + LABEL_H + GAP + ENV_WELL_H + GAP + s + p;
    let filter = p + segmented::HEIGHT + GAP + FILTER_WELL_H + SCALE_H + GAP + LABEL_H + CAPTION_GAP + m.max(s) + p;
    let lfo = p + toggle_h() + GAP + segmented::HEIGHT + GAP + LFO_WELL_H + GAP + toggle_h() + GAP + s + p;
    [amp, filter, lfo]
}

/// Row C's height: the tallest card's content.
pub fn row_h() -> f32 {
    card_heights().into_iter().fold(0.0, f32::max)
}

/// The section's height: the bar, and the row when open.
pub(crate) fn height(app: &StudioApp, dev: Dev, _width: f32) -> f32 {
    BAR_TOP + section_bar::HEIGHT + if collapsed(app, dev) { 0.0 } else { space::PANEL_GAP + row_h() }
}

/// x on the response well's log scale (0..1) of `hz`.
pub fn freq_x(hz: f32) -> f32 {
    ((hz.max(20.0) / 20.0).log10() / 3.0).clamp(0.0, 1.0)
}

/// The frequency at `x` (0..1) on the response well.
pub fn x_freq(x: f32) -> f32 {
    20.0 * 1000f32.powf(x.clamp(0.0, 1.0))
}

/// The cutoff (Hz) the shaper sets for the knob at `cutoff` and a modulation `offset` (the
/// matrix's Cutoff sum plus Env amt times the envelope's level; 1 is full scale): the formula of
/// the shaper's `design` (key tracking aside, which depends on the note): [`MOD_OCTAVES`] octaves
/// per unit on log2 of the knob, at least the filter's floor. Its ceiling (0.49 of the sample
/// rate) lies above the well's 20 kHz edge at every rate the engine runs.
pub fn modulated_cutoff(cutoff: f32, offset: f32) -> f32 {
    (cutoff.max(1.0).log2() + MOD_OCTAVES * offset).exp2().max(MIN_CUTOFF)
}

/// The lowest and the highest cutoff the modulation can reach from the knob at `cutoff`: the
/// routes to Cutoff move it up to their summed `|amount|` (`route_depth`, 1 is 100%) either way
/// (a source may swing both ways), Env amt (`env`, −1..1) from 0 to its full offset (an envelope
/// only adds). `None` when nothing moves it.
pub fn cutoff_reach(cutoff: f32, route_depth: f32, env: f32) -> Option<[f32; 2]> {
    let (lo, hi) = (-route_depth.abs() + env.min(0.0), route_depth.abs() + env.max(0.0));
    (hi > lo).then(|| [modulated_cutoff(cutoff, lo), modulated_cutoff(cutoff, hi)])
}

/// The summed `|amount|` (1 is 100%) of the rows that route a source to Cutoff.
pub(crate) fn cutoff_depth(app: &StudioApp, dev: Dev) -> f32 {
    (0..ROWS).map(|r| super::route(app, dev, r)).filter(|r| r.source != 0 && r.dest == dst::CUTOFF).map(|r| r.amount.abs() / 100.0).sum()
}

/// The response curve at `cutoff`, sampled across the well (normalized).
pub(crate) fn response_curve(mode: usize, slope24: bool, cutoff: f32, res: f32, sr: f32) -> Vec<[f32; 2]> {
    (0..=180)
        .map(|i| {
            let x = i as f32 / 180.0;
            [x, db_y(response_db(mode, slope24, cutoff, res, sr, x_freq(x)))]
        })
        .collect()
}

/// `curve` with the cutoff's own sample put in its place: a high resonance's peak is narrower than
/// the curve's spacing (16 Hz wide at 400 Hz at full resonance), so without it the line passes
/// under the peak and the cutoff's point floats above it. The gain at the cutoff is within 0.07 dB
/// of the peak from a resonance of 50% up (0.002 dB at full), and the point is always on the line.
pub(crate) fn with_cutoff(mut curve: Vec<[f32; 2]>, at: [f32; 2]) -> Vec<[f32; 2]> {
    let i = curve.partition_point(|p| p[0] < at[0]);
    if curve.get(i).is_some_and(|p| p[0] == at[0]) {
        curve[i] = at;
    } else {
        curve.insert(i, at);
    }
    curve
}

/// The filter's response in dB at `hz`: the shaper filter's own analytic response (#428; low, band
/// or high pass for mode 0, 1, 2, 12 or 24 dB, the resonance 0..1) at sample rate `sr`.
pub fn response_db(mode: usize, slope24: bool, cutoff: f32, resonance: f32, sr: f32, hz: f32) -> f32 {
    transit_dsp::plugins::macro_osc::shaper_response_db(mode, slope24, cutoff, resonance, sr, hz)
}

/// An ADSR shape in a well's normalized space: attack, decay and release take their knobs'
/// positions (0..1) of the width with a fixed hold between; sustain is the level.
pub fn env_curve(a: f32, d: f32, s: f32, r: f32) -> Vec<[f32; 2]> {
    const HOLD: f32 = 0.35;
    let total = (a + d + HOLD + r).max(1e-6);
    let (xa, xd) = (a / total, (a + d) / total);
    let xs = xd + HOLD / total;
    let top = 0.94;
    let s = s.clamp(0.0, 1.0) * top;
    vec![[0.0, 0.0], [xa, top], [xd, s], [xs, s], [1.0, 0.0]]
}

/// The LFO's trace: `shape` (the parameter's order) over `cycles`, its fade-in over the first
/// `fade` of the width, scaled by `amount` (0..1); y in 0..1 around 0.5. And the fade envelope.
pub fn lfo_curve(shape: usize, cycles: f32, fade: f32, amount: f32) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    const N: usize = 120;
    // Fixed steps for the random shapes, so the drawing is the same every frame.
    const STEPS: [f32; 8] = [0.6, -0.4, 0.9, -0.8, 0.2, -0.1, 0.7, -0.6];
    let mut trace = Vec::with_capacity(N + 1);
    let mut env = Vec::new();
    for i in 0..=N {
        let t = i as f32 / N as f32;
        let ph = t * cycles;
        let f = ph.fract();
        let v = match shape {
            1 => 1.0 - 4.0 * (f - 0.5).abs(),
            2 => 2.0 * f - 1.0,
            3 => {
                if f < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            4 => STEPS[(ph as usize) % STEPS.len()],
            5 => {
                let (a, b) = (STEPS[(ph as usize) % STEPS.len()], STEPS[(ph as usize + 1) % STEPS.len()]);
                a + (b - a) * (0.5 - 0.5 * (f * std::f32::consts::PI).cos())
            }
            _ => (std::f32::consts::TAU * ph).sin(),
        };
        let g = if fade > 0.0 { (t / fade).min(1.0) } else { 1.0 } * amount.clamp(0.0, 1.0);
        trace.push([t, 0.5 + 0.42 * v * g]);
        if fade > 0.0 {
            env.push([t, 0.5 + 0.42 * g]);
        }
    }
    (trace, env)
}

/// The one-line summary of the collapsed bar.
pub(crate) fn summary(app: &StudioApp, dev: Dev) -> String {
    let tx = |id: &str| dev.text(app, id);
    let mode = MODES.get(dev.index(app, "filter_mode")).copied().unwrap_or("LP");
    let lfo = |n: u8| {
        let rate = if dev.on(app, &format!("lfo{n}_sync")) { tx(&format!("lfo{n}_division")) } else { tx(&format!("lfo{n}_rate")) };
        format!("LFO {n} {rate}")
    };
    format!(
        "Amp {} · {} · {} · {}   ·   {mode} {} · {} · {}   ·   {}   ·   {}",
        tx("amp_attack"),
        tx("amp_decay"),
        tx("amp_sustain"),
        tx("amp_release"),
        tx("filter_slope"),
        tx("cutoff"),
        tx("resonance"),
        lfo(1),
        lfo(2)
    )
}

/// The bar and, when open, Row C, from `rect`'s top. Returns the height drawn.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) -> f32 {
    let collapsed = collapsed(app, dev);
    let on = dev.on(app, "shaper");
    let bar_r = Rect::from_min_size(pos2(rect.min.x + 2.0, rect.min.y + BAR_TOP), vec2(rect.width() - 4.0, section_bar::HEIGHT));
    let id = egui::Id::new(("macro_osc_shaper", dev.uid()));
    let text = summary(app, dev);
    let bar = section_bar::Bar {
        title: "Shaper",
        tag: Some("TRANSIT EXTENSION"),
        description: Some("Envelopes, filter and LFOs · not part of the original module"),
        summary: Some(&text),
        collapsed,
        enabled: Some(on),
    };
    let mut c = super::child(ui, bar_r);
    let b = section_bar::show(&mut c, id, bar_r.width(), &bar);
    if b.toggle_collapse {
        app.ui.device_sections.insert(section_key("shaper"), !collapsed);
    }
    if let Some(tag) = b.layout.tag {
        let r = ui.interact(rect_of(tag), id.with("tag"), egui::Sense::hover());
        crate::help::hover(ui, &r, crate::help::Id::MacroOscAddition);
    }
    if let Some(sw) = &b.layout.switch {
        // The switch's own response (same id and rect): help, MIDI Learn and the row.
        let r = ui.interact(rect_of(sw.rect), id.with("enable"), egui::Sense::click());
        super::finish(app, ui, dev, "shaper", &r, b.toggle_enable.then_some(if on { 0.0 } else { 1.0 }), lay);
    }
    out.shaper_bar = Some(b.layout);
    let opacity = section_bar::body_opacity(Some(on));
    out.shaper_opacity = opacity;
    if collapsed {
        out.shaper_body = None;
        return BAR_TOP + section_bar::HEIGHT;
    }
    let y = bar_r.max.y + space::PANEL_GAP;
    let body = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), row_h()));
    out.shaper_body = Some(r4(body));
    let mut c = super::child(ui, body);
    c.multiply_opacity(opacity);
    let col = (body.width() - 3.0 * space::PANEL_GAP) / 12.0;
    let mut x = body.min.x;
    for (span, which) in [(3.0, 0), (5.0, 1), (2.0, 2), (2.0, 3)] {
        let r = Rect::from_min_size(pos2(x, body.min.y), vec2(col * span, body.height()));
        match which {
            0 => amp(app, &mut c, dev, r, lay, out),
            1 => filter(app, &mut c, dev, r, lay, out),
            n => lfo(app, &mut c, dev, r, n - 1, lay, out),
        }
        x = r.max.x + space::PANEL_GAP;
    }
    BAR_TOP + section_bar::HEIGHT + space::PANEL_GAP + row_h()
}

pub(crate) fn rect_of(r: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
}

/// A display well at `r` with `curve`, recorded under `name`.
fn well(ui: &mut Ui, dev: Dev, r: Rect, name: &str, w: &display_well::Well, out: &mut MacroLayout) -> display_well::WellOut {
    let mut c = super::child(ui, r);
    let shown = display_well::show(&mut c, egui::Id::new(("macro_osc_well", dev.uid(), name.to_string())), r.size(), w);
    out.wells.push(super::WellRecord { name: name.to_string(), well: shown.layout.clone() });
    shown
}

fn norm(app: &StudioApp, dev: Dev, id: &str) -> f32 {
    dev.param(id).map_or(0.0, |p| p.to_normalized(dev.value(app, id)))
}

/// A segmented control for a choice parameter, recorded.
#[allow(clippy::too_many_arguments)]
fn segments(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    id: &str,
    options: &[&str],
    at: Rect,
    width: Option<f32>,
    lay: &mut EditorLayout,
    out: &mut MacroLayout,
) {
    let sel = dev.index(app, id).min(options.len().saturating_sub(1));
    let mut c = super::child(ui, at);
    let s = segmented::show(&mut c, egui::Id::new(("macro_osc_seg", dev.uid(), id.to_string())), options, sel, width);
    let r = ui.interact(rect_of(s.layout.track), egui::Id::new(("macro_osc_seg_row", dev.uid(), id.to_string())), egui::Sense::hover());
    super::finish(app, ui, dev, id, &r, s.picked.map(|i| i as f32), lay);
    out.segments.push(super::SegmentRecord { param: id.to_string(), options: options.iter().map(|o| o.to_string()).collect(), segmented: s.layout });
}

/// An option toggle: `RETRIG` flips its parameter; `SYNC` and `HZ` set the mode they name
/// (`lfo<n>_sync` on or off). Only the parameter's first toggle records its row and takes MIDI
/// Learn (HZ is SYNC's other side).
#[allow(clippy::too_many_arguments)]
fn option(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    id: &str,
    text: &str,
    sets: Option<bool>,
    at: Rect,
    lay: &mut EditorLayout,
    out: &mut MacroLayout,
) {
    let value = dev.on(app, id);
    let on = sets.map_or(value, |s| s == value);
    let mut c = super::child(ui, at);
    let tg = toggle::show(
        &mut c,
        egui::Id::new(("macro_osc_toggle", dev.uid(), id.to_string(), text.to_string())),
        text,
        toggle::Role::Option,
        on,
        true,
        None,
    );
    let new = tg.toggled.then(|| match sets {
        Some(s) => f32::from(u8::from(s)),
        None => f32::from(u8::from(!value)),
    });
    let name = if sets == Some(false) { format!("{id}:hz") } else { id.to_string() };
    if sets == Some(false) {
        if let Some(h) = super::help(id) {
            crate::help::hover(ui, &tg.response, h);
        }
        if let Some(v) = new {
            super::set(app, dev, id, v);
        }
    } else {
        super::finish(app, ui, dev, id, &tg.response, new, lay);
    }
    out.toggles.push(super::ToggleRecord { param: name, toggle: tg.layout });
}

fn amp(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "amp_env", out);
    section_label(ui, inner, "Amp Envelope");
    crate::widgets::role_text(ui.painter(), &crate::tokens::READOUT_DISPLAY, "ENV 1", t.text_faint, Align2::RIGHT_TOP, inner.right_top());
    let curve = env_curve(norm(app, dev, "amp_attack"), norm(app, dev, "amp_decay"), norm(app, dev, "amp_sustain"), norm(app, dev, "amp_release"));
    let w = Rect::from_min_size(pos2(inner.min.x, inner.min.y + LABEL_H + GAP), vec2(inner.width(), ENV_WELL_H));
    well(ui, dev, w, "amp_env", &display_well::Well { curve: &curve, ..Default::default() }, out);
    let row = Rect::from_min_max(pos2(inner.min.x, w.max.y + GAP), inner.max);
    let items =
        [("amp_velocity", "Velocity"), ("amp_attack", "Attack"), ("amp_decay", "Decay"), ("amp_sustain", "Sustain"), ("amp_release", "Release")];
    knob_row(app, ui, dev, row, &items, knob::Size::S, lay, out);
}

fn filter(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "filter", out);
    let head = Rect::from_min_size(inner.min, vec2(inner.width(), segmented::HEIGHT));
    section_label(ui, Rect::from_min_size(head.min + vec2(0.0, 5.0), head.size()), "Filter");
    let p = ui.painter().clone();
    let slope_w = SLOPES.iter().map(|s| crate::widgets::role_size(&p, &crate::tokens::VALUE, s).x + 2.0 * segmented::SEGMENT_PAD).sum::<f32>() + 4.0;
    let mode_w = MODES.iter().map(|s| crate::widgets::role_size(&p, &crate::tokens::VALUE, s).x + 2.0 * segmented::SEGMENT_PAD).sum::<f32>() + 4.0;
    segments(
        app,
        ui,
        dev,
        "filter_slope",
        &SLOPES,
        Rect::from_min_size(pos2(head.max.x - slope_w, head.min.y), vec2(slope_w, head.height())),
        None,
        lay,
        out,
    );
    let mx = head.max.x - slope_w - 8.0 - mode_w;
    segments(app, ui, dev, "filter_mode", &MODES, Rect::from_min_size(pos2(mx, head.min.y), vec2(mode_w, head.height())), None, lay, out);
    // The response well, with the modulation's reach under the base response.
    let (mode, slope24) = (dev.index(app, "filter_mode"), dev.index(app, "filter_slope") == 1);
    let (cutoff, res) = (dev.value(app, "cutoff"), dev.value(app, "resonance") / 100.0);
    let sr = app.engine.session().sample_rate.as_f64() as f32;
    let cx = freq_x(cutoff);
    let at_cut = db_y(response_db(mode, slope24, cutoff, res, sr, cutoff));
    let curve = with_cutoff(response_curve(mode, slope24, cutoff, res, sr), [cx, at_cut]);
    let reach = cutoff_reach(cutoff, cutoff_depth(app, dev), dev.value(app, "filter_env_amount") / 100.0);
    let edges = reach.map(|[lo, hi]| (response_curve(mode, slope24, lo, res, sr), response_curve(mode, slope24, hi, res, sr)));
    let wr = Rect::from_min_size(pos2(inner.min.x, head.max.y + GAP), vec2(inner.width(), FILTER_WELL_H));
    let between = edges.as_ref().map(|(lo, hi)| (lo.as_slice(), hi.as_slice()));
    let floor_fade = FLOOR_FADE_DB / (db_top() - DB_FLOOR);
    let shown =
        well(ui, dev, wr, "filter", &display_well::Well { curve: &curve, points: &[[cx, at_cut]], between, floor_fade, ..Default::default() }, out);
    let plot = display_well::plot_rect(wr, false);
    out.filter_reach = reach.map(|[lo, hi]| [plot.min.x + plot.width() * freq_x(lo), plot.min.x + plot.width() * freq_x(hi)]);
    for hz in [100.0, 1000.0, 10_000.0] {
        let x = plot.min.x + plot.width() * freq_x(hz);
        p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], Stroke::new(1.0, t.grid));
    }
    let zero = plot.max.y - plot.height() * db_y(0.0);
    p.extend(Shape::dashed_line(&[pos2(plot.min.x, zero), pos2(plot.max.x, zero)], Stroke::new(1.0, t.line), 3.0, 3.0));
    for (hz, label) in SCALE {
        let x = plot.min.x + plot.width() * freq_x(hz);
        let anchor = if hz <= 20.0 {
            Align2::LEFT_TOP
        } else if hz >= 20_000.0 {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        crate::widgets::role_text(&p, &crate::tokens::READOUT_DISPLAY, label, t.text_faint, anchor, pos2(x, wr.max.y + 1.0));
    }
    if let Some((_, [x, _])) = shown.dragged
        && let Some(cp) = dev.param("cutoff")
    {
        super::set(app, dev, "cutoff", cp.clamp(x_freq(x)));
    }
    // One knob row: the filter's five, a divider, the filter envelope's four under its caption.
    // Nine equal slots; the knobs share the label and value lines (the smaller ones sit lower).
    let cy = wr.max.y + SCALE_H + GAP;
    let ky = cy + LABEL_H + CAPTION_GAP;
    let (m, s) = (knob::cell(knob::Size::M).y, knob::cell(knob::Size::S).y);
    let slot = (inner.width() - 2.0 * DIVIDER_GAP - 1.0) / 9.0;
    let left = Rect::from_min_size(pos2(inner.min.x, ky), vec2(5.0 * slot, m));
    let items = [("cutoff", "Cutoff"), ("resonance", "Resonance"), ("drive", "Drive"), ("key_track", "Key track"), ("filter_env_amount", "Env amt")];
    knob_row(app, ui, dev, left, &items, knob::Size::M, lay, out);
    let dx = left.max.x + DIVIDER_GAP + 0.5;
    p.line_segment([pos2(dx, cy), pos2(dx, ky + m)], Stroke::new(1.0, t.line));
    out.filter_divider = [dx - 0.5, cy, dx + 0.5, ky + m];
    let right = Rect::from_min_size(pos2(dx + 0.5 + DIVIDER_GAP, ky + m - s), vec2(4.0 * slot, s));
    let caption = crate::widgets::role_text(&p, &crate::tokens::LABEL_SECTION, "Filter Env", t.text_muted, Align2::LEFT_TOP, pos2(right.min.x, cy));
    out.filter_env_caption = r4(caption);
    let items = [("filter_attack", "Attack"), ("filter_decay", "Decay"), ("filter_sustain", "Sustain"), ("filter_release", "Release")];
    knob_row(app, ui, dev, right, &items, knob::Size::S, lay, out);
}

fn lfo(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, n: usize, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let id = |f: &str| format!("lfo{n}_{f}");
    let inner = panel(ui, rect, &format!("lfo{n}"), out);
    let inner = Rect::from_min_max(pos2(rect.min.x + 14.0, inner.min.y), pos2(rect.max.x - 14.0, inner.max.y));
    let head = Rect::from_min_size(inner.min, vec2(inner.width(), toggle_h()));
    section_label(ui, Rect::from_min_size(head.min + vec2(0.0, 5.0), head.size()), &format!("LFO {n}"));
    let rw = toggle::width(ui, "RETRIG");
    option(app, ui, dev, &id("retrigger"), "RETRIG", None, Rect::from_min_size(pos2(head.max.x - rw, head.min.y), vec2(rw, head.height())), lay, out);
    let sy = head.max.y + GAP;
    segments(
        app,
        ui,
        dev,
        &id("shape"),
        &SHAPES,
        Rect::from_min_size(pos2(inner.min.x, sy), vec2(inner.width(), segmented::HEIGHT)),
        Some(inner.width()),
        lay,
        out,
    );
    // The trace.
    let sync = dev.on(app, &id("sync"));
    let cycles = if sync { 2.0 } else { (dev.value(app, &id("rate")) * 2.0).clamp(0.5, 8.0) };
    let fade = (dev.value(app, &id("fade")) / 2000.0).clamp(0.0, 1.0);
    let (trace, env) = lfo_curve(dev.index(app, &id("shape")), cycles, fade, dev.value(app, &id("amount")) / 100.0);
    let wr = Rect::from_min_size(pos2(inner.min.x, sy + segmented::HEIGHT + GAP), vec2(inner.width(), LFO_WELL_H));
    well(ui, dev, wr, &format!("lfo{n}"), &display_well::Well::default(), out);
    let plot = display_well::plot_rect(wr, false);
    let p = ui.painter().clone();
    let mid = plot.center().y;
    p.line_segment([pos2(plot.min.x, mid), pos2(plot.max.x, mid)], Stroke::new(1.0, t.grid));
    if !env.is_empty() {
        let pts: Vec<_> = env.iter().map(|q| display_well::to_screen(plot, *q)).collect();
        for w in pts.windows(2).step_by(2) {
            p.line_segment([w[0], w[1]], Stroke::new(1.0, t.ink_ghost));
        }
    }
    p.add(Shape::line(trace.iter().map(|q| display_well::to_screen(plot, *q)).collect(), Stroke::new(2.0, t.modulation)));
    // Rate: SYNC and HZ, then the knobs.
    let ry = wr.max.y + GAP;
    crate::widgets::role_text(&p, &crate::tokens::LABEL_CONTROL, "Rate", t.text_muted, Align2::LEFT_CENTER, pos2(inner.min.x, ry + toggle_h() / 2.0));
    let (sw, hw) = (toggle::width(ui, "SYNC"), toggle::width(ui, "HZ"));
    option(app, ui, dev, &id("sync"), "SYNC", Some(true), Rect::from_min_size(pos2(inner.max.x - hw - 2.0 - sw, ry), vec2(sw, toggle_h())), lay, out);
    option(app, ui, dev, &id("sync"), "HZ", Some(false), Rect::from_min_size(pos2(inner.max.x - hw, ry), vec2(hw, toggle_h())), lay, out);
    let ky = ry + toggle_h() + GAP;
    let (rate, fade, amount) = (if sync { id("division") } else { id("rate") }, id("fade"), id("amount"));
    let items = [(rate.as_str(), "Rate"), (fade.as_str(), "Fade in"), (amount.as_str(), "Amount")];
    knob_row(app, ui, dev, Rect::from_min_max(pos2(inner.min.x - 6.0, ky), pos2(inner.max.x + 6.0, inner.max.y)), &items, knob::Size::S, lay, out);
}

fn toggle_h() -> f32 {
    crate::tokens::toggle::HEIGHT
}
