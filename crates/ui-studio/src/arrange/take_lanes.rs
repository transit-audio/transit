//! Take lanes (#25, ADR 0039): a track's stacked takes as lanes under its row, one lane per take,
//! newest on top, at [`TAKE_H`] each. In a lane, a take's clips are dimmed where another take
//! plays and highlighted where it plays; a click on a dimmed clip makes its take play over the
//! clip's range (`takes.set_active`, one undo step). The lane's label in the track list says
//! "Take n" (and "playing" when some of it plays); a click on it makes that take play in every
//! stack it is in, and its menu button offers Flatten Takes and Delete Take. A clip of the
//! playing take carries an "n takes" chip that shows or hides the lanes.
//!
//! Conventions: Logic's Take Folder (takes stacked under the track, click a take to make it
//! active, Flatten) and Ableton's Take Lanes (Live 11: Show Take Lanes, a lane per pass, click to
//! make a take the main clip) agree on everything here. Swipe comping across takes is slice 2.

use super::*;
use transit_model::TakeStack;

/// How many take lanes a track shows: its takes while its lanes are open.
pub(crate) fn takes_shown(app: &StudioApp, t: &Track) -> usize {
    if t.playlists.len() > 1 && app.ui.takes_open.contains(&t.id) { app.takes_of(t).numbers.len() } else { 0 }
}

/// Draw each row's take chips and, for tracks whose lanes are open, the lanes and their labels.
/// Drawn after the timeline's input, so these take the clicks.
#[allow(clippy::too_many_arguments)]
pub(super) fn takes(app: &mut StudioApp, ui: &mut Ui, s: &Session, map: Map, tl: Rect, list: Rect, rows: &[&Track], geo: &RowGeo, layout: &Layout) {
    let _ = s;
    for (i, t) in rows.iter().enumerate() {
        if t.playlists.len() < 2 {
            continue;
        }
        let takes = app.takes_of(t);
        let stacks = &takes.stacks;
        if stacks.is_empty() {
            continue;
        }
        let y0 = tl.min.y + geo.tops[i] - app.arrange.vscroll;
        let open = app.ui.takes_open.contains(&t.id);
        chips(app, ui, t, stacks, tl, layout, open);
        if !open {
            continue;
        }
        for (k, number) in takes.numbers.iter().copied().enumerate() {
            let ly = y0 + geo.rh + k as f32 * TAKE_H;
            lane(app, ui, t, stacks, number, k, ly, map, tl, list);
        }
    }
}

/// The "n takes" chip on each playing clip of a stack: click shows or hides the lanes.
fn chips(app: &mut StudioApp, ui: &mut Ui, t: &Track, stacks: &[TakeStack], tl: Rect, layout: &Layout, open: bool) {
    for st in stacks {
        let playing = st.takes.iter().flat_map(|k| k.clips.iter()).filter(|(pl, _)| *pl == t.active_playlist).map(|(_, c)| *c);
        for c in playing {
            let Some(cr) = layout.clips.iter().find(|r| r.id == c.0).map(|r| r.rect) else { continue };
            let wide = cr[2] - cr[0] >= 64.0 + 8.0;
            // Wide clips get the chip at their right end; narrower ones a 10 px glyph at their left
            // edge, so the lanes stay reachable at any zoom.
            let chip = if wide {
                Rect::from_min_max(pos2(cr[2] - 64.0 - 2.0, cr[1] + 1.5), pos2(cr[2] - 2.0, cr[1] + CLIP_NAME_H - 0.5))
            } else {
                Rect::from_min_max(pos2(cr[0].max(tl.min.x) + 1.0, cr[1] + 1.5), pos2(cr[0].max(tl.min.x) + 11.0, cr[1] + CLIP_NAME_H - 0.5))
            };
            if !tl.intersects(chip) {
                continue;
            }
            let resp = ui.interact(chip.intersect(tl), ui.id().with(("take_chip", c.0)), Sense::click());
            let p = ui.painter().with_clip_rect(tl);
            p.rect_filled(chip, crate::theme::RADIUS_SM, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
            let icon = if open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronRight };
            if wide {
                crate::icons::paint(&p, pos2(chip.min.x + 8.0, chip.center().y), icon, 12.0, TOKENS.text);
                p.text(
                    pos2(chip.min.x + 16.0, chip.center().y),
                    Align2::LEFT_CENTER,
                    format!("{} takes", st.takes.len()),
                    FontId::proportional(10.5),
                    TOKENS.text,
                );
            } else {
                crate::icons::paint(&p, chip.center(), icon, 10.0, TOKENS.text);
            }
            if resp.on_hover_text(if open { "Hide the take lanes" } else { "Show the take lanes" }).clicked() {
                app.toggle_takes(t.id);
            }
            app.finish.layout.chips.push(crate::finish::TakeChip { track: t.id.0, clip: c.0, takes: st.takes.len(), open, rect: r4(chip) });
        }
    }
}

/// One take lane: its clips in the timeline, its label and menu in the track list.
#[allow(clippy::too_many_arguments)]
fn lane(app: &mut StudioApp, ui: &mut Ui, t: &Track, stacks: &[TakeStack], number: u32, k: usize, ly: f32, map: Map, tl: Rect, list: Rect) {
    let lane = Rect::from_min_max(pos2(tl.min.x, ly), pos2(tl.max.x, ly + TAKE_H));
    let active = stacks.iter().filter_map(|st| st.take(number)).any(transit_model::StackTake::is_active);
    let p = ui.painter().with_clip_rect(tl);
    p.rect_filled(lane.shrink2(vec2(0.0, 1.0)), 0.0, TOKENS.bg);
    p.line_segment([pos2(lane.min.x, lane.max.y), pos2(lane.max.x, lane.max.y)], Stroke::new(1.0, TOKENS.line));
    // The take's clips: those on other playlists dimmed first, the playing ones on top.
    let mut clips: Vec<(&Clip, bool)> = stacks
        .iter()
        .filter_map(|st| st.take(number))
        .flat_map(|k| k.clips.iter())
        .filter_map(|(pl, id)| t.playlists.get(*pl).and_then(|p| p.clip(*id)).map(|c| (c, *pl == t.active_playlist)))
        .collect();
    clips.sort_by_key(|(c, on)| (*on, c.start));
    let base = theme::rgb(t.color);
    for (c, on) in &clips {
        let r = Rect::from_min_max(pos2(map.x(c.start), ly + 3.0), pos2(map.x(c.end()).max(map.x(c.start) + 2.0), ly + TAKE_H - 3.0));
        if !tl.intersects(r) {
            continue;
        }
        let col = c.color.map_or(base, theme::rgb);
        if *on {
            p.rect_filled(r, crate::theme::RADIUS_SM, col);
            p.rect_stroke(r, crate::theme::RADIUS_SM, Stroke::new(1.5, TOKENS.accent), StrokeKind::Inside);
        } else {
            p.rect_filled(r, crate::theme::RADIUS_SM, col.gamma_multiply(0.35));
        }
        if r.width() > 40.0 {
            let ink = if *on { theme::text_on(col) } else { TOKENS.text_dim };
            p.with_clip_rect(r.intersect(tl)).text(pos2(r.min.x + 4.0, r.center().y), Align2::LEFT_CENTER, &c.name, FontId::proportional(10.0), ink);
        }
        let resp = ui.interact(r.intersect(tl), ui.id().with(("take_clip", t.id.0, c.id.0)), Sense::click());
        let resp = resp.on_hover_text(if *on { format!("Take {number} plays here") } else { format!("Play take {number} here") });
        if resp.clicked() && !*on {
            let _ = app.choose_take_clip(t.id, c.id);
        }
        app.finish.layout.take_clips.push(crate::finish::TakeClip { track: t.id.0, take: number, clip: c.id.0, active: *on, rect: r4(r) });
    }
    // In the track list: the label (click: this take plays everywhere) and the lane menu.
    let lp = ui.painter().with_clip_rect(list);
    let hl = Rect::from_min_max(pos2(list.min.x + 20.0, ly + 1.0), pos2(list.max.x - 6.0, ly + TAKE_H - 1.0));
    let menu = Rect::from_min_size(pos2(hl.max.x - 22.0, hl.min.y + 1.0), vec2(22.0, hl.height() - 2.0));
    let label = Rect::from_min_max(hl.min, pos2(menu.min.x - 4.0, hl.max.y));
    let hresp = ui.interact(label.intersect(list), ui.id().with(("take_label", t.id.0, number)), Sense::click());
    crate::help::hover(ui, &hresp, crate::help::Id::ArrangeHeaderTakeLane);
    if active {
        lp.rect_filled(label, crate::theme::RADIUS_SM, TOKENS.accent.gamma_multiply(0.2));
    }
    let text = if active { format!("Take {number} · playing") } else { format!("Take {number}") };
    lp.text(
        pos2(label.min.x + 6.0, label.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(11.0),
        if active { TOKENS.text } else { TOKENS.text_dim },
    );
    if hresp.on_hover_text(format!("Play take {number} wherever it was recorded")).clicked() {
        let _ = app.choose_take(t.id, number);
    }
    let mresp = ui.interact(menu.intersect(list), ui.id().with(("take_menu", t.id.0, number)), Sense::click());
    crate::help::hover(ui, &mresp, crate::help::Id::ArrangeHeaderTakeLaneMenu);
    lp.rect_filled(menu, crate::theme::RADIUS_SM, if mresp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    crate::icons::paint(&lp, menu.center(), crate::icons::Icon::ChevronDown, 12.0, TOKENS.text);
    let can_delete = !active;
    let track = t.id;
    egui::Popup::menu(&mresp).width(150.0).show(|ui| {
        flatten_item(app, ui, track, None);
        let label = format!("Delete Take {number}");
        let b = ui.add_enabled(can_delete, egui::Button::new(&label));
        app.arrange.menu.push(crate::mixer::MenuItem::at(ui, label, String::new(), b.rect));
        let b = b.on_disabled_hover_text("This take plays: choose another take first, or flatten");
        if b.clicked() {
            app.dialog = Some(crate::Dialog::DeleteTake { track, take: number });
            ui.close();
        }
    });
    let clips_n = clips.len();
    app.finish.layout.takes.push(crate::finish::TakeRow {
        track: t.id.0,
        index: k,
        take: number,
        active,
        clips: clips_n,
        lane: r4(lane),
        label: r4(label),
        menu: r4(menu),
        delete_enabled: can_delete,
    });
}

/// "Flatten Takes" for a track (stack `stack`, or all), as the track menu, the lane menu and the
/// clip menu offer it. Recorded in the menu layout.
pub(crate) fn flatten_item(app: &mut StudioApp, ui: &mut Ui, track: TrackId, stack: Option<usize>) {
    let b = ui.button("Flatten Takes");
    app.arrange.menu.push(crate::mixer::MenuItem::at(ui, "Flatten Takes".into(), String::new(), b.rect));
    if b.on_hover_text("Keep what plays and remove the other takes").clicked() {
        let _ = app.flatten_takes(track, stack);
        ui.close();
    }
}
