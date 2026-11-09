use super::*;

/// The piano roll pane at the bottom of the window. Returns true when it is shown (it then
/// replaces the mixer and device chain panes). Call before the central panel.
pub fn pane(app: &mut StudioApp, ui: &mut Ui) -> bool {
    // A deleted clip (or a new session) closes the editor; with several open, the next one takes
    // the foreground and the others stay.
    let gone = |app: &StudioApp, c: ClipId| clip_view(app.engine.session(), c).is_none();
    app.keys.others.retain(|c| clip_view(app.engine.session(), *c).is_some());
    if let Some(c) = app.keys.clip
        && gone(app, c)
    {
        app.keys.clip = None;
        app.keys.selection.clear();
        app.keys.gesture = None;
        if !app.keys.others.is_empty() {
            let next = app.keys.others.remove(0);
            app.keys.clip = Some(next);
            app.keys.fit = true;
            // Nothing of the deleted clip's ticks carries into the next one's.
            app.keys.click_tick = None;
            app.keys.step = StepState::default();
            app.keys.cc.cancel();
            app.keys.length_edit = None;
        }
    }
    let now = ui.input(|i| i.time);
    let down = ui.input(|i| i.pointer.any_down());
    // Focus lost: nothing may keep sounding.
    let blurred = ui.input(|i| i.viewport().focused == Some(false));
    if blurred {
        app.keys_focus_cancel();
    }
    app.release_auditions(if blurred { f64::INFINITY } else { now }, down && !blurred);
    if !app.keys_shown() {
        app.keys.quantize_open = false;
        // Hidden by a tab switch or a closed pane: let go of everything.
        app.release_auditions(f64::INFINITY, false);
        app.keys.layout = KeysLayout { auditions: app.keys.auditions, last_audition: app.keys.last_audition, ..KeysLayout::default() };
        return false;
    }
    app.mixer.layout.visible = false;
    app.mixer.devices.visible = false;
    let max_h = (ui.ctx().content_rect().height() - 200.0).max(MIN_H);
    let h = app.ui.keys.height.clamp(MIN_H, max_h);
    egui::Panel::bottom("keys").exact_size(h).frame(egui::Frame::NONE.fill(TOKENS.panel)).show(ui, |ui| editor(app, ui));
    quantize::window(app, ui.ctx());
    // A pending note-off needs a frame to happen on, even with the mouse still; held MIDI keys repaint at ~60 Hz.
    if !app.keys.held.is_empty() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    if !app.live_keys().is_empty() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }
    true
}

fn editor(app: &mut StudioApp, ui: &mut Ui) {
    let area = ui.max_rect();
    let Some(clip) = app.keys.clip else { return };
    let s = app.engine.session_arc();
    let Some(cv) = clip_view(&s, clip) else { return };
    let Some(all) = Composite::new(&s, clip, &app.keys.others) else { return };
    let now = ui.input(|i| i.time);
    // The pane takes the keyboard after a press inside it, and gives it up after one outside.
    if let Some(p) = ui.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None }) {
        app.keys.active = area.contains(p);
    }
    let mut layout = KeysLayout {
        visible: true,
        clip: Some(clip.0),
        track: Some(cv.track.0),
        pane: r4(area),
        notes: Vec::new(),
        focus: all.several().then_some(app.keys.focus),
        ..KeysLayout::default()
    };
    let handle = Rect::from_min_max(area.min, pos2(area.max.x, area.min.y + HANDLE_H));
    let header = Rect::from_min_max(area.min, pos2(area.max.x, area.min.y + HEADER_BAND_H));
    let narrow = area.width() < 1200.0;
    let body_top = header.max.y + if narrow { HEADER_H } else { 0.0 };
    let ruler = Rect::from_min_max(pos2(area.min.x + KEY_W, body_top), pos2(area.max.x, body_top + RULER_H));
    let vel = Rect::from_min_max(pos2(area.min.x + KEY_W, area.max.y - VEL_H), area.max);
    let grid = Rect::from_min_max(pos2(area.min.x + KEY_W, ruler.max.y), pos2(area.max.x, vel.min.y - 1.0));
    let keyboard = Rect::from_min_max(pos2(area.min.x, ruler.max.y), pos2(area.min.x + KEY_W, grid.max.y));
    let corner = Rect::from_min_max(pos2(area.min.x, body_top), pos2(area.min.x + KEY_W, ruler.max.y));
    let vel_label = Rect::from_min_max(pos2(area.min.x, vel.min.y), pos2(area.min.x + KEY_W, area.max.y));

    let prefs = app.ui.keys.clone();
    let kh = prefs.key_h.clamp(6.0, 40.0);
    let ppt = keys_ppt(&prefs);
    let step = keys_grid_step(&s, &prefs);
    // The corner has room for little: "Auto 1/16", "Grid: 1/8T", "Snap off".
    let grid_text = grid::label_at(ppt, grid::bar_ticks(&s), &prefs.grid).replace("Adaptive", "Auto");
    let rows = grid.height() / kh;
    let row_map = scale::Rows::new(app.keys_scale(), &all.notes, &app.keys.live, ui.input(|i| chromatic(i.modifiers)));
    if app.keys.fit {
        app.keys.fit = false;
        let (lo, hi) = (all.notes.iter().map(|n| n.pitch).min(), all.notes.iter().map(|n| n.pitch).max());
        let center = match (lo, hi) {
            (Some(a), Some(b)) => (f32::from(a) + f32::from(b)) / 2.0,
            _ => 60.0,
        };
        app.keys.top = row_map.pitch((row_map.rank(center) + rows / 2.0).round());
        // From the earliest open clip (the foreground's start when it is the only one).
        app.keys.scroll = all.parts.iter().map(|p| p.off).min().unwrap_or(0).min(0) as f64;
    }
    let last_row = row_map.last_rank();
    app.keys.top = row_map.pitch(row_map.rank(app.keys.top).clamp((rows - 1.0).min(last_row), last_row));
    app.keys.scroll = app.keys.scroll.max(-(cv.c0 as f64));
    let step_cur = app.step_cursor(ppt, grid.width());
    let map = Map {
        gx0: grid.min.x,
        gy0: grid.min.y,
        ppt,
        scroll: app.keys.scroll,
        top: app.keys.top,
        kh,
        c0: cv.c0,
        grid: prefs.grid.snap_enabled.then_some(step),
        step,
        rows: row_map,
        scale: app.keys_scale(),
    };
    layout.pitch_rows = (0..=127)
        .filter(|&p| map.rows.contains(p))
        .filter_map(|p| {
            let row = Rect::from_min_size(pos2(keyboard.min.x, map.y(p)), vec2(keyboard.width(), kh));
            row.intersects(keyboard).then_some((p, r4(row)))
        })
        .collect();
    // Drop selected notes that no longer exist (undo, another view).
    let present = resolve(&all.notes, &app.keys.selection);
    if present.len() != app.keys.selection.len() {
        app.keys.selection = present.iter().map(|i| all.notes[*i]).collect();
    }

    header_row(app, ui, header, &cv, &all, &mut layout);
    if narrow {
        let rect = Rect::from_min_size(pos2(area.min.x + 8.0, header.max.y), vec2(138.0, HEADER_H));
        note_inspector(app, ui, rect, &mut layout);
    }
    pane_handle(app, ui, handle, &mut layout);
    paint_grid(ui, &s, &map, grid, ruler, &cv, &all, &mut layout);
    let preview = preview(app, ui, &map, vel, &all);
    paint_notes(app, ui, &map, grid, vel, &all, preview.as_ref(), &mut layout);
    paint_loop_ghosts(ui, &map, grid, &cv, &mut layout);
    if app.keys.quantize_open {
        layout.quantize_preview = app.keys_quantize_preview();
        let painter = ui.painter().with_clip_rect(grid);
        for note in &layout.quantize_preview {
            painter.rect_stroke(map.note_rect(note), crate::theme::RADIUS_SM, Stroke::new(2.0, TOKENS.accent), StrokeKind::Inside);
        }
    }
    if let Some(chord) = app.step_preview() {
        let length = app.step_len().saturating_mul(i64::from(chord.units));
        let painter = ui.painter().with_clip_rect(grid);
        for (pitch, velocity) in chord.notes {
            let note = Note { pitch, velocity, release_velocity: 64, channel: 0, start: app.keys.step.cursor, length };
            painter.rect_filled(map.note_rect(&note), crate::theme::RADIUS_SM, TOKENS.accent.gamma_multiply(0.55));
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }
    grid_input(app, ui, &map, grid, vel, &cv, &all, now, &mut layout);
    if app.keys.cc.controller.is_none() {
        lane_input(app, ui, &map, vel, &all, &layout);
    } else {
        cc::lane(app, ui, &map, vel, &cv, &mut layout.cc);
    }
    paint_keyboard(app, ui, &map, keyboard);
    keyboard_input(app, ui, &map, keyboard, now);
    ruler_input(app, ui, &map, ruler, &s, &layout.clips);
    clip_end_handle(app, ui, &map, ruler, grid, &cv, clip, &mut layout);
    wheel(app, ui, &map, Rect::from_min_max(pos2(area.min.x, body_top), area.max));
    // Overlays: rubber band, cursor and playhead.
    let p = ui.painter().with_clip_rect(Rect::from_min_max(ruler.min, vel.max));
    if let Some(KeysGesture::Band { press, .. }) = app.keys.gesture
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        let band = Rect::from_two_pos(press, pos);
        p.rect_filled(band, 0.0, TOKENS.accent.gamma_multiply(0.15));
        p.rect_stroke(band, 0.0, Stroke::new(1.0, TOKENS.accent), StrokeKind::Inside);
    }
    let to_rel = |at: Samples| s.tempo.samples_to_ticks(at, s.sample_rate) - cv.c0;
    let cx = map.x(to_rel(app.cursor()));
    p.line_segment([pos2(cx, ruler.min.y), pos2(cx, vel.max.y)], Stroke::new(1.0, TOKENS.cursor.gamma_multiply(0.6)));
    if !app.keys.step.on
        && let Some(at) = app.keys.click_tick
    {
        let x = map.x(at.max(0));
        p.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(1.0, TOKENS.accent.gamma_multiply(0.35)));
    }
    if let Some((at, len)) = step_cur {
        let (x0, x1) = (map.x(at), map.x(at + len));
        let band = Rect::from_min_max(pos2(x0, ruler.min.y), pos2(x1.max(x0 + 2.0), vel.max.y));
        p.rect_filled(band, 0.0, TOKENS.accent.gamma_multiply(0.18));
        p.line_segment([pos2(x0, ruler.min.y), pos2(x0, vel.max.y)], Stroke::new(2.0, TOKENS.accent));
        p.add(egui::Shape::convex_polygon(
            vec![pos2(x0 - 5.0, ruler.min.y), pos2(x0 + 5.0, ruler.min.y), pos2(x0, ruler.min.y + 8.0)],
            TOKENS.accent,
            Stroke::NONE,
        ));
        layout.step_cursor = at;
        layout.step_ticks = len;
    }
    if let Some(delay) = step::step_repaint_delay(step_cur.is_some(), app.keys.step.busy()) {
        ui.ctx().request_repaint_after(delay);
    }
    layout.step_on = step_cur.is_some();
    if app.is_playing() {
        let rel = to_rel(app.engine.transport.position);
        let px = map.x(rel);
        p.line_segment([pos2(px, ruler.min.y), pos2(px, vel.max.y)], Stroke::new(1.5, TOKENS.accent));
        // Page the view so the playhead stays on screen; the editor stays on its clip.
        let w = f64::from(grid.width()) / ppt;
        if app.keys.gesture.is_none() && ((rel as f64) > app.keys.scroll + w * 0.97 || (rel as f64) < app.keys.scroll) {
            app.keys.scroll = rel as f64 - w * 0.05;
        }
        ui.ctx().request_repaint();
    }
    let lp = ui.painter();
    lp.rect_filled(corner, 0.0, TOKENS.ruler);
    let cp = lp.with_clip_rect(corner);
    match layout.live.last() {
        Some((pitch, _)) => {
            cp.text(corner.center(), Align2::CENTER_CENTER, transit_midi::note_name(*pitch), FontId::proportional(12.0), TOKENS.accent)
        }
        None => cp.text(corner.center(), Align2::CENTER_CENTER, &grid_text, FontId::proportional(9.5), TOKENS.text_dim),
    };
    lp.rect_filled(vel_label, 0.0, TOKENS.panel);
    lp.line_segment([pos2(area.min.x, vel.min.y - 0.5), pos2(area.max.x, vel.min.y - 0.5)], Stroke::new(1.0, TOKENS.line));
    cc::selector(app, ui, vel_label, &mut layout.cc);

    layout.active = app.keys.active;
    layout.ruler = r4(ruler);
    layout.keyboard = r4(keyboard);
    layout.grid = r4(grid);
    layout.velocity = r4(vel);
    layout.px_per_tick = ppt;
    layout.scroll_ticks = app.keys.scroll;
    layout.top_pitch = app.keys.top;
    layout.key_h = kh;
    layout.clip_start_ticks = cv.c0;
    layout.clip_ticks = cv.len;
    layout.looped_to_ticks = cv.cycle.map(|_| cv.full);
    layout.loop_cycle = cv.cycle;
    layout.lead_in_ticks = cv.cycle.map_or(0, |(a, _)| a.max(0));
    layout.grid_ticks = map.grid;
    layout.grid_step = step;
    layout.grid_text = grid_text;
    layout.selected = app.keys.selection.len();
    layout.auditions = app.keys.auditions;
    layout.last_audition = app.keys.last_audition;
    layout.sounding = app.keys.held.iter().map(|h| h.1).collect();
    layout.live = app.live_keys();
    app.keys.layout = layout;
}

/// The Piano Roll / Mixer / Devices tabs, left to right from `r`'s left edge, `on` marking the
/// views shown. Returns where the tabs end, their rectangles and the tab clicked.
fn switcher(ui: &mut Ui, r: Rect, on: [bool; 3]) -> (f32, [R4; 3], Option<usize>) {
    let mut x = r.min.x + 8.0;
    let font = FontId::proportional(12.0);
    let mut rects = [R4::default(); 3];
    let mut clicked = None;
    for (which, label) in ["Piano Roll", "Mixer", "Devices"].into_iter().enumerate() {
        let w = ui.painter().layout_no_wrap(label.into(), font.clone(), TOKENS.text).size().x + 16.0;
        let b = Rect::from_min_size(pos2(x, r.min.y + 3.0), vec2(w, r.height() - 6.0));
        x += w + 2.0;
        let resp = ui.interact(b, ui.id().with(("keys_tab", which)), Sense::click());
        let on = on[which];
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(b, crate::theme::RADIUS, TOKENS.button_hover);
        }
        p.text(b.center(), Align2::CENTER_CENTER, label, font.clone(), if on { TOKENS.text } else { TOKENS.text_dim });
        if on {
            p.line_segment([pos2(b.min.x + 6.0, b.max.y), pos2(b.max.x - 6.0, b.max.y)], Stroke::new(2.0, TOKENS.accent));
        }
        rects[which] = r4(b);
        if resp.clicked() {
            clicked = Some(which);
        }
    }
    (x, rects, clicked)
}

/// The switcher strip above the mixer and device panes (the piano roll draws its own in its
/// header), so the three views switch the same way from each. Call after the panes are added.
pub fn switcher_panel(app: &mut StudioApp, ui: &mut Ui) {
    app.keys.switcher = Default::default();
    // The strip stays when nothing is shown, so the panes are one click away; the full mixer and
    // the piano roll bring their own.
    if app.ui.mixer_full || app.keys_shown() {
        return;
    }
    egui::Panel::bottom("bottom_switcher").exact_size(HEADER_H).frame(egui::Frame::NONE.fill(TOKENS.panel)).show(ui, |ui| {
        let r = ui.max_rect();
        let p = ui.painter();
        p.line_segment([pos2(r.min.x, r.min.y + 0.5), pos2(r.max.x, r.min.y + 0.5)], Stroke::new(1.0, TOKENS.line));
        let (_, rects, clicked) = switcher(ui, r, [false, app.ui.show_mixer, app.ui.show_devices]);
        app.keys.switcher = rects;
        match clicked {
            Some(0) => app.toggle_keys(),
            // Each tab toggles its own pane; the Mixer and Devices panes are independent and may
            // both be shown.
            Some(1) => crate::menus::toggle_mixer(&mut app.ui),
            Some(_) => crate::menus::toggle_devices(&mut app.ui),
            None => {}
        }
    });
}

/// The height of every control in the header row: the dense control height, so the buttons, the
/// velocity field and the Length field are one size on one center line (#412).
const HEADER_CONTROL_H: f32 = crate::tokens::layout::DENSE_CONTROL_HEIGHT;

/// The header's control band: [`HEADER_CONTROL_H`] high, centered on `row`'s center line.
fn control_band(row: Rect, x0: f32, x1: f32) -> Rect {
    Rect::from_center_size(pos2((x0 + x1) / 2.0, row.center().y), vec2(x1 - x0, HEADER_CONTROL_H))
}

/// Makes egui's own fields (drag value, text edit) come out [`HEADER_CONTROL_H`] high, not the
/// taller of their font and padding; use inside a style-scoped closure (the style is restored).
fn exact_height(ui: &mut Ui) {
    ui.spacing_mut().interact_size.y = HEADER_CONTROL_H;
    ui.spacing_mut().button_padding.y = 0.0;
}

/// A header field (the velocity drag value, the Length text field): the kit's field frame (the
/// inset well, a 1 px `line`, `line.strong` on hover, the focus ring) painted by us on `band`,
/// [`HEADER_CONTROL_H`] high, with egui's widget drawn bare inside it. egui sizes the frame it
/// paints from the widget's own content, so a frame left to the widget can come out shorter than
/// the allocation (#412); this one is the band, whatever the widget measures.
fn header_field(ui: &mut Ui, band: Rect, enabled: bool, add: impl FnOnce(&mut Ui, Rect) -> egui::Response) -> egui::Response {
    let ring = ui.visuals().selection.stroke;
    // The corner the kit's fields take (`chrome::field_visuals` leaves the widget corner as is).
    let radius = ui.visuals().widgets.inactive.corner_radius;
    let faded = if enabled { 1.0 } else { ui.visuals().disabled_alpha() };
    // The frame goes under the widget: reserve its place in paint order, fill it once the
    // widget's response (hover, focus) is known.
    let slot = ui.painter().add(egui::Shape::Noop);
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(band).sense(Sense::hover()), |ui| {
            if !enabled {
                ui.disable();
            }
            exact_height(ui);
            let v = ui.visuals_mut();
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.bg_fill = egui::Color32::TRANSPARENT;
                w.weak_bg_fill = egui::Color32::TRANSPARENT;
                w.bg_stroke = Stroke::NONE;
            }
            v.text_edit_bg_color = Some(egui::Color32::TRANSPARENT);
            v.selection.stroke = Stroke::NONE;
            add(ui, band)
        })
        .inner;
    let edge = if resp.has_focus() {
        ring.color
    } else if resp.hovered() || resp.dragged() {
        TOKENS.line_strong
    } else {
        TOKENS.line
    };
    let width = if resp.has_focus() { ring.width } else { 1.0 };
    ui.painter().set(
        slot,
        egui::Shape::Rect(egui::epaint::RectShape::new(
            band,
            radius,
            TOKENS.surface_inset.gamma_multiply(faded),
            Stroke::new(width, edge.gamma_multiply(faded)),
            egui::StrokeKind::Inside,
        )),
    );
    resp
}

/// Tabs (Piano Roll, Mixer, Devices), the clip's name, grid, Quantize and the audition switch.
fn header_row(app: &mut StudioApp, ui: &mut Ui, r: Rect, cv: &ClipView, all: &Composite, layout: &mut KeysLayout) {
    let p = ui.painter();
    p.rect_filled(r, 0.0, TOKENS.panel);
    p.line_segment([pos2(r.min.x, r.max.y - 0.5), pos2(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, TOKENS.line));
    let (x, rects, clicked) = switcher(ui, Rect::from_center_size(r.center(), vec2(r.width(), HEADER_H)), [true, false, false]);
    layout.tab_keys = rects[0];
    layout.tab_mixer = rects[1];
    layout.tab_devices = rects[2];
    match clicked {
        // The active Piano Roll tab closes the piano roll, back to the pane under it.
        Some(0) => app.close_keys(),
        Some(which) => {
            app.close_keys();
            if which == 1 {
                app.ui.show_mixer = true;
            } else {
                app.ui.show_devices = true;
            }
        }
        None => {}
    }
    // The clip: a separator, a swatch in the track color, then its name (not a tab).
    let (track_name, color) = app.engine.session().track(cv.track).map(|t| (t.name.clone(), t.color)).unwrap_or_default();
    let title = if cv.name.is_empty() || cv.name == track_name { track_name } else { format!("{track_name} · {}", cv.name) };
    // Several clips: how many, then the foreground's name (#329).
    let title = if all.several() { format!("{} clips · {title}", all.parts.len()) } else { title };
    // A frozen track plays its render: its notes are read-only (edits are refused) until unfrozen.
    let title = if app.engine.session().track(cv.track).is_some_and(|t| t.freeze.is_some()) {
        format!("{title} · Frozen (unfreeze to edit)")
    } else {
        title
    };
    let p = ui.painter();
    p.line_segment([pos2(x + 8.0, r.min.y + 7.0), pos2(x + 8.0, r.max.y - 7.0)], Stroke::new(1.0, TOKENS.line));
    p.rect_filled(Rect::from_center_size(pos2(x + 22.0, r.center().y), vec2(8.0, 8.0)), crate::theme::RADIUS_SM, theme::rgb(color));

    // Right side, right to left: Audition, Quantize, Grid, velocity and its eyedropper.
    let mut rx = r.max.x - 8.0;
    let mut button = |ui: &mut Ui, w: f32, salt: &str| {
        let b = control_band(r, rx - w, rx);
        rx -= w + 6.0;
        (b, ui.interact(b, ui.id().with(("keys_btn", salt)), Sense::click()))
    };
    let small = FontId::proportional(11.5);
    let (b, resp) = button(ui, 96.0, "step");
    let on = app.keys.step.on;
    let p = ui.painter();
    p.rect_filled(
        b,
        crate::theme::RADIUS_SM,
        if on {
            TOKENS.accent
        } else if resp.hovered() {
            TOKENS.button_hover
        } else {
            TOKENS.button
        },
    );
    let label = if on { format!("Step {}", step::length_label(app.step_len())) } else { "Step".into() };
    p.text(b.center(), Align2::CENTER_CENTER, label, small.clone(), if on { theme::text_on(TOKENS.accent) } else { TOKENS.text_dim });
    layout.step = r4(b);
    crate::chrome::context_menu(&resp, true).show(|ui| {
        use transit_engine::step_input::StepControl;
        ui.label("Step input settings");
        crate::chrome::checkbox(ui, &mut app.ui.keys.step_hold.enabled, "Hold to lengthen");
        ui.horizontal(|ui| {
            ui.label("Hold threshold (ms)");
            crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut app.ui.keys.step_hold.threshold_ms).range(1..=60_000)));
        });
        ui.horizontal(|ui| {
            ui.label("Repeat floor (ms)");
            crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut app.ui.keys.step_hold.floor_ms).range(1..=1000)));
        });
        ui.horizontal(|ui| {
            ui.label("Speed up over");
            let r = crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut app.ui.keys.step_hold.ramp_units).range(0..=64).suffix(" units")));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Speed up over N units"));
        });
        ui.label("Sustain + the same notes ties the previous chord.");
        for (label, action, binding) in [
            ("Tie/Rest", StepControl::TieRest, app.ui.keys.step_bindings.tie_rest),
            ("Rest", StepControl::Rest, app.ui.keys.step_bindings.rest),
            ("Step back", StepControl::Back, app.ui.keys.step_bindings.back),
            ("Tie", StepControl::Tie, app.ui.keys.step_bindings.tie),
        ] {
            ui.horizontal(|ui| {
                ui.label(format!("{label}: {}", binding.map(|b| format!("{b:?}")).unwrap_or_else(|| "Unassigned".into())));
                if ui.button("Learn").clicked() {
                    if !app.keys.step.on {
                        app.step_toggle();
                    }
                    app.keys.step.input_cancel_for_learn(action);
                }
                if ui.button("Clear").clicked() {
                    *match action {
                        StepControl::TieRest => &mut app.ui.keys.step_bindings.tie_rest,
                        StepControl::Rest => &mut app.ui.keys.step_bindings.rest,
                        StepControl::Back => &mut app.ui.keys.step_bindings.back,
                        StepControl::Tie => &mut app.ui.keys.step_bindings.tie,
                    } = None;
                    app.live_clear();
                }
            });
        }
        if app.keys.step.learning.is_some() {
            ui.label("Press a MIDI key, pad, or controller to learn.");
            if ui.button("Cancel learn").clicked() {
                app.keys.step.learning = None;
            }
        }
    });
    if resp
        .on_hover_text(format!(
            "Step entry: notes played on the MIDI keyboard go in at the cursor ({}; rest {}, back {}, length 1-6)",
            crate::shortcuts::label(crate::shortcuts::Action::StepToggle),
            crate::shortcuts::label(crate::shortcuts::Action::StepRest),
            crate::shortcuts::label(crate::shortcuts::Action::StepBack)
        ))
        .clicked()
    {
        app.step_toggle();
    }
    // Step velocity: click toggles Played / Fixed; dragging up or right raises the fixed value.
    let (b, _) = button(ui, 116.0, "step_velocity");
    let resp = ui.interact(b, ui.id().with(("keys_btn", "step_velocity_drag")), Sense::click_and_drag());
    let fixed = app.ui.keys.step_fixed;
    let p = ui.painter();
    p.rect_filled(b, crate::theme::RADIUS, if resp.hovered() || resp.dragged() { TOKENS.button_hover } else { TOKENS.button });
    let label = if fixed { format!("Velocity: Fixed {}", app.ui.keys.step_velocity) } else { "Velocity: Played".into() };
    p.text(b.center(), Align2::CENTER_CENTER, label, small.clone(), if fixed { TOKENS.text } else { TOKENS.text_dim });
    layout.step_velocity = r4(b);
    if resp.drag_started() {
        app.ui.keys.step_fixed = true;
        app.keys.step_vel_drag = 0.0;
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        app.keys.step_vel_drag += (d.x - d.y) * 0.5;
        let whole = app.keys.step_vel_drag.trunc();
        app.keys.step_vel_drag -= whole;
        app.ui.keys.step_velocity = (i32::from(app.ui.keys.step_velocity) + whole as i32).clamp(1, 127) as u8;
    }
    if resp
        .on_hover_text(
            "Step entry velocity: Played uses how hard the key was hit; Fixed enters every note at the set value. Click to switch, drag to change.",
        )
        .clicked()
    {
        app.ui.keys.step_fixed = !fixed;
    }
    let (b, resp) = button(ui, 72.0, "audition");
    let on = app.ui.keys.audition;
    let p = ui.painter();
    p.rect_filled(
        b,
        crate::theme::RADIUS_SM,
        if on {
            TOKENS.accent
        } else if resp.hovered() {
            TOKENS.button_hover
        } else {
            TOKENS.button
        },
    );
    p.text(b.center(), Align2::CENTER_CENTER, "Audition", small.clone(), if on { theme::text_on(TOKENS.accent) } else { TOKENS.text_dim });
    layout.audition = r4(b);
    if resp.on_hover_text("Play notes as you place, click or drag them").clicked() {
        app.ui.keys.audition = !on;
        app.release_auditions(f64::INFINITY, false);
    }
    let (b, resp) = button(ui, 72.0, "quantize");
    let p = ui.painter();
    p.rect_filled(b, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    p.text(b.center(), Align2::CENTER_CENTER, "Quantize", small.clone(), TOKENS.text);
    layout.quantize = r4(b);
    if resp
        .on_hover_text(format!("Preview strength and swing; quick apply: {}", crate::shortcuts::label(crate::shortcuts::Action::NotesQuantize)))
        .clicked()
    {
        app.ui.keys.quantize.grid_ticks = app.keys_step();
        app.keys.quantize_open = true;
    }
    let (b, resp) = button(ui, 54.0, "pitch");
    let p = ui.painter();
    p.rect_filled(b, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    p.text(b.center(), Align2::CENTER_CENTER, "Pitch", small.clone(), TOKENS.text);
    layout.pitch = r4(b);
    crate::chrome_pick::dropdown(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| pitch::menu(app, ui, layout));
    let (b, resp) = button(ui, 104.0, "snap");
    let p = ui.painter();
    p.rect_filled(b, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    let grid_name = match Snap::of(&app.ui.keys.grid) {
        Some(snap) => snap.label().to_string(),
        None => grid::label_at(keys_ppt(&app.ui.keys), grid::bar_ticks(app.engine.session()), &app.ui.keys.grid),
    };
    p.text(pos2(b.min.x + 8.0, b.center().y), Align2::LEFT_CENTER, format!("Grid {grid_name}"), small, TOKENS.text);
    let c = pos2(b.max.x - 9.0, b.center().y);
    p.add(egui::Shape::convex_polygon(
        vec![pos2(c.x - 3.0, c.y - 1.5), pos2(c.x + 3.0, c.y - 1.5), pos2(c.x, c.y + 2.0)],
        TOKENS.text_dim,
        Stroke::NONE,
    ));
    layout.snap = r4(b);
    let current = Snap::of(&app.ui.keys.grid);
    crate::chrome_pick::dropdown(&resp).width(100.0).show(|ui| {
        for snap in Snap::ALL {
            let item = crate::menus::check_button(ui, Some(snap) == current, snap.label());
            layout.snap_menu.push(MenuItem::at(ui, snap.label().into(), snap.label().into(), item.rect));
            if item.clicked() {
                snap.apply(&mut app.ui.keys.grid);
                ui.close();
            }
        }
    });

    // Copy only a single, still-existing selection: a stale or mixed selection is ambiguous.
    let selected = match app.keys.selection.as_slice() {
        [note] if app.keys_all().is_some_and(|all| all.notes.contains(note)) => Some(note.velocity),
        _ => None,
    };
    let b = control_band(r, rx - 24.0, rx);
    rx -= 30.0;
    let resp = ui.add_enabled_ui(selected.is_some(), |ui| ui.put(b, egui::Button::new(""))).inner;
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, selected.is_some(), "Use selected note's velocity"));
    // Original code-drawn eyedropper; no image asset or font glyph.
    let p = ui.painter();
    let c = b.center();
    let ink = if selected.is_some() { TOKENS.text } else { TOKENS.text_dim };
    let stroke = Stroke::new(1.4, ink);
    p.line_segment([c + vec2(-6.0, 6.0), c + vec2(-4.0, 2.0)], stroke);
    p.add(egui::Shape::closed_line(vec![c + vec2(-4.0, 2.0), c + vec2(2.0, -4.0), c + vec2(4.0, -2.0), c + vec2(-2.0, 4.0)], stroke));
    p.line_segment([c + vec2(0.0, -6.0), c + vec2(6.0, 0.0)], stroke);
    p.line_segment([c + vec2(3.0, -3.0), c + vec2(6.0, -6.0)], Stroke::new(3.0, ink));
    layout.use_selected_velocity = r4(b);
    if resp.on_hover_text("Use selected note's velocity for new notes and audition (select one note)").clicked()
        && let Some(velocity) = selected
    {
        app.ui.keys.velocity = velocity.clamp(1, 127);
    }

    let field = control_band(r, rx - 44.0, rx);
    ui.push_id("keys_new_note_velocity", |ui| {
        let resp = header_field(ui, field, true, |ui, band| ui.put(band, egui::DragValue::new(&mut app.ui.keys.velocity).range(1..=127).speed(1)));
        layout.new_note_velocity = r4(resp.rect);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "New note velocity"));
        resp.on_hover_text("New note velocity (1–127): used for drawing and audition. Drag or click to type.");
    });
    rx -= 150.0;
    let label = ui.painter().text(pos2(rx, r.center().y), Align2::LEFT_CENTER, "New note velocity", FontId::proportional(11.5), TOKENS.text);
    layout.new_note_velocity_label = r4(label);
    if r.width() >= 1200.0 {
        rx -= 146.0;
        note_inspector(app, ui, Rect::from_min_max(pos2(rx, r.min.y), pos2(rx + 138.0, r.max.y)), layout);
    }
    let title_rect = Rect::from_min_max(pos2(x + 32.0, r.min.y), pos2((rx - 8.0).max(x + 32.0), r.max.y));
    ui.painter().with_clip_rect(title_rect).text(title_rect.left_center(), Align2::LEFT_CENTER, title, FontId::proportional(11.5), TOKENS.text_dim);
}

/// Commit exact ticks on Enter or focus loss, as one undoable command.
fn note_inspector(app: &mut StudioApp, ui: &mut Ui, rect: Rect, layout: &mut KeysLayout) {
    let selected = match app.keys.selection.as_slice() {
        [note] if app.keys_all().is_some_and(|all| all.notes.contains(note)) => Some(*note),
        _ => None,
    };
    if app.keys.length_edit.as_ref().map(|(note, _)| *note) != selected {
        app.keys.length_edit = selected.map(|note| (note, note.length.to_string()));
    }
    let label = ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, "Length", FontId::proportional(11.5), TOKENS.text);
    layout.note_length_label = r4(label);
    let field = control_band(rect, rect.min.x + 44.0, rect.max.x);
    let mut empty = String::new();
    let text = app.keys.length_edit.as_mut().map_or(&mut empty, |(_, text)| text);
    let resp = header_field(ui, field, selected.is_some(), |ui, band| {
        let edit = egui::TextEdit::singleline(text)
            .id_salt("keys_note_length")
            .font(egui::TextStyle::Monospace)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(6, 0)));
        ui.put(band, edit.min_size(vec2(0.0, HEADER_CONTROL_H)).vertical_align(egui::Align::Center))
    });
    layout.note_length = r4(resp.rect);
    let commit = resp.lost_focus();
    resp.on_hover_text("Selected note length in ticks (minimum 1). Type an exact value, then Enter.");
    if commit && let Some((note, text)) = app.keys.length_edit.take() {
        match text.trim().parse::<i64>() {
            Ok(length) if length >= 1 && note.start.checked_add(length).is_some() => {
                if let Ok(selection) = app.keys_edit(&[(note, Note { length, ..note })]) {
                    app.keys.selection = selection;
                }
            }
            _ => app.status = "Note length must be a positive whole number of ticks".into(),
        }
    }
}

/// The pane's top edge: drag to change its height.
fn pane_handle(app: &mut StudioApp, ui: &mut Ui, handle: Rect, layout: &mut KeysLayout) {
    let resp = ui.interact(handle, ui.id().with("keys_handle"), Sense::drag());
    ui.painter().line_segment([handle.left_top(), handle.right_top()], Stroke::new(1.0, TOKENS.line));
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        ui.painter().line_segment(
            [pos2(handle.min.x, handle.min.y + 1.0), pos2(handle.max.x, handle.min.y + 1.0)],
            Stroke::new(2.0, TOKENS.accent.gamma_multiply(0.6)),
        );
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        app.keys.gesture = Some(KeysGesture::Height { start: app.ui.keys.height, press });
    }
    if resp.dragged()
        && let Some(KeysGesture::Height { start, press }) = app.keys.gesture.clone()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        let max_h = (ui.ctx().content_rect().height() - 200.0).max(MIN_H);
        app.ui.keys.height = (start - (pos.y - press.y)).clamp(MIN_H, max_h);
    }
    if resp.drag_stopped() {
        app.keys.gesture = None;
    }
    layout.handle = r4(handle);
}

/// Key rows, the clips' ranges, bar, beat and grid lines, and the ruler with bar numbers.
#[allow(clippy::too_many_arguments)]
fn paint_grid(ui: &Ui, s: &Session, map: &Map, grid: Rect, ruler: Rect, cv: &ClipView, all: &Composite, layout: &mut KeysLayout) {
    let p = ui.painter().with_clip_rect(grid);
    p.rect_filled(grid, 0.0, TOKENS.bg);
    // Each open clip's span in the roll's ticks (one clip: its own `0..len`).
    let spans: Vec<(i64, i64)> = all.parts.iter().map(|part| (part.off, part.off + part.len)).collect();
    let insides: Vec<Rect> = spans
        .iter()
        .map(|&(a, b)| Rect::from_min_max(pos2(map.x(a).max(grid.min.x), grid.min.y), pos2(map.x(b).min(grid.max.x), grid.max.y)))
        .collect();
    let lo = map.pitch(grid.max.y).clamp(0, 127);
    let hi = map.pitch(grid.min.y).clamp(0, 127);
    for pitch in lo..=hi {
        let pitch = pitch as u8;
        if !map.rows.contains(pitch) {
            continue;
        }
        let y = map.y(pitch);
        let row = Rect::from_min_max(pos2(grid.min.x, y), pos2(grid.max.x, y + map.kh));
        let fill = if map.scale.enabled {
            if pitch % 12 == map.scale.key.root % 12 {
                TOKENS.accent.gamma_multiply(0.25)
            } else if map.scale.key.contains(pitch) {
                TOKENS.lane_alt
            } else {
                // Out of scale: the loop hue at 12 percent (#326).
                TOKENS.loop_region.gamma_multiply(crate::tokens::alpha::LOOP_SCALE_ROW)
            }
        } else if is_black(pitch) {
            TOKENS.lane
        } else {
            TOKENS.lane_alt
        };
        for inside in insides.iter().filter(|r| r.width() > 0.0) {
            p.rect_filled(row.intersect(*inside), 0.0, fill);
        }
        if pitch.is_multiple_of(12) {
            p.line_segment([pos2(grid.min.x, y + map.kh), pos2(grid.max.x, y + map.kh)], Stroke::new(1.0, TOKENS.grid_bar));
        }
    }
    let rp = ui.painter().with_clip_rect(ruler);
    rp.rect_filled(ruler, 0.0, TOKENS.ruler);
    if all.several() {
        // Each clip's bar in its color, the foreground's brighter: click one to bring it forward.
        for part in &all.parts {
            let fg = part.clip == all.parts[0].clip;
            let bar = Rect::from_min_max(pos2(map.x(part.off), ruler.max.y - 6.0), pos2(map.x(part.off + part.len), ruler.max.y));
            rp.rect_filled(bar, 0.0, theme::rgb(part.color).gamma_multiply(if fg { 1.0 } else { 0.55 }));
            layout.clips.push(OpenClip {
                clip: part.clip.0,
                track: part.track.0,
                offset_ticks: part.off,
                ticks: part.len,
                foreground: fg,
                color: part.color,
                bar: r4(bar.intersect(ruler)),
            });
        }
    } else {
        rp.rect_filled(
            Rect::from_min_max(pos2(map.x(0), ruler.max.y - 4.0), pos2(map.x(cv.len), ruler.max.y)),
            0.0,
            TOKENS.text_dim.gamma_multiply(0.5),
        );
    }
    rp.line_segment([pos2(ruler.min.x, ruler.max.y - 0.5), pos2(ruler.max.x, ruler.max.y - 0.5)], Stroke::new(1.0, TOKENS.line));
    // Lines: grid subdivisions, beats, bars (absolute ticks; the clip starts at c0).
    let t_left = (map.tick(grid.min.x) as i64 + map.c0).max(0);
    let t_right = map.tick(grid.max.x) as i64 + map.c0;
    let first_bar = s.tempo.bar_beat_at_tick(t_left).bar.max(1);
    let bar_px = (map.x(s.tempo.bar_start_tick(first_bar + 1) - map.c0) - map.x(s.tempo.bar_start_tick(first_bar) - map.c0)).max(0.01);
    let every = [1, 2, 4, 8, 16, 32, 64].into_iter().find(|n| bar_px * *n as f32 >= 36.0).unwrap_or(128);
    for bar in (first_bar..).take(2000) {
        let b0 = s.tempo.bar_start_tick(bar);
        if b0 > t_right {
            break;
        }
        let b1 = s.tempo.bar_start_tick(bar + 1);
        let m = s.tempo.meter_at_tick(b0);
        let beat = (PPQ * 4 / i64::from(m.denominator.max(1))).max(1);
        let sub = grid::sub_line_step(map.step, map.ppt, beat);
        let mut t = b0 + beat;
        while t < b1 {
            let x = map.x(t - map.c0);
            p.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(1.0, TOKENS.grid_bar.gamma_multiply(0.75)));
            t += beat;
        }
        // Sub-beat lines, fainter than the beats (a triplet step does not divide the beat evenly).
        if let Some(sub) = sub {
            let mut t = b0 + sub;
            while t < b1 {
                if (t - b0) % beat != 0 {
                    let x = map.x(t - map.c0);
                    p.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(1.0, TOKENS.grid_beat.gamma_multiply(0.6)));
                }
                t += sub;
            }
        }
        let x = map.x(b0 - map.c0);
        p.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(1.0, TOKENS.grid_bar));
        if (bar - 1) % every == 0 {
            rp.line_segment([pos2(x, ruler.min.y + 6.0), pos2(x, ruler.max.y)], Stroke::new(1.0, TOKENS.text_dim));
            rp.text(pos2(x + 4.0, ruler.min.y + 8.0), Align2::LEFT_CENTER, bar.to_string(), FontId::proportional(10.5), TOKENS.text);
        }
    }
    // Outside every clip: dimmed (the gaps between the spans, merged, and both ends).
    let dim = TOKENS.overlay;
    let mut merged: Vec<(i64, i64)> = Vec::new();
    let mut sorted = spans.clone();
    sorted.sort_unstable();
    for (a, b) in sorted {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut x = grid.min.x;
    for (a, b) in &merged {
        if map.x(*a) > x {
            p.rect_filled(Rect::from_min_max(pos2(x, grid.min.y), pos2(map.x(*a), grid.max.y)), 0.0, dim);
        }
        x = x.max(map.x(*b));
    }
    if x < grid.max.x {
        p.rect_filled(Rect::from_min_max(pos2(x, grid.min.y), grid.max), 0.0, dim);
    }
    // The other clips' starts, in their colors.
    for part in all.parts.iter().skip(1) {
        let x = map.x(part.off);
        p.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(1.5, theme::rgb(part.color)));
    }
    if let Some((start, end)) = cv.cycle {
        // The one-shot lead-in before the cycle, the repeats' dividers and the loop end with its label.
        if start > 0 {
            let r = Rect::from_min_max(pos2(map.x(0).max(grid.min.x), grid.min.y), pos2(map.x(start).min(grid.max.x), grid.max.y));
            p.rect_filled(r, 0.0, TOKENS.text_dim.gamma_multiply(0.14));
        }
        let cycle = end - start;
        let mut t = end + cycle;
        let mut n = 0;
        while t < cv.full && map.x(t) < grid.max.x && n < MAX_GHOSTS {
            p.line_segment([pos2(map.x(t), grid.min.y), pos2(map.x(t), grid.max.y)], Stroke::new(1.0, TOKENS.line.gamma_multiply(0.7)));
            t += cycle;
            n += 1;
        }
        let x = map.x(end);
        p.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(2.0, TOKENS.accent));
        p.text(pos2(x + 4.0, grid.min.y + 8.0), Align2::LEFT_CENTER, "Loop end", FontId::proportional(10.5), TOKENS.accent);
    }
}

/// A looped clip's repeats: the cycle's notes as dimmed ghosts, as the arranger draws loop-drag cycles.
fn paint_loop_ghosts(ui: &Ui, map: &Map, grid: Rect, cv: &ClipView, layout: &mut KeysLayout) {
    layout.loop_ghosts = cv.ghosts((map.tick(grid.min.x).floor() as i64, map.tick(grid.max.x).ceil() as i64)).0;
    let p = ui.painter().with_clip_rect(grid);
    for g in &layout.loop_ghosts {
        let r = map.note_rect(&Note { pitch: g.pitch, velocity: 100, release_velocity: 64, channel: 0, start: g.start, length: g.length });
        if r.max.x < grid.min.x || r.min.x > grid.max.x || r.max.y < grid.min.y || r.min.y > grid.max.y {
            continue;
        }
        p.rect_filled(r, crate::theme::RADIUS_SM, ghost_colors().0);
        p.rect_stroke(r, crate::theme::RADIUS_SM, Stroke::new(1.0, ghost_colors().1), StrokeKind::Inside);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_notes(
    app: &StudioApp,
    ui: &Ui,
    map: &Map,
    grid: Rect,
    vel: Rect,
    all: &Composite,
    preview: Option<&(Vec<(Note, Note)>, bool)>,
    layout: &mut KeysLayout,
) {
    let notes = &all.notes;
    // A note's clip: its span in the roll's ticks and, with several open, its color (#329).
    let part_of = |i: usize| all.locate(i).map(|(p, _)| p);
    let span_end = |i: usize| part_of(i).map_or(i64::MAX, |p| p.off + p.len);
    let ink = |part: Option<&Part>| if all.several() { part.map_or(TOKENS.accent, |p| theme::rgb(p.color)) } else { TOKENS.accent };
    let p = ui.painter().with_clip_rect(grid);
    let vp = ui.painter().with_clip_rect(vel);
    vp.rect_filled(vel, 0.0, TOKENS.panel);
    let sel_idx = resolve(notes, &app.keys.selection);
    let moving: Vec<usize> = match preview {
        Some((pairs, false)) => resolve(notes, &pairs.iter().map(|(a, _)| *a).collect::<Vec<_>>()),
        _ => Vec::new(),
    };
    let stem = |n: &Note| {
        let x = map.x(n.start);
        let h = (vel.height() - 6.0) * f32::from(n.velocity) / 127.0;
        Rect::from_min_max(pos2(x, vel.max.y - h), pos2(x + 3.0, vel.max.y))
    };
    let draw = |n: &Note, selected: bool, alpha: f32, end: i64, accent: egui::Color32| {
        let r = map.note_rect(n);
        let outside = n.start >= end;
        let alpha = if outside { alpha * 0.4 } else { alpha };
        let a = NOTE_MIN_ALPHA + 0.55 * f32::from(n.velocity) / 127.0;
        let fill = if selected { theme::lighten(accent, 0.25) } else { accent.gamma_multiply(a) };
        p.rect_filled(r, crate::theme::RADIUS_SM, fill.gamma_multiply(alpha));
        if selected {
            p.rect_stroke(r, crate::theme::RADIUS_SM, Stroke::new(1.0, TOKENS.selection_outline.gamma_multiply(alpha)), StrokeKind::Inside);
        }
        if outside {
            // Diagonal hatch: past the clip's end the note is silent.
            let hp = p.with_clip_rect(r.intersect(grid));
            let mut x = r.min.x - r.height();
            while x < r.max.x {
                hp.line_segment([pos2(x, r.max.y), pos2(x + r.height(), r.min.y)], Stroke::new(1.0, TOKENS.text_dim.gamma_multiply(0.7)));
                x += 5.0;
            }
        }
        let s = stem(n);
        let c = if selected { TOKENS.selection_outline } else { accent };
        vp.rect_filled(s, 0.0, c.gamma_multiply(alpha * 0.9));
        vp.rect_filled(Rect::from_min_size(pos2(s.min.x - 1.5, s.min.y - 1.5), vec2(6.0, 3.0)), crate::theme::RADIUS_SM, c.gamma_multiply(alpha));
    };
    for (i, n) in notes.iter().enumerate() {
        let selected = sel_idx.contains(&i);
        let dimmed = layout.focus == Some(true) && part_of(i).is_some_and(|part| part.clip != all.parts[0].clip);
        let r = map.note_rect(n);
        layout.notes.push(NoteRect {
            index: i,
            pitch: n.pitch,
            start: n.start,
            length: n.length,
            velocity: n.velocity,
            selected,
            outside: n.start >= span_end(i),
            rect: r4(r),
            vel_rect: r4(stem(n)),
            clip: all.several().then(|| part_of(i).map(|p| p.clip.0)).flatten(),
            dimmed: (layout.focus == Some(true)).then_some(dimmed),
        });
        if r.max.x < grid.min.x || r.min.x > grid.max.x || r.max.y < grid.min.y || r.min.y > grid.max.y {
            continue;
        }
        let alpha = if moving.contains(&i) { 0.3 } else { 1.0 };
        draw(n, selected, if dimmed { alpha * 0.35 } else { alpha }, span_end(i), ink(part_of(i)));
    }
    if let Some((pairs, _)) = preview {
        let sources: Vec<Note> = pairs.iter().map(|(a, _)| *a).collect();
        for ((_, n), part) in pairs.iter().zip(all.owners(&sources)) {
            draw(n, true, 0.9, part.map_or(i64::MAX, |p| p.off + p.len), ink(part));
        }
        layout.drag_lengths = pairs.iter().map(|(_, n)| n.length).collect();
        if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
            let text = pairs.iter().map(|(_, n)| format!("{} ticks", n.length)).collect::<Vec<_>>().join(", ");
            let galley = p.layout_no_wrap(text, FontId::monospace(11.0), TOKENS.text);
            // Keep the readout visible even when the pointer reaches or leaves a grid edge.
            let at = pos2(
                (pos.x + 12.0).clamp(grid.min.x + 3.0, (grid.max.x - galley.size().x - 3.0).max(grid.min.x + 3.0)),
                (pos.y - 22.0).clamp(grid.min.y + 3.0, (grid.max.y - galley.size().y - 3.0).max(grid.min.y + 3.0)),
            );
            p.rect_filled(Rect::from_min_size(at, galley.size()).expand(3.0), crate::theme::RADIUS_SM, TOKENS.panel);
            p.galley(at, galley, TOKENS.text);
        }
    }
}

fn paint_keyboard(app: &StudioApp, ui: &Ui, map: &Map, kb: Rect) {
    let p = ui.painter().with_clip_rect(kb);
    p.rect_filled(kb, 0.0, TOKENS.panel_raised);
    let lo = map.pitch(kb.max.y).clamp(0, 127);
    let hi = map.pitch(kb.min.y).clamp(0, 127);
    let sounding: Vec<u8> = app.keys.held.iter().map(|h| h.1).collect();
    for pitch in lo..=hi {
        let pitch = pitch as u8;
        if !map.rows.contains(pitch) {
            continue;
        }
        let y = map.y(pitch);
        let row = Rect::from_min_max(pos2(kb.min.x, y), pos2(kb.max.x, y + map.kh));
        let live_color = if map.scale.enabled && !map.scale.key.contains(pitch) { TOKENS.loop_region } else { TOKENS.accent };
        let fill = if sounding.contains(&pitch) {
            live_color
        } else if map.scale.enabled {
            if pitch % 12 == map.scale.key.root % 12 {
                TOKENS.accent.gamma_multiply(0.5)
            } else if map.scale.key.contains(pitch) || !is_black(pitch) {
                TOKENS.panel_raised
            } else {
                // Out of scale: the key keeps its own fill; its label takes the loop hue (#326).
                TOKENS.bg
            }
        } else if is_black(pitch) {
            TOKENS.bg
        } else {
            TOKENS.panel_raised
        };
        p.rect_filled(row, 0.0, fill);
        // Held accidentals keep their distinct color; intensity follows played velocity.
        let v = app.keys.live[usize::from(pitch)];
        if v > 0 {
            p.rect_filled(row, 0.0, live_color.gamma_multiply(0.6 + 0.4 * f32::from(v) / 127.0));
        }
        p.line_segment(
            [pos2(kb.min.x, row.max.y), pos2(kb.max.x, row.max.y)],
            Stroke::new(1.0, TOKENS.line.gamma_multiply(if pitch.is_multiple_of(12) { 1.0 } else { 0.5 })),
        );
        let c = if map.scale.enabled { pitch % 12 == map.scale.key.root % 12 } else { pitch.is_multiple_of(12) };
        if c || (map.kh >= 14.0 && (map.scale.enabled || !is_black(pitch))) {
            let out_of_scale = map.scale.enabled && !map.scale.key.contains(pitch);
            let color = if c {
                TOKENS.text
            } else if out_of_scale {
                TOKENS.loop_region
            } else {
                TOKENS.text_dim
            };
            p.text(
                pos2(kb.max.x - 6.0, row.center().y),
                Align2::RIGHT_CENTER,
                transit_midi::note_name(pitch),
                FontId::proportional((map.kh - 4.0).clamp(8.0, 11.0)),
                color,
            );
        }
    }
    p.line_segment([pos2(kb.max.x - 0.5, kb.min.y), pos2(kb.max.x - 0.5, kb.max.y)], Stroke::new(1.0, TOKENS.line));
}

/// The ruler: a click sets the cursor; a drag up or down zooms time around the pointer, sideways scrolls.
fn ruler_input(app: &mut StudioApp, ui: &mut Ui, map: &Map, ruler: Rect, s: &Session, clips: &[OpenClip]) {
    let resp = ui.interact(ruler, ui.id().with("keys_ruler"), Sense::click_and_drag());
    if resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        // A clip's bar at the ruler's foot: that clip comes forward (#329); elsewhere the ruler sets the playhead.
        let on_bar = clips.iter().rev().find(|c| pos.x >= c.bar[0] && pos.x <= c.bar[2] && pos.y >= c.bar[1] && pos.y <= c.bar[3]);
        if let Some(c) = on_bar {
            app.keys_foreground(ClipId(c.clip));
        } else {
            let abs = (map.c0 as f64 + map.tick(pos.x)).max(0.0) as i64;
            app.click_ruler(s.tempo.tick_to_samples(abs, s.sample_rate));
        }
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        app.keys.gesture = Some(KeysGesture::Ruler { ppq: app.ui.keys.px_per_quarter, anchor: map.tick(press.x), press });
    }
    if resp.dragged()
        && let Some(KeysGesture::Ruler { ppq, anchor, press }) = app.keys.gesture.clone()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        let z = (ppq * ((pos.y - press.y) * 0.01).exp()).clamp(8.0, 800.0);
        app.ui.keys.px_per_quarter = z;
        app.keys.scroll = anchor - f64::from(pos.x - map.gx0) / (f64::from(z) / PPQ as f64);
    }
    if resp.drag_stopped() {
        app.keys.gesture = None;
    }
}
