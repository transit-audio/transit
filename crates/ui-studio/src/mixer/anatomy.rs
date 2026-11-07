//! The channel strip's anatomy (`docs/design/language`, Components > Channel strip): where each
//! section sits in a strip's rect, top to bottom, with the fixed heights that keep every fader
//! aligned across strips. Pure geometry, so the layout tests can check it without painting.

use crate::tokens::{channel_strip as cs, toggle};
use crate::widgets::fader;
use egui::{Rect, pos2, vec2};

/// The fixed sections of one strip, as rects.
#[derive(Debug, Clone, Copy)]
pub(super) struct Anatomy {
    /// The 3 px track color bar, inside the 1 px border.
    pub bar: Rect,
    pub name: Rect,
    pub input: Rect,
    /// The inserts and sends sections' bodies (full height only) and the 1 px line above each.
    pub inserts: Option<Rect>,
    pub inserts_line: Option<Rect>,
    pub sends: Option<Rect>,
    pub sends_line: Option<Rect>,
    /// The short variant's `FX n` and `Snd n` buttons.
    pub fx_button: Option<Rect>,
    pub snd_button: Option<Rect>,
    pub output: Rect,
    pub dial: Rect,
    pub pan_value: Rect,
    /// M S / R I.
    pub toggles: [Rect; 4],
    /// The Channel fader's rect: the fader column, the scale and the meter slot.
    pub fader: Rect,
    pub gain: Rect,
    pub peak: Rect,
}

/// Everything above the fader, from the strip's top edge.
fn above_fader(short: bool) -> f32 {
    let sections = if short {
        cs::SHORT_ROW + cs::SECTION_GAP
    } else {
        2.0 * (1.0 + cs::SECTION_PADDING + cs::SECTION_GAP) + cs::INSERTS_HEIGHT + cs::SENDS_HEIGHT
    };
    1.0 + cs::COLOR_BAR
        + cs::PADDING[0]
        + cs::NAME_HEIGHT
        + cs::SECTION_GAP
        + cs::ROW_HEIGHT
        + cs::SECTION_GAP
        + sections
        + cs::ROW_HEIGHT
        + cs::SECTION_GAP
        + cs::PAN_ROW
        + cs::SECTION_GAP
        + 2.0 * toggle::HEIGHT
        + cs::GAP
        + cs::SECTION_GAP
        + FADER_TOP
}

/// Everything below the fader, to the strip's bottom edge.
const BELOW_FADER: f32 = cs::SECTION_GAP + cs::READOUT_HEIGHT + cs::PADDING[2] + 1.0;
/// The space above the fader's own rect (the board's 2 px).
const FADER_TOP: f32 = 2.0;

/// The height of a strip's sections without its fader.
pub(super) fn fixed_height(short: bool) -> f32 {
    above_fader(short) + BELOW_FADER
}

/// The strip's width: a channel or the main bus.
pub(super) fn width(main: bool) -> f32 {
    if main { cs::WIDTH_MAIN } else { cs::WIDTH }
}

/// The sections of a strip in `r` (`short`: the short variant; `main`: the main bus).
pub(super) fn anatomy(r: Rect, short: bool, main: bool) -> Anatomy {
    let x0 = r.min.x + cs::PADDING[1];
    let w = r.width() - 2.0 * cs::PADDING[1];
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let bar = Rect::from_min_max(pos2(r.min.x + 1.0, r.min.y + 1.0), pos2(r.max.x - 1.0, r.min.y + 1.0 + cs::COLOR_BAR));
    let mut y = bar.max.y + cs::PADDING[0];
    let name = row(y, cs::NAME_HEIGHT);
    y = name.max.y + cs::SECTION_GAP;
    let input = row(y, cs::ROW_HEIGHT);
    y = input.max.y + cs::SECTION_GAP;
    let (mut inserts, mut inserts_line, mut sends, mut sends_line, mut fx_button, mut snd_button) = (None, None, None, None, None, None);
    if short {
        let half = (w - cs::GAP) / 2.0;
        fx_button = Some(Rect::from_min_size(pos2(x0, y), vec2(half, cs::SHORT_ROW)));
        snd_button = Some(Rect::from_min_size(pos2(x0 + half + cs::GAP, y), vec2(half, cs::SHORT_ROW)));
        y += cs::SHORT_ROW + cs::SECTION_GAP;
    } else {
        inserts_line = Some(row(y, 1.0));
        let body = row(y + 1.0 + cs::SECTION_PADDING, cs::INSERTS_HEIGHT);
        inserts = Some(body);
        y = body.max.y + cs::SECTION_GAP;
        sends_line = Some(row(y, 1.0));
        let body = row(y + 1.0 + cs::SECTION_PADDING, cs::SENDS_HEIGHT);
        sends = Some(body);
        y = body.max.y + cs::SECTION_GAP;
    }
    let output = row(y, cs::ROW_HEIGHT);
    y = output.max.y + cs::SECTION_GAP;
    // The pan row: the 36 px dial and its value, centered together.
    let pair = cs::PAN_KNOB + 8.0 + cs::PAN_VALUE_WIDTH;
    let px = x0 + (w - pair) / 2.0;
    let dial = Rect::from_min_size(pos2(px, y + (cs::PAN_ROW - cs::PAN_KNOB) / 2.0), vec2(cs::PAN_KNOB, cs::PAN_KNOB));
    let pan_value = Rect::from_min_size(pos2(dial.max.x + 8.0, y), vec2(cs::PAN_VALUE_WIDTH, cs::PAN_ROW));
    y += cs::PAN_ROW + cs::SECTION_GAP;
    let half = (w - cs::GAP) / 2.0;
    let cell = |i: usize, j: usize| {
        Rect::from_min_size(pos2(x0 + i as f32 * (half + cs::GAP), y + j as f32 * (toggle::HEIGHT + cs::GAP)), vec2(half, toggle::HEIGHT))
    };
    let toggles = [cell(0, 0), cell(1, 0), cell(0, 1), cell(1, 1)];
    y += 2.0 * toggle::HEIGHT + cs::GAP + cs::SECTION_GAP + FADER_TOP;
    let bottom = r.max.y - BELOW_FADER;
    // The Channel fader is 116 wide in a channel; the main bus gives its meters the extra width.
    let fw = if main { w } else { fader::Variant::Channel.size(0.0).x.min(w) };
    let fader = Rect::from_min_max(pos2(x0 + (w - fw) / 2.0, y), pos2(x0 + (w + fw) / 2.0, bottom.max(y)));
    let ry = bottom + cs::SECTION_GAP;
    let peak_w = if main { cs::PEAK_WIDTH_MAIN } else { cs::PEAK_WIDTH };
    let gain = Rect::from_min_size(pos2(x0, ry), vec2(w - cs::GAP - peak_w, cs::READOUT_HEIGHT));
    let peak = Rect::from_min_size(pos2(x0 + w - peak_w, ry), vec2(peak_w, cs::READOUT_HEIGHT));
    Anatomy { bar, name, input, inserts, inserts_line, sends, sends_line, fx_button, snd_button, output, dial, pan_value, toggles, fader, gain, peak }
}

/// The rows inside an inserts or sends body: the section label, then slot `i` (22 high, 3 apart).
pub(super) fn section_label(body: Rect) -> Rect {
    Rect::from_min_size(body.min, vec2(body.width(), LABEL_H))
}

/// The section label's row (the board's 10 px uppercase label).
const LABEL_H: f32 = 11.0;

pub(super) fn slot(body: Rect, i: usize) -> Rect {
    let y = body.min.y + LABEL_H + cs::INSERT_SLOT_GAP + i as f32 * (cs::INSERT_SLOT_HEIGHT + cs::INSERT_SLOT_GAP);
    Rect::from_min_size(pos2(body.min.x, y), vec2(body.width(), cs::INSERT_SLOT_HEIGHT))
}

/// How many slot rows a body holds (inserts: three and "+ Insert"; sends: two and "+ Send").
pub(super) fn slots_in(body: Rect) -> usize {
    ((body.height() - LABEL_H - cs::INSERT_SLOT_GAP + cs::INSERT_SLOT_GAP) / (cs::INSERT_SLOT_HEIGHT + cs::INSERT_SLOT_GAP)).floor().max(0.0) as usize
}
