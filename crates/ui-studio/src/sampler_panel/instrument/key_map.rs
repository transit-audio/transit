//! The key map, the Instrument view's hero (README, Sampler views, Instrument view; the boards
//! `SamplerInstrument` and `SamplerRound2`): the `KEY MAP` label and its hint, then a display well
//! over a keyboard with the velocity axis at its left (`127`, `64`, `1`, a `grid` line at 64,
//! octave lines at each C). Every zone is a rectangle over its keys and velocities: its root key
//! and label top left, the `RR n` tag top right on a round-robin layer, the root dot on its foot;
//! the selected zone takes the `signal` edge and wash. Pad-bound zones show as one key at full
//! velocity (the pad's note) with a dashed edge, the pad id and the lock: they do not drag. Held
//! keys light on the keyboard and as a column over the zones.
//!
//! The keys shown run from the lowest to the highest key any zone covers (a pad zone covers its
//! pad's note), at least two octaves, widened to white keys at both ends. A round-robin layer's
//! alternatives share one rectangle: the map draws the selected alternative, else the layer's
//! lowest. Everything comes from `mix.sampler_state` ([`super::state`]); the gestures are
//! [`super::key_map_gestures`].

use super::color;
use super::key_map_gestures::{self as g, Drag, Part, ZoneEdit};
use super::state::{State, Zone};
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, chrome, layout, radius, sampler as tk};
use crate::widgets::{self, r4};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

/// The gap between the header's items and under the header (the `space` steps 12 and 8).
const ITEM_GAP: f32 = SPACE[2];
const ROW_GAP: f32 = SPACE[1];
/// How far inside a zone's edge a press holds that edge (no stated value: the `space` step 4).
pub const EDGE: f32 = SPACE[0];
/// The fewest keys the map shows.
const MIN_KEYS: u8 = 24;

pub const HINT: &str = "drag an edge for the key range · the top for the velocity split · the dot for the root";

const fn mono(weight: f32, size: f32) -> tokens::TypeRole {
    tokens::TypeRole { family: tokens::Family::Mono, weight: weight as u16, size, tracking: 0.0, uppercase: false, ink: None }
}
pub(crate) const ZONE_LABEL: tokens::TypeRole = mono(tk::TYPE[0].2, tk::TYPE[0].3);
const RR_TAG: tokens::TypeRole = mono(tk::TYPE[1].2, tk::TYPE[1].3);
const OCTAVE_NAME: tokens::TypeRole = mono(tk::TYPE[2].2, tk::TYPE[2].3);
pub(crate) const AXIS: tokens::TypeRole = mono(tk::TYPE[3].2, tk::TYPE[3].3);

/// Whether `n` is a black key.
pub fn is_black(n: u8) -> bool {
    matches!(n % 12, 1 | 3 | 6 | 8 | 10)
}

/// The keys shown and their x spans across the well's inside.
#[derive(Debug, Clone, PartialEq)]
pub struct Keys {
    pub lo: u8,
    pub hi: u8,
    /// Each key's (x, width), `lo` first.
    spans: Vec<(f32, f32)>,
}

impl Keys {
    /// Keys `lo..=hi` (white at both ends) across `x0..x0 + width`: equal white keys, each black
    /// key `BLACK_KEY_WIDTH` of a white one, centered on the boundary before it.
    pub fn new(lo: u8, hi: u8, x0: f32, width: f32) -> Keys {
        let whites = (lo..=hi).filter(|n| !is_black(*n)).count().max(1);
        let ww = width / whites as f32;
        let mut spans = Vec::with_capacity(usize::from(hi - lo) + 1);
        let mut i = 0;
        for n in lo..=hi {
            if is_black(n) {
                let bw = ww * tk::BLACK_KEY_WIDTH;
                spans.push((x0 + i as f32 * ww - bw / 2.0, bw));
            } else {
                spans.push((x0 + i as f32 * ww, ww));
                i += 1;
            }
        }
        Keys { lo, hi, spans }
    }

    /// Key `n`'s (x, width), clamped to the keys shown.
    pub fn span(&self, n: u8) -> (f32, f32) {
        let i = usize::from(n.clamp(self.lo, self.hi) - self.lo);
        self.spans[i]
    }

    /// The key under `x` (black keys first, as on a keyboard), clamped to the keys shown.
    pub fn at(&self, x: f32) -> u8 {
        let hit = |black: bool| {
            (self.lo..=self.hi).find(|n| {
                is_black(*n) == black && {
                    let (a, w) = self.span(*n);
                    x >= a && x < a + w
                }
            })
        };
        if let Some(n) = hit(true).or_else(|| hit(false)) {
            return n;
        }
        if x < self.span(self.lo).0 { self.lo } else { self.hi }
    }
}

/// The keys the map shows for `state` (see the module notes).
pub fn range(state: &State) -> (u8, u8) {
    let covered = state.zones.iter().filter_map(|z| match z.pad {
        Some(p) => state.pad_note(p).map(|n| (n, n)),
        None => Some((z.keys[0], z.keys[1])),
    });
    let (mut lo, mut hi) =
        covered.fold((None::<u8>, None::<u8>), |(lo, hi), (a, b)| (Some(lo.map_or(a, |l| l.min(a))), Some(hi.map_or(b, |h| h.max(b)))));
    let start = state.pad_start_note;
    let (mut lo, mut hi) = (lo.take().unwrap_or(start), hi.take().unwrap_or(start));
    if hi - lo + 1 < MIN_KEYS {
        hi = lo.saturating_add(MIN_KEYS - 1).min(127);
        lo = hi.saturating_sub(MIN_KEYS - 1).min(lo);
    }
    if is_black(lo) {
        lo -= 1;
    }
    if is_black(hi) {
        hi += 1;
    }
    (lo, hi)
}

/// One zone as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneRecord {
    pub zone: u16,
    pub rect: [f32; 4],
    /// `E2 ff`, or the pad id on a pad zone.
    pub label: String,
    pub label_rect: [f32; 4],
    pub dot: [f32; 4],
    pub pad: Option<u8>,
    pub selected: bool,
    pub fill: Color32,
    pub edge: Color32,
    pub edge_width: f32,
    pub dashed: bool,
    pub ink: Color32,
    pub dot_ink: Color32,
    /// The `RR n` tag and where it sits.
    pub rr: Option<(String, [f32; 4])>,
    /// A pad zone's lock.
    pub lock: Option<[f32; 4]>,
    /// Edge fades: the rect at full fill inside the wedges, and the fill's alpha at the edges.
    pub fade: Option<([f32; 4], f32)>,
}

/// One key of the keyboard as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyRecord {
    pub note: u8,
    pub rect: [f32; 4],
    pub fill: Color32,
    /// The octave name on a C.
    pub name: Option<(String, [f32; 4])>,
}

/// The key map as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyMapLayout {
    pub panel: [f32; 4],
    pub label: [f32; 4],
    pub hint: [f32; 4],
    pub axis: [f32; 4],
    /// `127`, `64`, `1` and where each sits.
    pub axis_labels: Vec<(String, [f32; 4])>,
    pub well: [f32; 4],
    /// The line at velocity 64 and the octave lines (their x).
    pub mid_line: [f32; 4],
    pub octave_lines: Vec<f32>,
    pub keyboard: [f32; 4],
    pub lo: u8,
    pub hi: u8,
    /// The keys' left edge and width (the well's inside), unrounded: [`Keys::new`]'s inputs.
    pub frame: (f32, f32),
    pub zones: Vec<ZoneRecord>,
    pub keys: Vec<KeyRecord>,
    /// The held keys' columns over the zones.
    pub held: Vec<[f32; 4]>,
    /// A drag's preview: the dragged zone's rect, whether it conflicts and why.
    pub preview: Option<([f32; 4], Option<String>)>,
}

impl KeyMapLayout {
    pub fn zone(&self, n: u16) -> Option<&ZoneRecord> {
        self.zones.iter().find(|z| z.zone == n)
    }
}

fn line_h(p: &egui::Painter, role: &tokens::TypeRole) -> f32 {
    widgets::role_size(p, role, "Ag").y
}

fn header_h(p: &egui::Painter) -> f32 {
    line_h(p, &tokens::LABEL_SECTION).max(line_h(p, &tokens::READOUT_DISPLAY))
}

/// The panel's height.
pub fn height(p: &egui::Painter) -> f32 {
    2.0 * layout::PANEL_PADDING[0] + header_h(p) + ROW_GAP + tk::KEY_MAP_WELL_HEIGHT + tk::KEYBOARD_GAP + tk::KEYBOARD_HEIGHT
}

/// What the map shows beside the state.
pub struct Inputs<'a> {
    pub selected: Option<u16>,
    /// Notes sounding now (the live tap's pads).
    pub held: &'a [u8],
    pub names: transit_midi::MiddleC,
    pub session: &'a transit_model::Session,
}

/// What the map's input asks for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Select(u16),
    Map(Vec<ZoneEdit>),
    /// An Alt-drag on a pad's zone: select it and ask about making it a group (the Group panel's
    /// Notice; nothing changes until Make group).
    Ask(u16),
}

/// The zone's label: its root key's name, then its stored label (`E2 ff`); the pad id on a pad
/// zone.
pub fn label(state: &State, z: &Zone, names: transit_midi::MiddleC) -> String {
    if let Some(p) = z.pad {
        return super::super::pad_id(p - 1);
    }
    let root = transit_midi::note_name(z.root, names);
    match state.entry(z.zone).and_then(|e| e.label.as_deref()).filter(|l| !l.is_empty()) {
        Some(l) => format!("{root} {l}"),
        None => root,
    }
}

/// The zones to draw, bottom first: one per round-robin layer (the selected alternative, else the
/// lowest), the pad zones, the selected zone last.
fn drawn(state: &State, selected: Option<u16>) -> Vec<&Zone> {
    let shown = |z: &&Zone| match z.membership {
        None => true,
        Some(m) => {
            let peers = state.layer_peers(z);
            match peers.iter().find(|o| Some(o.zone) == selected) {
                Some(s) => s.zone == z.zone,
                None => peers.iter().map(|o| o.membership.map_or(0, |p| p.alternative)).min() == Some(m.alternative),
            }
        }
    };
    let mut out: Vec<&Zone> = state.zones.iter().filter(|z| z.pad.is_none()).filter(shown).collect();
    out.extend(state.zones.iter().filter(|z| z.pad.is_some()));
    if let Some(i) = out.iter().position(|z| Some(z.zone) == selected) {
        let s = out.remove(i);
        out.push(s);
    }
    out
}

/// The map's frame: the well's inside, the keys, and velocity to y.
struct Frame {
    area: Rect,
    keys: Keys,
}

impl Frame {
    fn y(&self, v: f32) -> f32 {
        self.area.max.y - v / 127.0 * self.area.height()
    }

    fn velocity_at(&self, y: f32) -> u8 {
        (((self.area.max.y - y) / self.area.height()) * 127.0).round().clamp(1.0, 127.0) as u8
    }

    /// The rect of keys `k` and velocities `v`, a zone inset inside them.
    fn zone_rect(&self, k: [u8; 2], v: [u8; 2]) -> Rect {
        let (x0, _) = self.keys.span(k[0]);
        let (a, w) = self.keys.span(k[1]);
        let r = Rect::from_min_max(pos2(x0, self.y(f32::from(v[1]))), pos2(a + w, self.y(f32::from(v[0]) - 1.0)));
        r.shrink(tk::ZONE_INSET)
    }

    /// The part of zone `z`'s rect `r` at full fill: inside its key fades (a low width n is full
    /// from key `lo + n`, a high one up to key `hi - n`) and its velocity fades (full from
    /// velocity `lo + n`, up to `hi - n`).
    fn fade_inner(&self, z: &Zone, r: Rect, w: [u8; 4]) -> Rect {
        let [klo, khi] = z.keys;
        let [vlo, vhi] = z.velocities;
        let left = if w[0] > 0 { self.keys.span(klo.saturating_add(w[0])).0 } else { r.min.x };
        let right = if w[1] > 0 {
            let (a, width) = self.keys.span(khi.saturating_sub(w[1]));
            a + width
        } else {
            r.max.x
        };
        let bottom = if w[2] > 0 { self.y(f32::from(vlo.saturating_add(w[2])) - 1.0) } else { r.max.y };
        let top = if w[3] > 0 { self.y(f32::from(vhi.saturating_sub(w[3]))) } else { r.min.y };
        let cx = |x: f32| x.clamp(r.min.x, r.max.x);
        let cy = |y: f32| y.clamp(r.min.y, r.max.y);
        Rect::from_min_max(pos2(cx(left), cy(top)), pos2(cx(right).max(cx(left)), cy(bottom).max(cy(top))))
    }

    fn rect_of(&self, state: &State, z: &Zone) -> Rect {
        match z.pad.and_then(|p| state.pad_note(p)) {
            Some(n) => self.zone_rect([n, n], [1, 127]),
            None => self.zone_rect(z.keys, z.velocities),
        }
    }
}

/// Where a press at `at` lands on `rect` (a zone with its dot at `dot`): an edge or the dot, or the
/// body (`None`).
pub fn part_at(rect: Rect, dot: Rect, at: egui::Pos2) -> Option<Part> {
    if dot.expand(EDGE / 2.0).contains(at) {
        Some(Part::Root)
    } else if at.y - rect.min.y < EDGE {
        Some(Part::Top)
    } else if at.x - rect.min.x < EDGE {
        Some(Part::Low)
    } else if rect.max.x - at.x < EDGE {
        Some(Part::High)
    } else {
        None
    }
}

fn value_of(z: &Zone, part: Part) -> u8 {
    match part {
        Part::Low => z.keys[0],
        Part::High => z.keys[1],
        Part::Top => z.velocities[1],
        Part::Root => z.root,
    }
}

/// Draws the map into `rect` (its panel) for instance `uid`. `drag` and `conflict` are the view's
/// drag and the last preview check (the edits it was made for and its result).
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut Ui,
    uid: u64,
    state: &State,
    inputs: &Inputs,
    drag: &mut Option<Drag>,
    conflict: &mut Option<(Vec<ZoneEdit>, Option<String>)>,
    rect: Rect,
) -> (KeyMapLayout, Option<Intent>) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let mut out = KeyMapLayout { panel: r4(rect), ..KeyMapLayout::default() };
    p.rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(layout::PANEL_PADDING[1], layout::PANEL_PADDING[0]));
    let hh = header_h(&p);
    let cy = inner.min.y + hh / 2.0;
    out.label = r4(widgets::role_text(&p, &tokens::LABEL_SECTION, "Key map", t.text_muted, Align2::LEFT_CENTER, pos2(inner.min.x, cy)));
    out.hint = r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, HINT, t.text_faint, Align2::LEFT_CENTER, pos2(out.label[2] + ITEM_GAP, cy)));
    // The axis, the well and the keyboard.
    let top = inner.min.y + hh + ROW_GAP;
    let axis = Rect::from_min_size(pos2(inner.min.x, top), vec2(tk::AXIS_WIDTH, tk::KEY_MAP_WELL_HEIGHT));
    out.axis = r4(axis);
    let well = Rect::from_min_max(pos2(axis.max.x + tk::AXIS_GAP, top), pos2(inner.max.x, top + tk::KEY_MAP_WELL_HEIGHT));
    out.well = r4(well);
    p.rect(well, radius::DISPLAY, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line_inset), StrokeKind::Inside);
    let area = well.shrink(chrome::FIELD_EDGE_WIDTH);
    let (lo, hi) = range(state);
    (out.lo, out.hi) = (lo, hi);
    out.frame = (area.min.x, area.width());
    let f = Frame { area, keys: Keys::new(lo, hi, area.min.x, area.width()) };
    let ax = axis.max.x - tk::AXIS_INSET;
    for (text, align, y) in [("127", Align2::RIGHT_TOP, area.min.y), ("64", Align2::RIGHT_CENTER, f.y(64.0)), ("1", Align2::RIGHT_BOTTOM, area.max.y)]
    {
        let r = widgets::role_text(&p, &AXIS, text, t.text_faint, align, pos2(ax, y));
        out.axis_labels.push((text.to_string(), r4(r)));
    }
    let wp = p.with_clip_rect(area);
    let mid = Rect::from_min_size(pos2(area.min.x, f.y(64.0).round()), vec2(area.width(), 1.0));
    wp.rect_filled(mid, 0.0, t.grid);
    out.mid_line = r4(mid);
    for n in (lo..=hi).filter(|n| n % 12 == 0) {
        let x = f.keys.span(n).0;
        if x > area.min.x {
            wp.rect_filled(Rect::from_min_size(pos2(x, area.min.y), vec2(1.0, area.height())), 0.0, t.grid);
            out.octave_lines.push(x);
        }
    }
    // The drag's preview values and their check (made again only when the edits change).
    let preview = drag.as_ref().filter(|d| !d.cancelled).map(|d| g::edits(state, d));
    if let Some(e) = &preview {
        if conflict.as_ref().is_none_or(|(was, _)| was != e) {
            *conflict = Some((e.clone(), if e.is_empty() { None } else { g::conflict(state, e, inputs.session, inputs.names) }));
        }
    } else {
        *conflict = None;
    }
    let dragged = drag.as_ref().filter(|d| !d.cancelled).map(|d| d.zone);
    for z in drawn(state, inputs.selected) {
        let moved = preview.as_ref().and_then(|e| e.iter().find(|e| e.zone == z.zone)).copied();
        let shown = moved.map_or_else(|| z.clone(), |e| Zone { keys: e.keys, velocities: e.velocities, root: e.root, ..z.clone() });
        let rec = zone(&wp, state, &f, &shown, inputs, Some(z.zone) == inputs.selected || Some(z.zone) == dragged);
        if Some(z.zone) == dragged {
            let hot = conflict.as_ref().and_then(|(_, c)| c.clone());
            if hot.is_some() {
                let r = super::super::grid_rect(rec.rect);
                dashed_rect(&wp, r, Stroke::new(tk::CONFLICT_WIDTH, color(tk::CONFLICT_EDGE)));
            }
            out.preview = Some((rec.rect, hot));
        }
        out.zones.push(rec);
    }
    // Held keys: a column over the zones.
    for n in inputs.held.iter().copied().filter(|n| (lo..=hi).contains(n)) {
        let (x, w) = f.keys.span(n);
        let col = Rect::from_min_max(pos2(x, area.min.y), pos2(x + w, area.max.y));
        wp.rect_filled(col, 0.0, t.signal.gamma_multiply(tk::HELD_COLUMN_ALPHA));
        out.held.push(r4(col));
    }
    let keyboard =
        Rect::from_min_max(pos2(area.min.x, well.max.y + tk::KEYBOARD_GAP), pos2(area.max.x, well.max.y + tk::KEYBOARD_GAP + tk::KEYBOARD_HEIGHT));
    out.keyboard = r4(keyboard);
    keys(&p, state, &f, keyboard, inputs, &mut out);
    let intent = input(ui, uid, state, &f, drag, inputs.names, &out);
    (out, intent)
}

fn dashed_rect(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let r = r.shrink(stroke.width / 2.0);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    let [dash, gap] = tk::CANDIDATE_DASH;
    p.extend(Shape::dashed_line(&pts, stroke, dash, gap));
}

/// One zone.
fn zone(p: &egui::Painter, state: &State, f: &Frame, z: &Zone, inputs: &Inputs, selected: bool) -> ZoneRecord {
    let t = &TOKENS;
    let r = f.rect_of(state, z);
    let pad = z.pad;
    let (fill, edge, edge_width, dashed, ink, dot_ink) = if selected {
        (
            t.signal.gamma_multiply(tk::SELECTED_FILL_ALPHA),
            color(tk::SELECTED_BORDER),
            tk::SELECTED_BORDER_WIDTH,
            false,
            color(tk::SELECTED_INK),
            color(tk::SELECTED_ROOT_DOT_INK),
        )
    } else if pad.is_some() {
        (color(tk::PAD_ZONE_FILL), color(tk::PAD_ZONE_BORDER), chrome::FIELD_EDGE_WIDTH, true, color(tk::PAD_ZONE_INK), color(tk::PAD_ZONE_DOT))
    } else {
        (color(tk::ZONE_FILL), color(tk::ZONE_BORDER), chrome::FIELD_EDGE_WIDTH, false, color(tk::ZONE_INK), color(tk::ROOT_DOT_INK))
    };
    let dashed = dashed || (selected && pad.is_some());
    let zr = tk::ZONE_RADIUS;
    // Edge fades (the widths playback uses): the fill runs down to the floor across them.
    let fades = z.fades_used.all();
    let fade = (pad.is_none() && fades != [0; 4]).then(|| {
        let inner = f.fade_inner(z, r, fades);
        fade_fill(p, r, inner, fades, fill);
        (r4(inner), tk::FADE_FLOOR_ALPHA)
    });
    if fade.is_none() {
        p.rect_filled(r, zr, fill);
    }
    if dashed {
        dashed_rect(p, r, Stroke::new(edge_width, edge));
    } else {
        p.rect_stroke(r, zr, Stroke::new(edge_width, edge), StrokeKind::Inside);
    }
    let zp = p.with_clip_rect(r.intersect(p.clip_rect()));
    let text = label(state, z, inputs.names);
    let [lx, ly] = tk::ZONE_LABEL_INSET;
    let lr = widgets::role_text(&zp, &ZONE_LABEL, &text, ink, Align2::LEFT_TOP, pos2(r.min.x + lx, r.min.y + ly));
    let lock = pad.map(|_| {
        let s = tk::LOCK_GLYPH_SIZE;
        let at = Rect::from_min_size(pos2(r.min.x + lx, lr.max.y + SPACE[0]), vec2(s, s));
        crate::icons::paint(&zp, at.center(), crate::icons::Icon::Lock, s, color(tk::LOCK_GLYPH_COLOR));
        r4(at)
    });
    let rr = z.membership.and_then(|_| {
        let n = state.layer_peers(z).len();
        (n > 1).then(|| {
            let text = format!("RR {n}");
            let size = widgets::role_size(&zp, &RR_TAG, &text) + vec2(2.0 * tk::RR_TAG_PADDING_X + 2.0 * chrome::FIELD_EDGE_WIDTH, 0.0);
            let tr = Rect::from_min_size(pos2(r.max.x - tk::RR_TAG_INSET - size.x, r.min.y + tk::RR_TAG_INSET), size);
            zp.rect(tr, tk::RR_TAG_RADIUS, Color32::TRANSPARENT, Stroke::new(chrome::FIELD_EDGE_WIDTH, color(tk::RR_TAG_EDGE)), StrokeKind::Inside);
            widgets::role_text(&zp, &RR_TAG, &text, color(tk::RR_TAG_INK), Align2::CENTER_CENTER, tr.center());
            (text, r4(tr))
        })
    });
    let root = pad.and_then(|p| state.pad_note(p)).unwrap_or(z.root);
    let (kx, kw) = f.keys.span(root);
    let d = tk::ROOT_DOT;
    let cx = (kx + kw / 2.0).clamp(r.min.x + d / 2.0, (r.max.x - d / 2.0).max(r.min.x + d / 2.0));
    let dot = Rect::from_center_size(pos2(cx, r.max.y - tk::ROOT_DOT_INSET - d / 2.0), vec2(d, d));
    zp.circle_filled(dot.center(), d / 2.0, dot_ink);
    ZoneRecord {
        zone: z.zone,
        rect: r4(r),
        label: text,
        label_rect: r4(lr),
        dot: r4(dot),
        pad,
        selected,
        fill,
        edge,
        edge_width,
        dashed,
        ink,
        dot_ink,
        rr,
        lock,
        fade,
    }
}

/// The zone fill with its edge fades: a mesh over `r` whose alpha is full inside `inner` and runs
/// to `fadeFloorAlpha` at each faded edge (the lower of the two factors where both axes fade).
fn fade_fill(p: &egui::Painter, r: Rect, inner: Rect, fades: [u8; 4], fill: Color32) {
    let floor = tk::FADE_FLOOR_ALPHA;
    let xs = [r.min.x, inner.min.x, inner.max.x, r.max.x];
    let ys = [r.min.y, inner.min.y, inner.max.y, r.max.y];
    let fx = [if fades[0] > 0 { floor } else { 1.0 }, 1.0, 1.0, if fades[1] > 0 { floor } else { 1.0 }];
    let fy = [if fades[3] > 0 { floor } else { 1.0 }, 1.0, 1.0, if fades[2] > 0 { floor } else { 1.0 }];
    let mut mesh = egui::Mesh::default();
    for (j, y) in ys.iter().enumerate() {
        for (i, x) in xs.iter().enumerate() {
            mesh.colored_vertex(pos2(*x, *y), fill.gamma_multiply(fx[i].min(fy[j])));
        }
    }
    for j in 0..3u32 {
        for i in 0..3u32 {
            let a = j * 4 + i;
            mesh.add_triangle(a, a + 1, a + 5);
            mesh.add_triangle(a, a + 5, a + 4);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// The keyboard: white keys (pad keys fainter), octave names on the Cs, black keys over them;
/// held keys in `signal`.
fn keys(p: &egui::Painter, state: &State, f: &Frame, kb: Rect, inputs: &Inputs, out: &mut KeyMapLayout) {
    let pad_notes: Vec<u8> = state.zones.iter().filter_map(|z| z.pad.and_then(|p| state.pad_note(p))).collect();
    let kp = p.with_clip_rect(kb);
    let held = |n: u8| inputs.held.contains(&n);
    let wr = tk::WHITE_KEY_RADIUS as u8;
    for n in (f.keys.lo..=f.keys.hi).filter(|n| !is_black(*n)) {
        let (x, w) = f.keys.span(n);
        let r = Rect::from_min_max(pos2(x, kb.min.y), pos2(x + w, kb.max.y));
        let fill = if held(n) {
            TOKENS.signal
        } else if pad_notes.contains(&n) {
            color(tk::PAD_KEY)
        } else {
            color(tk::WHITE_KEY)
        };
        // One pixel wider so neighbors share an edge.
        let drawn = Rect::from_min_max(r.min, pos2((r.max.x + chrome::FIELD_EDGE_WIDTH).min(kb.max.x), r.max.y));
        let cr = CornerRadius { nw: 0, ne: 0, sw: wr, se: wr };
        kp.rect(drawn, cr, fill, Stroke::new(chrome::FIELD_EDGE_WIDTH, color(tk::KEY_EDGE)), StrokeKind::Inside);
        let name = (n % 12 == 0).then(|| {
            let text = transit_midi::note_name(n, inputs.names);
            let nr = widgets::role_text(
                &kp,
                &OCTAVE_NAME,
                &text,
                color(tk::OCTAVE_NAME_INK),
                Align2::CENTER_BOTTOM,
                pos2(r.center().x, r.max.y - tk::OCTAVE_NAME_INSET),
            );
            (text, r4(nr))
        });
        out.keys.push(KeyRecord { note: n, rect: r4(r), fill, name });
    }
    let br = tk::BLACK_KEY_RADIUS as u8;
    for n in (f.keys.lo..=f.keys.hi).filter(|n| is_black(*n)) {
        let (x, w) = f.keys.span(n);
        let r = Rect::from_min_size(pos2(x, kb.min.y), vec2(w, tk::BLACK_KEY_HEIGHT));
        let fill = if held(n) { TOKENS.signal } else { color(tk::BLACK_KEY) };
        let cr = CornerRadius { nw: 0, ne: 0, sw: br, se: br };
        kp.rect(r, cr, fill, Stroke::new(chrome::FIELD_EDGE_WIDTH, color(tk::BLACK_KEY_EDGE)), StrokeKind::Inside);
        out.keys.push(KeyRecord { note: n, rect: r4(r), fill, name: None });
    }
}

/// The zone drawn topmost at `at`, with the part a press there holds.
fn hit(out: &KeyMapLayout, at: egui::Pos2) -> Option<(&ZoneRecord, Option<Part>)> {
    let g = super::super::grid_rect;
    let z = out.zones.iter().rev().find(|z| g(z.rect).contains(at))?;
    Some((z, part_at(g(z.rect), g(z.dot), at)))
}

/// The map's pointer input: a click selects the zone under it; a drag on an edge or the dot of a
/// zone that is not pad-bound previews and, let go, asks for one map edit.
fn input(
    ui: &mut Ui,
    uid: u64,
    state: &State,
    f: &Frame,
    drag: &mut Option<Drag>,
    names: transit_midi::MiddleC,
    out: &KeyMapLayout,
) -> Option<Intent> {
    let resp = ui.interact(f.area, egui::Id::new(("sampler_key_map", uid)), Sense::click_and_drag());
    let mut intent = None;
    if let Some(at) = resp.hover_pos()
        && drag.is_none()
        && let Some((z, part)) = hit(out, at)
    {
        if let Some(pad) = z.pad {
            resp.clone().on_hover_text(pad_tip(state, pad, names));
        } else if let Some(part) = part {
            ui.ctx().set_cursor_icon(match part {
                Part::Low | Part::High => egui::CursorIcon::ResizeHorizontal,
                Part::Top => egui::CursorIcon::ResizeVertical,
                Part::Root => egui::CursorIcon::Grab,
            });
        }
    }
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
        && let Some((z, _)) = hit(out, at)
    {
        intent = Some(Intent::Select(z.zone));
    }
    if resp.drag_started()
        && let Some(at) = ui.input(|i| i.pointer.press_origin())
        && let Some((z, part)) = hit(out, at)
    {
        intent = Some(Intent::Select(z.zone));
        if z.pad.is_some() && ui.input(|i| i.modifiers.alt) {
            intent = Some(Intent::Ask(z.zone));
        }
        if let (None, Some(part), Some(zs)) = (z.pad, part, state.zone(z.zone)) {
            *drag = Some(Drag { zone: z.zone, part, value: value_of(zs, part), cancelled: false });
        }
    }
    if resp.dragged()
        && let (Some(d), Some(at)) = (drag.as_mut(), resp.interact_pointer_pos())
    {
        d.value = match d.part {
            Part::Top => f.velocity_at(at.y),
            _ => f.keys.at(at.x),
        };
    }
    if resp.drag_stopped()
        && let Some(d) = drag.take()
        && !d.cancelled
    {
        let e = g::edits(state, &d);
        if !e.is_empty() {
            intent = Some(Intent::Map(e));
        }
    }
    intent
}

/// Why a pad zone's edges do not drag (its tooltip).
pub fn pad_tip(state: &State, pad: u8, names: transit_midi::MiddleC) -> String {
    let id = super::super::pad_id(pad - 1);
    let note = state.pad_note(pad).map(|n| transit_midi::note_name(n, names)).unwrap_or_default();
    format!("Pad {id}'s zone plays on the pad's note, {note}: its edges and root do not drag. Change the note in the Pads view.")
}
