use super::*;

/// The bottom panes: device chain (lowest) and the mixer above it. Call before the central panel.
pub fn panes(app: &mut StudioApp, ui: &mut Ui) {
    app.mixer.layout.menu.clear();
    app.mixer.devices.menu.clear();
    app.mixer.filter_rect = Default::default();
    app.mixer.hover = None;
    app.mixer.wheel_taken = false;
    if app.mixer.edit.is_none() {
        app.mixer.edit_rect = [0.0; 4];
    }
    app.mixer.devices.visible = app.ui.show_devices;
    if app.ui.show_devices {
        egui::Panel::bottom("devices")
            .exact_size(crate::devices::pane_height(app).max(crate::midi_fx_panel::pane_height(app)))
            .frame(egui::Frame::NONE.fill(TOKENS.panel))
            .show(ui, |ui| crate::devices::chain(app, ui));
    }
    app.mixer.layout.visible = app.ui.show_mixer || app.ui.mixer_full;
    if app.ui.show_mixer && !app.ui.mixer_full {
        let h = app.ui.mixer_height.clamp(min_pane_height(), 600.0);
        egui::Panel::bottom("mixer").exact_size(h).frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| strips(app, ui));
    }
    crate::keys::switcher_panel(app, ui);
}

/// The mixer filling the window (F3).
pub fn show_full(app: &mut StudioApp, ui: &mut Ui) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| strips(app, ui));
}

/// The docked pane's margins above and below its strips (4 each) and the panel's 1 px edge.
const PANE_MARGINS: f32 = 9.0;

/// The docked pane's smallest height: the short strip's sections and a fader with
/// [`MIN_FADER`] of travel, so the readouts never leave the pane (#326; 379 points).
pub fn min_pane_height() -> f32 {
    PANE_MARGINS + anatomy::fixed_height(true) + MIN_FADER + 2.0 * crate::tokens::fader::INSET
}

/// Whether a strip in a lane `height` tall keeps its sections in order: the readouts below the
/// toggles, and the fader [`MIN_FADER`] of travel between them.
pub fn strip_fits(height: f32, short: bool) -> bool {
    let a = anatomy::anatomy(Rect::from_min_size(pos2(0.0, 0.0), vec2(STRIP_W, height)), short, false);
    let travel = a.fader.height() - 2.0 * crate::tokens::fader::INSET;
    a.gain.min.y >= a.toggles[3].max.y && travel >= MIN_FADER - 0.01 && a.gain.max.y <= height
}

/// Whether strips in a lane `height` tall take the short variant: when the full sections would
/// leave the fader less than [`MIN_FADER`] of travel (the docked pane, normally; the full mixer
/// window shows them full height).
pub(super) fn short_for(height: f32) -> bool {
    height - anatomy::fixed_height(false) < MIN_FADER + 2.0 * crate::tokens::fader::INSET
}

fn strips(app: &mut StudioApp, ui: &mut Ui) {
    let area = ui.max_rect();
    let p = ui.painter();
    p.line_segment([area.left_top(), area.right_top()], Stroke::new(1.0, TOKENS.line));
    let s = app.engine.session_arc();
    let focus = app.chosen_track();
    let master = s.tracks.iter().find(|t| t.kind == TrackKind::Master && !t.hidden);
    // Stable partition of the visible strips only: the document/arranger order never changes.
    // Routing folders are tracks and stay inline with their members, not in the aux block.
    let (mut channels, aux): (Vec<&Track>, Vec<&Track>) =
        s.tracks.iter().filter(|t| t.kind != TrackKind::Master && !t.hidden && !app.tucked(&s, t)).partition(|t| t.kind != TrackKind::Aux);
    let track_count = channels.len();
    let block_gap = if track_count > 0 && !aux.is_empty() { FX_GAP } else { 0.0 };
    channels.extend(aux);
    // A track's number is its place in the document among the tracks that are not the master.
    let numbers: std::collections::HashMap<TrackId, usize> =
        s.tracks.iter().filter(|t| t.kind != TrackKind::Master).enumerate().map(|(i, t)| (t.id, i + 1)).collect();
    // The main bus strip is pinned right; the others scroll under it.
    let main_w = anatomy::width(true);
    let master_rect = Rect::from_min_max(pos2(area.max.x - main_w - 6.0, area.min.y + 4.0), pos2(area.max.x - 6.0, area.max.y - 4.0));
    let lane = Rect::from_min_max(
        pos2(area.min.x + 4.0, area.min.y + 4.0),
        pos2(master_rect.min.x - crate::tokens::channel_strip::GAP_MAIN, area.max.y - 4.0),
    );
    let short = short_for(lane.height());
    let content_w = channels.len() as f32 * (STRIP_W + GAP) + block_gap + 110.0;
    app.mixer.hscroll = app.mixer.hscroll.clamp(0.0, (content_w - lane.width()).max(0.0));
    let mut layout = MixerLayout { visible: true, area: r4(area), short, menu: std::mem::take(&mut app.mixer.layout.menu), ..MixerLayout::default() };
    // Empty mixer space deselects, like empty space in the arranger. Registered first so every
    // strip and control above it wins the click.
    if ui.interact(area, ui.id().with("mixer_empty"), Sense::click()).clicked() {
        app.deselect_all();
    }
    for (i, t) in channels.iter().enumerate() {
        let x = lane.min.x + i as f32 * (STRIP_W + GAP) + if i >= track_count { block_gap } else { 0.0 } - app.mixer.hscroll;
        let r = Rect::from_min_size(pos2(x, lane.min.y), vec2(STRIP_W, lane.height()));
        if r.max.x < lane.min.x || r.min.x > lane.max.x {
            continue;
        }
        let number = numbers.get(&t.id).map_or(String::new(), ToString::to_string);
        let sl = strip(app, ui, &s, t, r, lane, focus == Some(t.id), short, &number, &mut layout.menu);
        layout.strips.push(sl);
    }
    layout.groups = group_links(ui, &s, &channels[..track_count], lane, area, app.mixer.hscroll);
    if block_gap > 0.0 {
        let x = lane.min.x + track_count as f32 * (STRIP_W + GAP) + block_gap / 2.0 - app.mixer.hscroll;
        let divider = Rect::from_min_max(pos2(x, lane.min.y), pos2(x + 1.0, lane.max.y)).intersect(lane);
        if divider.is_positive() {
            ui.painter().with_clip_rect(lane).rect_filled(divider, 0.0, TOKENS.line);
            layout.fx_divider = Some(r4(divider));
        }
    }
    // "Add FX Channel" after the last strip.
    let ax = lane.min.x + channels.len() as f32 * (STRIP_W + GAP) + block_gap - app.mixer.hscroll + 6.0;
    let add = Rect::from_min_size(pos2(ax, lane.min.y + 4.0), vec2(96.0, 24.0));
    if add.min.x < lane.max.x {
        let resp = ui.interact(add.intersect(lane), ui.id().with("add_fx"), Sense::click());
        let p = ui.painter().with_clip_rect(lane);
        p.rect_filled(add, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        crate::icons::paint_labeled(&p, add, false, crate::icons::Icon::Plus, 12.0, "FX Channel", FontId::proportional(11.5), TOKENS.text);
        if resp.on_hover_text("A new FX channel (add a send to it from a strip's + Send)").clicked() {
            let _ = app.add_fx_channel();
        }
    }
    layout.add_fx = r4(add);
    // The main bus.
    match master {
        Some(m) => {
            let sl = strip(app, ui, &s, m, master_rect, master_rect, focus == Some(m.id), short, "M", &mut layout.menu);
            layout.master = Some(sl);
        }
        None => {
            let (sl, add) = main_out(app, ui, master_rect, short);
            layout.master = Some(sl);
            layout.add_master = add;
        }
    }
    // Wheel over the strips scrolls them sideways.
    if let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        && lane.contains(pos)
        && ui.rect_contains_pointer(lane)
        && app.mixer.gesture.is_none()
        && !app.mixer.wheel_taken
    {
        let d = ui.input(|i| i.smooth_scroll_delta);
        let d = if d.x.abs() > d.y.abs() { d.x } else { d.y };
        if d.abs() > 0.1 {
            app.mixer.hscroll = (app.mixer.hscroll - d).clamp(0.0, (content_w - lane.width()).max(0.0));
        }
    }
    app.mixer.layout = layout;
}

/// Above the strips, a thin accent bar from each group's strip over its members' strips.
fn group_links(ui: &Ui, s: &Session, channels: &[&Track], lane: Rect, area: Rect, hscroll: f32) -> Vec<GroupLink> {
    let x = |i: usize| lane.min.x + i as f32 * (STRIP_W + GAP) - hscroll;
    let p = ui.painter().with_clip_rect(Rect::from_min_max(pos2(lane.min.x, area.min.y), pos2(lane.max.x, lane.min.y)));
    let mut out = Vec::new();
    for (i, g) in channels.iter().enumerate().filter(|(_, t)| t.is_folder()) {
        let members: Vec<u64> = channels[i + 1..]
            .iter()
            .take_while(|t| crate::groups::within(s, t.id, g.id))
            .filter(|t| t.folder == Some(g.id))
            .map(|t| t.id.0)
            .collect();
        let last = channels[i + 1..].iter().take_while(|t| crate::groups::within(s, t.id, g.id)).count();
        if last == 0 {
            continue;
        }
        let r = Rect::from_min_max(pos2(x(i) + 2.0, area.min.y + 1.0), pos2(x(i + last) + STRIP_W - 2.0, area.min.y + 3.0));
        p.rect_filled(r, crate::theme::RADIUS_SM, TOKENS.accent);
        out.push(GroupLink { folder: g.id.0, members, rect: r4(r) });
    }
    out
}

/// The main-out strip when the session has no master track: the main bus strip's frame with the
/// main meter in its meter slot, its peak readout, and an offer to add a master fader.
fn main_out(app: &mut StudioApp, ui: &mut Ui, r: Rect, short: bool) -> (StripLayout, R4) {
    use crate::tokens;
    let a = anatomy::anatomy(r, short, true);
    let p = ui.painter().clone();
    p.rect_filled(r, tokens::radius::PANEL, TOKENS.surface_panel);
    let bar = channel::bar_color(None);
    let outline = channel::paint_bar(&p, r, a.bar, bar);
    channel::text_in(&p, &channel::NUMBER, "M", TOKENS.text_faint, a.name, Align2::RIGHT_CENTER);
    channel::text_in(&p, &channel::NAME, "Main", TOKENS.text, a.name, Align2::LEFT_CENTER);
    let add = a.input;
    let resp = ui.interact(add, ui.id().with("add_master"), Sense::click());
    inserts::add_row(&p, add, "+ Master Fader", resp.hovered());
    if resp.on_hover_text("Add a master fader").clicked() {
        let _ = app.run("track.new", json!({"kind": "master"}));
    }
    // The meter where the main bus fader's meter sits, so it lines up with the strips beside it.
    let travel = crate::widgets::fader::track_rect(crate::widgets::fader::Variant::Channel, a.fader);
    let x = a.fader.min.x + crate::widgets::fader::CHANNEL_FADER_W + crate::widgets::fader::CHANNEL_SCALE_W + 6.0;
    let d = app.main_meter;
    let meter = strip_meter(&p, x, travel, &d, true);
    let wells = meter_bounds(&meter);
    let on_meter = ui.interact(wells, ui.id().with("main_meter_click"), Sense::click());
    if on_meter.clicked() {
        let all = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        if all {
            app.reset_meters(None);
        } else {
            app.reset_main_meter();
        }
    }
    let mut sl = StripLayout {
        name: "Main".into(),
        number: "M".into(),
        rect: r4(r),
        color_bar: r4(a.bar),
        color_bar_outline: outline,
        color_bar_color: bar.to_array(),
        short,
        meter: r4(wells),
        level: app.main_meter.level,
        ..StripLayout::default()
    };
    channel::peak_readout(app, ui, &p, r, a.peak, d.peak, d.clip, None, &mut sl);
    sl.meter_layout = Some(meter);
    p.rect_stroke(r, tokens::radius::PANEL, Stroke::new(1.0, TOKENS.line), egui::StrokeKind::Inside);
    sl.border_color = TOKENS.line.to_array();
    (sl, r4(add))
}
