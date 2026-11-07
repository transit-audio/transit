//! One channel strip in the design language (`docs/design/language`, Components > Channel strip),
//! top to bottom: the 3 px track color bar, name and number, input (and MIDI OUT), inserts and
//! sends (or the short variant's `FX n` / `Snd n`), output, the 36 px bipolar pan knob with its
//! value, the M S / R I toggles, the Channel fader with its scale and the stereo meter, and the
//! gain and peak readouts. The sections have fixed heights ([`super::anatomy`]) so every fader
//! lines up across strips; the selected strip has a 1 px `signal` border.

use super::anatomy::{self, Anatomy};
use super::*;
use crate::tokens::{self, TypeRole, channel_strip as cs};
use crate::widgets::{fader as kit_fader, knob, toggle};

/// The track name: Sans 600 13.
pub(super) const NAME: TypeRole = TypeRole { size: 13.0, ..tokens::TITLE_SECTION_BAR };
/// The track number: Mono 400 10.
pub(super) const NUMBER: TypeRole = TypeRole { size: 10.0, ..tokens::READOUT_DISPLAY };
/// Dropdown text, insert and send names, the short buttons: Sans 500 11.
pub(super) const CONTROL: TypeRole = TypeRole { size: 11.0, ..tokens::TITLE_DEVICE };
/// The INSERTS and SENDS labels: the section label at 10.
pub(super) const SECTION: TypeRole = TypeRole { size: 10.0, ..tokens::LABEL_SECTION };
/// Small mono figures (a send level, a count): Mono 500 10.
pub(super) const SMALL_VALUE: TypeRole = TypeRole { size: 10.0, ..tokens::VALUE };
/// The gain field: Mono 500 12.
pub(super) const GAIN: TypeRole = TypeRole { size: 12.0, ..tokens::VALUE };

/// Paints `text` in `role` inside `rect`, cut with an ellipsis when it does not fit; returns where
/// the text sits.
pub(super) fn text_in(p: &egui::Painter, role: &TypeRole, text: &str, color: Color32, rect: Rect, align: Align2) -> Rect {
    let mut job = role.job(p.ctx(), text, color);
    job.wrap = egui::text::TextWrapping { max_width: rect.width().max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let at = align.anchor_size(align.pos_in_rect(&rect), galley.size());
    p.galley(at.min, galley, color);
    at
}

/// A boxed tag (`tag` type, 1 px `line.strong`, radius 4, 6 px padding) ending at `right`,
/// centered on `row`; returns its rect.
pub(super) fn tag(p: &egui::Painter, row: Rect, right: f32, text: &str, ink: Color32) -> Rect {
    let w = crate::widgets::role_size(p, &tokens::TAG, text).x + 12.0;
    let r = Rect::from_min_max(pos2(right - w, row.min.y), pos2(right, row.max.y));
    p.rect_stroke(r, tokens::radius::SEGMENT, Stroke::new(1.0, TOKENS.line_strong), egui::StrokeKind::Inside);
    crate::widgets::role_text(p, &tokens::TAG, text, ink, Align2::CENTER_CENTER, r.center());
    r
}

/// The track color bar's fill: the palette's `bar` value of the hue nearest the track's RGB
/// ([`super::hue`], the bridge until tracks carry a palette name), or Graphite when the RGB is
/// grey or there is no track (the main-out strip without a master).
pub(super) fn bar_color(rgb: Option<[u8; 3]>) -> Color32 {
    super::hue::bar_for(rgb.unwrap_or([128, 128, 128]))
}

/// The color bar's outline: the bar rect `bar` (inside the strip's 1 px border) cut by the panel's
/// rounded rect, so its top corners follow the border's inner edge (radius 8 less the border, 7)
/// instead of egui's corner radius, which a 3 px bar clamps to half its height. Clockwise from the
/// bottom-left; convex.
pub(super) fn bar_outline(strip: Rect, bar: Rect) -> Vec<egui::Pos2> {
    let r = tokens::radius::PANEL - 1.0;
    let cy = strip.min.y + tokens::radius::PANEL;
    // The arc's x inset from the bar's edge at height `y` (zero below the corner).
    let inset = |y: f32| {
        let dy = (cy - y).max(0.0);
        if dy >= r { r } else { r - (r * r - dy * dy).sqrt() }
    };
    const STEPS: usize = 6;
    let ys: Vec<f32> = (0..=STEPS).map(|i| bar.max.y - (bar.max.y - bar.min.y) * i as f32 / STEPS as f32).collect();
    let mut pts: Vec<egui::Pos2> = ys.iter().map(|&y| pos2(bar.min.x + inset(y), y)).collect();
    pts.extend(ys.iter().rev().map(|&y| pos2(bar.max.x - inset(y), y)));
    pts
}

/// Paints the color bar inside the panel's rounded corners; returns its outline.
pub(super) fn paint_bar(p: &egui::Painter, strip: Rect, bar: Rect, color: Color32) -> Vec<[f32; 2]> {
    let pts = bar_outline(strip, bar);
    p.add(egui::Shape::convex_polygon(pts.clone(), color, Stroke::NONE));
    pts.iter().map(|q| [q.x, q.y]).collect()
}

/// One channel strip in `r` (clipped to `clip`): a track's, or the master's (the main bus).
#[allow(clippy::too_many_arguments)]
pub(super) fn strip(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    r: Rect,
    clip: Rect,
    focused: bool,
    short: bool,
    number: &str,
    menu: &mut Vec<MenuItem>,
) -> StripLayout {
    let id = t.id;
    let m = &t.mixer;
    let main = t.kind == TrackKind::Master;
    // Under Read automation the controls show the lane's value at the playhead (no command, no edit).
    let vol_db = app.followed(t, &crate::automation::Lane::Volume).unwrap_or(m.volume_db);
    let bal = app.followed(t, &crate::automation::Lane::Pan).unwrap_or_else(|| balance_of(&m.pan));
    let a = anatomy::anatomy(r, short, main);
    let p = ui.painter().with_clip_rect(clip);
    p.rect_filled(r, tokens::radius::PANEL, TOKENS.surface_panel);
    let bar = bar_color(Some(t.color));
    let outline = paint_bar(&p, r, a.bar, bar);
    let mut sl = StripLayout {
        track: id.0,
        name: t.name.clone(),
        number: number.to_string(),
        kind: Some(t.kind),
        rect: r4(r),
        color_bar: r4(a.bar),
        color_bar_outline: outline,
        color_bar_color: bar.to_array(),
        short,
        volume_db: vol_db,
        balance: bal,
        pan: m.pan.clone(),
        level: app.meters.get(&id).map_or([0.0; 2], |d| d.level),
        fx: t.kind == TrackKind::Aux,
        focused,
        folder: t.folder.map(|f| f.0),
        ..StripLayout::default()
    };
    name_row(app, ui, &p, clip, t, &a, &mut sl, menu);
    input_row(app, ui, &p, clip, s, t, &a, &mut sl, menu);
    if short {
        super::inserts::fx_button(app, ui, s, t, a.fx_button.unwrap_or(Rect::NOTHING), clip, &mut sl, menu);
        super::sends::snd_button(app, ui, s, t, a.snd_button.unwrap_or(Rect::NOTHING), clip, &mut sl, menu);
    } else {
        for line in [a.inserts_line, a.sends_line].into_iter().flatten() {
            p.rect_filled(line, 0.0, TOKENS.line_inset);
        }
        if let Some(body) = a.inserts {
            sl.inserts_section = r4(body);
            text_in(&p, &SECTION, "Inserts", TOKENS.text_faint, anatomy::section_label(body), Align2::LEFT_CENTER);
            inserts_block(app, ui, t, body, clip, &mut sl, menu);
        }
        if let Some(body) = a.sends {
            sl.sends_section = r4(body);
            text_in(&p, &SECTION, "Sends", TOKENS.text_faint, anatomy::section_label(body), Align2::LEFT_CENTER);
            sends_block(app, ui, s, t, body, clip, &mut sl, menu);
        }
    }
    output_row(app, ui, clip, s, t, a.output, &mut sl, menu);
    pan(app, ui, &p, clip, t, &a, bal, &mut sl);
    toggles(app, ui, &p, clip, t, &a, &mut sl);
    fader_and_meter(app, ui, &p, clip, t, &a, vol_db, &mut sl);
    if let Some(footer) = a.footer {
        super::main_bus::footer(app, ui, &p, clip, footer, &mut sl);
    }
    // The border last, over everything: 1 px `line`, `signal` on the selected strip.
    let edge = if focused { TOKENS.signal } else { TOKENS.line };
    p.rect_stroke(r, tokens::radius::PANEL, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    sl.border_color = edge.to_array();
    sl
}

/// Name and number; tags for an inactive track, a rack and an FX channel. Anywhere on the strip
/// focuses its track; a double-click on the name renames it in place.
#[allow(clippy::too_many_arguments)]
fn name_row(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, a: &Anatomy, sl: &mut StripLayout, menu: &mut Vec<MenuItem>) {
    let id = t.id;
    let r = rect(sl.rect);
    let row = a.name;
    let num = text_in(p, &NUMBER, &sl.number, TOKENS.text_faint, row, Align2::RIGHT_CENTER);
    let mut right = num.min.x - 6.0;
    let tag_row = row.shrink2(vec2(0.0, 1.0));
    if t.inactive {
        let r = tag(p, tag_row, right, "inactive", TOKENS.text_muted);
        sl.inactive_tag = r4(r);
        right = r.min.x - 4.0;
    }
    if t.is_rack() {
        // A rack (ADR 0034).
        let r = tag(p, tag_row, right, "Rack", TOKENS.text_muted);
        let resp = ui.interact(r.intersect(clip), ui.id().with(("rack_tag", t.id.0)), Sense::hover());
        crate::help::hover(ui, &resp, crate::help::Id::MixerStripRack);
        right = r.min.x - 4.0;
    }
    if sl.fx {
        let r = tag(p, tag_row, right, "FX", TOKENS.text_muted);
        right = r.min.x - 4.0;
    }
    // A muted or inactive channel dims its name.
    let ink = if t.inactive || t.mixer.mute { TOKENS.text_faint } else { TOKENS.text };
    let name_r = Rect::from_min_max(row.min, pos2(right, row.max.y));
    text_in(p, &NAME, &t.name, ink, name_r, Align2::LEFT_CENTER);
    sl.name_rect = r4(row);
    sl.name_color = ink.to_array();
    // Anywhere on the strip focuses it; controls drawn after this take their own clicks.
    let resp = ui.interact(r.intersect(clip), ui.id().with(("strip_name", id.0)), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripName);
    if resp.clicked() {
        let shift = ui.input(|i| i.modifiers.shift);
        app.focus_header(id, shift);
    }
    // The inactive tag lies over the strip, so it is interacted after it: a click is Make Active (#333).
    let tag_resp = t.inactive.then(|| {
        let tag_resp = ui.interact(rect(sl.inactive_tag).intersect(clip), ui.id().with(("inactive_tag", id.0)), Sense::click());
        crate::help::hover(ui, &tag_resp, crate::help::Id::MixerStripInactive);
        let tag_resp = tag_resp.on_hover_text("Make Active");
        if tag_resp.clicked() {
            let _ = app.run("track.make_inactive", json!({"tracks": [id.0], "inactive": false}));
        }
        tag_resp
    });
    // Slice 6: double-click the name to rename the track in place.
    if resp.double_clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| row.contains(p)) {
        app.start_rename(crate::finish::RenameTarget::Track(id), r4(row));
    }
    let mut track_menu = |ui: &mut Ui| {
        if t.kind == TrackKind::Audio {
            crate::arrange::width_item(app, ui, id);
        }
        menu.push(crate::menus::activity_item(app, ui, Some(id)));
    };
    resp.context_menu(&mut track_menu);
    // The tag covers part of the name, so it gets the same menu on right-click.
    if let Some(tag_resp) = &tag_resp {
        egui::Popup::context_menu(tag_resp).id(tag_resp.id.with("strip-menu")).show(&mut track_menu);
    }
}

/// The input dropdown: hardware or bus input for audio tracks and FX channels; the MIDI input
/// for MIDI and instrument tracks, with their MIDI OUT button (#61) and MIDI effects tag (#201).
#[allow(clippy::too_many_arguments)]
fn input_row(
    app: &mut StudioApp,
    ui: &mut Ui,
    p: &egui::Painter,
    clip: Rect,
    s: &Session,
    t: &Track,
    a: &Anatomy,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    let id = t.id;
    let m = &t.mixer;
    let in_r = a.input;
    if matches!(t.kind, TrackKind::Audio | TrackKind::Aux) {
        let cur = route_label(s, &m.input, true, if t.kind == TrackKind::Audio { t.channels() } else { 0 });
        let mut choices = input_choices_with(s, t, app.device_inputs(), app.engine.io_map(), app.system_audio_on());
        push_unused_entry(s, &mut choices);
        if let Some(v) = dropdown(ui, clip, in_r, ui.id().with(("input", id.0)), &cur, None, &choices, menu, false) {
            if v == REMOVE_UNUSED {
                let _ = app.remove_unused_busses();
            } else {
                let _ = app.set_input(id, &v);
            }
        }
        sl.input = r4(in_r);
        sl.input_label = cur;
    } else if t.kind == TrackKind::Master {
        // The main bus has no input: the row says where it plays, the output path and the device
        // it feeds (the Mixer board's "Out 1–2 · Speakers"), in Mono 11 `text.faint`.
        let text = main_destination(app, s, t);
        text_in(p, &MAIN_DESTINATION, &text, TOKENS.text_faint, in_r.shrink2(vec2(2.0, 0.0)), Align2::LEFT_CENTER);
        sl.input = r4(in_r);
        sl.input_label = text;
    } else if t.kind.is_midi() {
        let out_r = Rect::from_min_max(pos2(in_r.max.x - cs::MIDI_OUT_WIDTH, in_r.min.y), in_r.max);
        let mut in_r = Rect::from_min_max(in_r.min, pos2(out_r.min.x - cs::GAP, in_r.max.y));
        midi_output_button(app, ui, p, clip, t, out_r, sl);
        if !t.midi_effects.is_empty() {
            // The MIDI effects ahead of the instrument (#201): a tag, their chain on hover.
            let all_off = t.midi_effects.iter().all(|f| f.bypass);
            let r = tag(p, in_r.shrink2(vec2(0.0, 3.0)), in_r.max.x, "MFX", if all_off { TOKENS.ink_arrow } else { TOKENS.text_muted });
            let chain: Vec<String> = t
                .midi_effects
                .iter()
                .map(|f| {
                    let name = crate::midi_fx_panel::name_of(&f.plugin);
                    if f.bypass { format!("{name} (bypassed)") } else { name }
                })
                .collect();
            let resp = ui.interact(r.intersect(clip), ui.id().with(("midi_fx_tag", id.0)), Sense::hover());
            crate::help::hover(ui, &resp, crate::help::Id::MixerStripMidiEffects);
            resp.on_hover_text(format!("MIDI effects: {}", chain.join(" → ")));
            sl.midi_fx = r4(r);
            sl.midi_fx_count = t.midi_effects.len();
            in_r.max.x = r.min.x - cs::GAP;
        }
        let view = crate::midi_input::InputView::read(app.player.as_ref());
        let label = view.short_label(&t.midi_input);
        let resp = ui.interact(in_r.intersect(clip), ui.id().with(("midi-input", id.0)), Sense::click());
        paint_dropdown(p, in_r, &label, resp.hovered(), None, false);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "MIDI input"));
        if let Some(params) = view.popup(&resp, id, &t.midi_input) {
            let _ = app.run("midi.input", params);
        }
        sl.input = r4(in_r);
        sl.input_label = label;
    }
}

/// The main bus's destination line: Mono 11.
const MAIN_DESTINATION: TypeRole = TypeRole { size: 11.0, ..tokens::READOUT_DISPLAY };

/// The output path the main bus plays on, by name: the I/O map's Main path for the main mix, a
/// named path as it is.
fn main_path(app: &StudioApp, s: &Session, t: &Track) -> String {
    match &t.mixer.output {
        Route::Main => app.engine.io_map().main.clone(),
        other => route_label(s, other, false, t.channels()),
    }
}

/// "Out 1-2 · Speakers": the main bus's output path and the device the audio system opened for
/// it (what the Player recorded or the fake's listing says); the path alone without a device.
fn main_destination(app: &StudioApp, s: &Session, t: &Track) -> String {
    let path = main_path(app, s, t);
    match app.opened().output {
        Some(device) => format!("{path} · {device}"),
        None => path,
    }
}

/// The output dropdown.
#[allow(clippy::too_many_arguments)]
fn output_row(app: &mut StudioApp, ui: &mut Ui, clip: Rect, s: &Session, t: &Track, out_r: Rect, sl: &mut StripLayout, menu: &mut Vec<MenuItem>) {
    let m = &t.mixer;
    // The main bus reads the path it plays on ("Out 1-2"), not the main mix's own name.
    let cur = if t.kind == TrackKind::Master { main_path(app, s, t) } else { route_label(s, &m.output, false, t.channels()) };
    let mut choices = output_choices(s, t, app.engine.io_map());
    push_unused_entry(s, &mut choices);
    let tip = unresolved_output(app.engine.io_map(), &m.output).then_some(NOT_HERE);
    if let Some(v) = dropdown(ui, clip, out_r, ui.id().with(("output", t.id.0)), &cur, tip, &choices, menu, true) {
        if v == REMOVE_UNUSED {
            let _ = app.remove_unused_busses();
        } else {
            let _ = app.set_output(t.id, &v);
        }
    }
    sl.output = r4(out_r);
    sl.output_arrow = r4(super::menus::arrow_rect(out_r));
    sl.output_label = cur;
}

/// The pan knob (36, bipolar) and its mono value beside it.
#[allow(clippy::too_many_arguments)]
fn pan(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, a: &Anatomy, bal: f32, sl: &mut StripLayout) {
    let id = t.id;
    let has_pan = !t.mixer.pan.is_empty();
    let me = Target::Pan(id);
    let resp = ui.interact(a.dial.intersect(clip), ui.id().with(("knob", id.0)), Sense::click_and_drag());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripPan);
    let held = resp.hovered() || app.mixer.gesture.is_some_and(|g| g.target == me);
    let state = if !has_pan {
        knob::State::Disabled
    } else if held {
        knob::State::Active
    } else {
        knob::State::Default
    };
    sl.knob = r4(a.dial);
    sl.knob_paint = Some(knob::paint_dial(p, a.dial, (bal + 1.0) / 2.0, true, None, state));
    if has_pan {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        control(app, ui, &resp, me, 1.0, true, true);
        resp.context_menu(|ui| app.midi_learn_menu(ui, id, transit_model::Control::Pan));
        crate::midi_learn::mark(app, p, a.dial, id, transit_model::Control::Pan);
    }
    let pan_text = fine::pan_text(bal);
    let pan_r = a.pan_value;
    if has_pan {
        readout(
            app,
            ui,
            p,
            clip,
            Readout {
                target: me,
                salt: 0,
                draw: pan_r,
                hit: pan_r,
                text: pan_text.clone(),
                edit_text: pan_text.clone(),
                font: tokens::VALUE.font_id(ui.ctx()),
                color: TOKENS.text,
                align: Align2::LEFT_CENTER,
                drag: false,
                click_to_type: true,
            },
        );
    } else {
        text_in(p, &tokens::VALUE, &pan_text, TOKENS.text_faint, pan_r, Align2::LEFT_CENTER);
    }
    sl.pan_readout_rect = r4(pan_r);
    sl.pan_text = pan_text;
}

/// M S / R I: the kit's toggles. The main bus has M, Mono, Dim and Ref ([`super::main_bus`]); an
/// unavailable toggle stays, ghosted, so every strip keeps its layout.
#[allow(clippy::too_many_arguments)]
fn toggles(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, a: &Anatomy, sl: &mut StripLayout) {
    let id = t.id;
    let m = &t.mixer;
    let main = t.kind == TrackKind::Master;
    use toggle::Role;
    let cells: [(&str, Role, bool, &str, bool); 4] = [
        ("M", Role::Mute, m.mute, "mix.mute", true),
        ("S", Role::Solo, m.solo, "mix.solo", !main),
        ("R", Role::Arm, m.record_arm, "mix.record_arm", !main && t.kind.has_playlist()),
        ("I", Role::Monitor, m.input_monitor, "mix.input_monitor", !main && t.kind == TrackKind::Audio),
    ];
    for (i, (label, role, on, cmd, enabled)) in cells.into_iter().enumerate() {
        if main && i > 0 {
            // The main bus has no solo, record or input: its other cells are the monitoring set.
            continue;
        }
        let br = a.toggles[i];
        let resp = ui.interact(br.intersect(clip), ui.id().with((cmd, id.0, "strip")), if enabled { Sense::click() } else { Sense::hover() });
        let lay = toggle::paint(p, br, label, role, on, enabled);
        crate::help::hover(
            ui,
            &resp,
            match label {
                "M" => crate::help::Id::MixerStripMute,
                "S" => crate::help::Id::MixerStripSolo,
                "R" => crate::help::Id::MixerStripArm,
                _ => crate::help::Id::MixerStripMonitor,
            },
        );
        if resp.clicked() && enabled {
            let _ = app.run(cmd, json!({"tracks": [id.0], "value": !on}));
        }
        let learnable = match label {
            "M" => Some(transit_model::Control::Mute),
            "S" => Some(transit_model::Control::Solo),
            "R" => Some(transit_model::Control::Arm),
            _ => None,
        };
        if let Some(control) = learnable.filter(|_| enabled) {
            resp.context_menu(|ui| app.midi_learn_menu(ui, id, control.clone()));
            crate::midi_learn::mark(app, p, br, id, control);
        }
        match label {
            "M" => sl.mute = r4(br),
            "S" => sl.solo = r4(br),
            "R" => sl.arm = r4(br),
            _ => sl.monitor = r4(br),
        }
        sl.toggles.push(lay);
    }
    if main {
        super::main_bus::monitor_toggles(app, ui, p, clip, [a.toggles[1], a.toggles[2], a.toggles[3]], sl);
    }
}

/// The Channel fader with its scale, the stereo meter in its meter slot, and the gain and peak
/// readouts under them.
#[allow(clippy::too_many_arguments)]
fn fader_and_meter(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, a: &Anatomy, vol_db: f32, sl: &mut StripLayout) {
    let id = t.id;
    let main = t.kind == TrackKind::Master;
    let lay = kit_fader::paint(p, a.fader, kit_fader::Variant::Channel, vol_db, t.mixer.mute);
    let travel = rect(lay.track);
    let column = Rect::from_min_max(a.fader.min, pos2(a.fader.min.x + kit_fader::CHANNEL_FADER_W, a.fader.max.y));
    let me = Target::Fader(id);
    let resp = ui.interact(column.intersect(clip), ui.id().with(("fader", id.0)), Sense::click_and_drag());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripFader);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    control(app, ui, &resp, me, travel.height().max(1.0), true, true);
    resp.context_menu(|ui| app.midi_learn_menu(ui, id, transit_model::Control::Volume));
    crate::midi_learn::mark(app, p, column, id, transit_model::Control::Volume);
    // The meter, in the fader's meter slot.
    let d = app.meters.get(&id).copied().unwrap_or_default();
    let slot = lay.meter.map_or(Rect::NOTHING, rect);
    let meter = strip_meter(p, slot.min.x, travel, &d, main);
    let wells = meter_bounds(&meter);
    let resp = ui.interact(wells.intersect(clip), ui.id().with(("meter", id.0)), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripMeter);
    if resp.on_hover_text("Click to clear the clip latch and the peak hold, Ctrl-click for every strip").clicked() {
        let all = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        app.reset_meters(if all { None } else { Some(id) });
    }
    let cap_line = rect(lay.center_line);
    let zero = meter.grid.get(1).copied().map(rect);
    let span = |r: Rect| {
        let ppp = ui.ctx().pixels_per_point();
        [(r.min.y * ppp).round() as i32, (r.max.y * ppp).round() as i32]
    };
    (sl.cap_line_y, sl.cap_line_span) = (Some(cap_line.center().y), Some(span(cap_line)));
    (sl.zero_tick_y, sl.zero_tick_span) = (zero.map(|z| z.center().y), zero.map(span));
    sl.fader = r4(travel);
    sl.fader_layout = Some(lay);
    sl.meter = r4(wells);
    // Gain: Mono 12 in an inset field, click to type.
    let fader_hot = app.mixer.gesture.is_some_and(|g| g.target == me);
    p.rect_filled(a.gain, tokens::radius::CONTROL, TOKENS.surface_inset);
    p.rect_stroke(a.gain, tokens::radius::CONTROL, Stroke::new(1.0, TOKENS.line), egui::StrokeKind::Inside);
    let text = fine::minus(fine::db_text(vol_db));
    readout(
        app,
        ui,
        p,
        clip,
        Readout {
            target: me,
            salt: 0,
            draw: a.gain,
            hit: a.gain,
            text: text.clone(),
            edit_text: fine::edit_text_db(vol_db),
            font: GAIN.font_id(ui.ctx()),
            color: if fader_hot { TOKENS.signal } else { TOKENS.text },
            align: Align2::CENTER_CENTER,
            drag: false,
            click_to_type: true,
        },
    );
    sl.readout_rect = r4(a.gain);
    sl.volume_text = text;
    // Peak: Mono 11 under the meter, click to reset; `meter.hot` above 0 dBFS or while latched.
    peak_readout(app, ui, p, clip, a.peak, d.peak, d.clip, Some(id), sl);
    sl.meter_layout = Some(meter);
}

/// The peak readout: the held peak in Mono 11 (`text.muted`, `meter.hot` above 0 dBFS or while the
/// clip latch is lit) in a 1 px `line.inset` box; a click resets the strip's (`None`: the main
/// meter's), Ctrl-click every strip's.
#[allow(clippy::too_many_arguments)]
pub(super) fn peak_readout(
    app: &mut StudioApp,
    ui: &mut Ui,
    p: &egui::Painter,
    clip: Rect,
    r: Rect,
    held: f32,
    latched: bool,
    id: Option<TrackId>,
    sl: &mut StripLayout,
) {
    let (text, over) = peak_text(held);
    let hot = over || latched;
    let resp = ui.interact(r.intersect(clip), ui.id().with(("peak", id.map_or(u64::MAX, |t| t.0))), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripPeak);
    p.rect_stroke(r, tokens::radius::CONTROL, Stroke::new(1.0, TOKENS.line_inset), egui::StrokeKind::Inside);
    let ink = if hot {
        TOKENS.meter_hot
    } else if resp.hovered() {
        TOKENS.text
    } else {
        TOKENS.text_muted
    };
    text_in(p, &tokens::VALUE, &text, ink, r, Align2::CENTER_CENTER);
    if resp.on_hover_text("Peak hold. Click to reset, Ctrl-click to reset all").clicked() {
        let all = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        match (all, id) {
            (true, _) => app.reset_meters(None),
            (false, Some(id)) => app.reset_meters(Some(id)),
            (false, None) => app.reset_main_meter(),
        }
    }
    sl.peak_rect = r4(r);
    sl.peak_text = text;
    sl.peak_color = ink.to_array();
}

/// The rect of a layout's `[min x, min y, max x, max y]`.
pub(super) fn rect(r: R4) -> Rect {
    Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
}
