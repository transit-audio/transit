//! The rack device (ADR 0034, #197): the first card of a rack's device chain, as Ableton's
//! Instrument Rack shows its macro knobs and its chain list with the zone editor.
//!
//! - **Macros.** Eight knobs (drag, Shift for fine, double-click for 0). A knob move is
//!   `rack.macro`, merged per knob so a drag is one undo step, and writes every mapped parameter.
//!   Right-click: map a member's parameter (instrument or insert, by the automation picker's
//!   table), edit a mapping's range (min, max, inverted when min > max) and curve, unmap it, and
//!   MIDI Learn the knob itself.
//! - **Members.** One row per member: its name, its key zone bar (0 to 127) and its velocity zone
//!   bar. Drag in the lower half of a bar to move the nearer edge, in the upper half to move the
//!   nearer fade handle; the ramps are drawn as slopes, as in Ableton's zone editor. A drag is one
//!   `rack.zone` undo step.
//! - **Play.** The rack's input monitor: while it is on, the rack takes live MIDI and plays its
//!   members (`mix.input_monitor`; the R button records arm the same route).

use crate::StudioApp;
use crate::theme::{self, TOKENS};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;
use transit_dsp::{ParamInfo, Taper, Unit};
use transit_model::{AutoParam, Control, RACK_MACROS, Track, TrackId, Zone};

/// The card's width and its parts.
pub const CARD_W: f32 = 600.0;
const HEAD_H: f32 = 20.0;
const KNOB_W: f32 = 50.0;
const KNOB_H: f32 = 46.0;
const ROW_H: f32 = 18.0;
const NAME_W: f32 = 92.0;
const KEY_W: f32 = 170.0;
const VEL_W: f32 = 110.0;

/// What the rack card drew (the control channel and the tests read it).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RackLayout {
    pub rect: crate::arrange::R4,
    pub play: crate::arrange::R4,
    /// Macro knobs, in macro order.
    pub macros: Vec<crate::arrange::R4>,
    /// Per member: its track id, its key zone bar and its velocity zone bar.
    pub members: Vec<(u64, crate::arrange::R4, crate::arrange::R4)>,
}

static MACRO_INFO: ParamInfo =
    ParamInfo { id: "macro", name: "Macro", min: 0.0, max: 1.0, default: 0.0, unit: Unit::Percent, taper: Taper::Linear, choices: &[], group: "" };

/// The card's height for rack `t`: the knobs or the member rows, whichever is taller.
pub fn card_height(s: &transit_model::Session, t: &Track) -> f32 {
    let rows = s.rack_members(t).count().max(1) as f32;
    HEAD_H + (2.0 * KNOB_H).max(rows * ROW_H + 14.0) + 6.0
}

fn r4(r: Rect) -> crate::arrange::R4 {
    crate::arrange::r4(r)
}

/// The x of value `v` (0..127) in `bar`.
fn x_of(bar: Rect, v: u8) -> f32 {
    bar.min.x + bar.width() * f32::from(v.min(127)) / 127.0
}

/// The value (0..127) at `x` in `bar`.
pub(crate) fn value_at(bar: Rect, x: f32) -> u8 {
    (((x - bar.min.x) / bar.width().max(1.0)) * 127.0).round().clamp(0.0, 127.0) as u8
}

/// Which handle a drag starting at `pos` in `bar` takes: the lower half moves the nearer edge
/// (`low`/`high`), the upper half the nearer fade handle (`fade_low`/`fade_high`).
pub(crate) fn handle_at(bar: Rect, z: Zone, pos: Pos2) -> &'static str {
    let v = value_at(bar, pos.x);
    let near_low = |a: u8, b: u8| v.abs_diff(a) <= v.abs_diff(b);
    if pos.y < bar.center().y {
        if near_low(z.fade_low, z.fade_high) { "fade_low" } else { "fade_high" }
    } else if near_low(z.low, z.high) {
        "low"
    } else {
        "high"
    }
}

/// Draws the rack card at `r` and handles its gestures. `t` is the rack track.
pub fn card(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, lane: Rect, t: &Track, r: Rect) -> RackLayout {
    let mut out = RackLayout { rect: r4(r), ..RackLayout::default() };
    let pc = p.with_clip_rect(lane);
    pc.rect_filled(r, theme::RADIUS, TOKENS.panel_raised);
    pc.rect_stroke(r, theme::RADIUS, Stroke::new(1.0, TOKENS.line), egui::StrokeKind::Inside);
    // Header: the badge, the rack's name, the Play toggle.
    let badge = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 4.0), vec2(32.0, 13.0));
    pc.rect_filled(badge, theme::RADIUS_SM, TOKENS.accent);
    pc.text(badge.center(), Align2::CENTER_CENTER, "Rack", FontId::proportional(9.5), theme::text_on(TOKENS.accent));
    pc.text(pos2(badge.max.x + 6.0, badge.center().y), Align2::LEFT_CENTER, &t.name, FontId::proportional(12.0), TOKENS.text);
    let play = Rect::from_min_size(pos2(r.max.x - 52.0, r.min.y + 3.0), vec2(46.0, 15.0));
    out.play = r4(play);
    let on = t.mixer.input_monitor;
    let resp = ui.interact(play.intersect(lane), ui.id().with(("rack_play", t.id.0)), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::DeviceChainRackPlay);
    pc.rect_filled(play, theme::RADIUS_SM, if on { TOKENS.monitor } else if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    pc.text(play.center(), Align2::CENTER_CENTER, "Play", FontId::proportional(10.5), if on { theme::text_on(TOKENS.monitor) } else { TOKENS.text });
    if resp.clicked() {
        let _ = app.run("mix.input_monitor", json!({"tracks": [t.id.0], "value": !on}));
    }
    // The macros: two rows of four.
    let rack = t.rack.clone().unwrap_or_default();
    let top = r.min.y + HEAD_H + 2.0;
    for i in 0..RACK_MACROS {
        let cell = Rect::from_min_size(pos2(r.min.x + 6.0 + (i % 4) as f32 * KNOB_W, top + (i / 4) as f32 * KNOB_H), vec2(KNOB_W, KNOB_H));
        out.macros.push(r4(cell));
        if cell.min.x >= lane.max.x {
            continue;
        }
        let value = rack.macros.get(i).map_or(0.0, |m| m.value);
        let mapped = rack.macros.get(i).is_some_and(|m| !m.mappings.is_empty());
        macro_knob(app, ui, &pc, lane, t.id, i, &rack.macro_name(i), value, mapped, cell);
    }
    // The members and their zones.
    let x0 = r.min.x + 6.0 + 4.0 * KNOB_W + 12.0;
    let s = app.engine.session_arc();
    pc.text(pos2(x0 + NAME_W, top - 1.0), Align2::LEFT_BOTTOM, "Key", FontId::proportional(9.5), TOKENS.text_dim);
    pc.text(pos2(x0 + NAME_W + KEY_W + 8.0, top - 1.0), Align2::LEFT_BOTTOM, "Velocity", FontId::proportional(9.5), TOKENS.text_dim);
    let mut y = top + 2.0;
    let members: Vec<(Track, transit_model::RackMember)> = s.rack_members(t).map(|(m, z)| (m.clone(), z.clone())).collect();
    if members.is_empty() {
        pc.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, "Drag instrument tracks into the rack", FontId::proportional(11.0), TOKENS.text_dim);
    }
    for (m, z) in members {
        let row = Rect::from_min_size(pos2(x0, y), vec2(NAME_W + KEY_W + VEL_W + 16.0, ROW_H - 2.0));
        pc.rect_filled(Rect::from_min_size(row.min, vec2(4.0, row.height())), 0.0, theme::rgb(m.color));
        pc.with_clip_rect(Rect::from_min_size(row.min, vec2(NAME_W - 4.0, row.height())).intersect(lane)).text(
            pos2(row.min.x + 8.0, row.center().y),
            Align2::LEFT_CENTER,
            &m.name,
            FontId::proportional(11.0),
            TOKENS.text,
        );
        let key_bar = Rect::from_min_size(pos2(row.min.x + NAME_W, row.min.y), vec2(KEY_W, row.height()));
        let vel_bar = Rect::from_min_size(pos2(key_bar.max.x + 8.0, row.min.y), vec2(VEL_W, row.height()));
        zone_bar(app, ui, &pc, lane, t.id, m.id, "key", z.key_zone, key_bar, theme::rgb(m.color));
        zone_bar(app, ui, &pc, lane, t.id, m.id, "velocity", z.vel_zone, vel_bar, theme::rgb(m.color));
        out.members.push((m.id.0, r4(key_bar), r4(vel_bar)));
        y += ROW_H;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn macro_knob(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, lane: Rect, rack: TrackId, i: usize, name: &str, value: f32, mapped: bool, cell: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cell.intersect(lane)));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let k = crate::widgets::knob::show(&mut child, egui::Id::new(("rack_macro", rack.0, i)), name, &MACRO_INFO, value);
    crate::help::hover(ui, &k.response, crate::help::Id::DeviceChainRackMacro);
    if mapped {
        p.circle_filled(pos2(cell.max.x - 8.0, cell.min.y + 4.0), 2.5, TOKENS.accent);
    }
    let control = Control::Macro(u8::try_from(i).unwrap_or(0));
    crate::midi_learn::mark(app, p, k.rect, rack, control.clone());
    if let Some(v) = k.value {
        let key = format!("rack_macro:{}:{i}", rack.0);
        if let Err(e) = app.engine.execute_merged("rack.macro", &json!({"track": rack.0, "macro": i + 1, "value": v}), &key) {
            app.status = e.to_string();
        }
    }
    if k.response.drag_stopped() {
        app.engine.end_merge();
    }
    k.response.context_menu(|ui| macro_menu(app, ui, rack, i, control));
}

/// The knob's right-click menu: the mappings with their range and curve, Map, MIDI Learn.
fn macro_menu(app: &mut StudioApp, ui: &mut Ui, rack: TrackId, i: usize, control: Control) {
    let s = app.engine.session_arc();
    let Some(t) = s.track(rack) else { return };
    let Some(r) = t.rack.as_ref() else { return };
    ui.label(egui::RichText::new(r.macro_name(i)).strong());
    let mappings = r.macros.get(i).map(|m| m.mappings.clone()).unwrap_or_default();
    for m in &mappings {
        let member = s.track(m.track).map_or_else(|| format!("track {}", m.track.0), |x| x.name.clone());
        let slot = if m.slot == AutoParam::INSTRUMENT_SLOT { "Instrument".to_string() } else { format!("Insert {}", slot_letter(m.slot)) };
        ui.label(format!("{member} > {slot} > {}", m.param));
        ui.horizontal(|ui| {
            let (mut min, mut max, mut curve) = (m.min, m.max, m.curve);
            let a = ui.add(egui::DragValue::new(&mut min).range(0.0..=1.0).speed(0.005).prefix("min "));
            let b = ui.add(egui::DragValue::new(&mut max).range(0.0..=1.0).speed(0.005).prefix("max "));
            let c = ui.add(egui::DragValue::new(&mut curve).range(-1.0..=1.0).speed(0.01).prefix("curve "));
            if a.changed() || b.changed() || c.changed() {
                let slot = slot_text(m.slot);
                let key = format!("rack_map:{}:{i}:{}:{}:{}", rack.0, m.track.0, m.slot, m.param);
                let p = json!({"track": rack.0, "macro": i + 1, "member": m.track.0, "slot": slot, "param": m.param, "min": min, "max": max, "curve": curve});
                if let Err(e) = app.engine.execute_merged("rack.macro_map", &p, &key) {
                    app.status = e.to_string();
                }
            }
            if ui.small_button("Unmap").clicked() {
                let p = json!({"track": rack.0, "macro": i + 1, "member": m.track.0, "slot": slot_text(m.slot), "param": m.param});
                let _ = app.run("rack.macro_unmap", p);
                ui.close();
            }
        });
    }
    if !mappings.is_empty() {
        ui.separator();
    }
    ui.menu_button("Map Parameter", |ui| {
        for (member, _) in s.rack_members(t) {
            ui.menu_button(&member.name, |ui| {
                let devices = member
                    .instrument
                    .iter()
                    .map(|d| (AutoParam::INSTRUMENT_SLOT, d))
                    .chain(member.mixer.inserts.iter().enumerate().filter_map(|(k, d)| d.as_ref().map(|d| (u8::try_from(k).unwrap_or(0), d))));
                for (slot, d) in devices {
                    let Some(info) = crate::automation::device_info(&d.plugin) else { continue };
                    let title = if slot == AutoParam::INSTRUMENT_SLOT { info.name.to_string() } else { format!("{} ({})", info.name, slot_letter(slot)) };
                    ui.menu_button(title, |ui| {
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for p in info.params {
                                if ui.button(p.name).clicked() {
                                    let args = json!({"track": rack.0, "macro": i + 1, "member": member.id.0, "slot": slot_text(slot), "param": p.id});
                                    let _ = app.run("rack.macro_map", args);
                                    ui.close();
                                }
                            }
                        });
                    });
                }
            });
        }
    });
    ui.separator();
    app.midi_learn_menu(ui, rack, control);
}

fn slot_letter(slot: u8) -> char {
    char::from(b'A'.saturating_add(slot.min(25)))
}

fn slot_text(slot: u8) -> String {
    if slot == AutoParam::INSTRUMENT_SLOT { "instrument".into() } else { slot_letter(slot).to_ascii_lowercase().to_string() }
}

/// One zone bar: the zone filled, the fade ranges as slopes, the four handles; a drag moves one.
#[allow(clippy::too_many_arguments)]
fn zone_bar(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, lane: Rect, rack: TrackId, member: TrackId, which: &str, z: Zone, bar: Rect, color: Color32) {
    let z = z.normalized();
    p.rect_filled(bar, theme::RADIUS_SM, TOKENS.bg);
    let (xl, xfl, xfh, xh) = (x_of(bar, z.low), x_of(bar, z.fade_low), x_of(bar, z.fade_high), x_of(bar, z.high));
    // Ableton's look: full height between the fade handles, slopes down to the edges.
    let fill = color.gamma_multiply(0.75);
    let (t, b) = (bar.min.y + 2.0, bar.max.y - 1.0);
    p.add(Shape::convex_polygon(vec![pos2(xl, b), pos2(xfl, t), pos2(xfh, t), pos2(xh, b)], fill, Stroke::NONE));
    for x in [xfl, xfh] {
        p.line_segment([pos2(x, t), pos2(x, t + 4.0)], Stroke::new(2.0, TOKENS.text));
    }
    for x in [xl, xh] {
        p.line_segment([pos2(x, b - 4.0), pos2(x, b)], Stroke::new(2.0, TOKENS.text));
    }
    let id = ui.id().with(("rack_zone", rack.0, member.0, which));
    let resp = ui.interact(bar.intersect(lane), id, Sense::click_and_drag());
    crate::help::hover(ui, &resp, crate::help::Id::DeviceChainRackZone);
    let tip = format!("{} {}..{} (fades to {} and from {})", if which == "key" { "Keys" } else { "Velocities" }, z.low, z.high, z.fade_low, z.fade_high);
    let resp = resp.on_hover_text(tip);
    let handle_id = id.with("handle");
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        ui.data_mut(|d| d.insert_temp(handle_id, handle_at(bar, z, pos)));
    }
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(handle) = ui.data(|d| d.get_temp::<&'static str>(handle_id))
    {
        let v = value_at(bar, pos.x);
        let current = match handle {
            "low" => z.low,
            "high" => z.high,
            "fade_low" => z.fade_low,
            _ => z.fade_high,
        };
        if v != current {
            let key = format!("rack_zone:{}:{}:{which}", rack.0, member.0);
            let args = json!({"track": rack.0, "member": member.0, which: {handle: v}});
            if let Err(e) = app.engine.execute_merged("rack.zone", &args, &key) {
                app.status = e.to_string();
            }
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<&'static str>(handle_id));
        app.engine.end_merge();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_in_the_lower_half_takes_an_edge_and_in_the_upper_half_a_fade_handle() {
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(127.0, 16.0));
        let z = Zone { low: 20, fade_low: 30, fade_high: 90, high: 100 };
        assert_eq!(handle_at(bar, z, pos2(22.0, 12.0)), "low");
        assert_eq!(handle_at(bar, z, pos2(97.0, 12.0)), "high");
        assert_eq!(handle_at(bar, z, pos2(33.0, 3.0)), "fade_low");
        assert_eq!(handle_at(bar, z, pos2(80.0, 3.0)), "fade_high");
        assert_eq!(value_at(bar, -5.0), 0);
        assert_eq!(value_at(bar, 500.0), 127);
        assert_eq!(value_at(bar, 64.0), 64);
    }
}
