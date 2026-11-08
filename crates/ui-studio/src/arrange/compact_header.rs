//! Compact track headers have no hidden interactive controls.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn show(app: &mut StudioApp, ui: &mut Ui, t: &Track, r: Rect, list: Rect, selected: bool, row: &mut TrackRow) {
    let content = Rect::from_min_max(r.min, pos2(r.max.x, r.max.y - 4.0));
    let clip = content.intersect(list);
    let p = ui.painter().with_clip_rect(clip);
    p.rect_filled(r, 0.0, if selected { TOKENS.header_selected } else { TOKENS.header });
    if t.inactive {
        // An inactive track is hatched at every height (#326; only slivers skip it).
        row.hatch_header = crate::widgets::hatch::paint(&p, r);
    }
    let x = r.min.x + row.depth as f32 * crate::groups::INDENT;
    row.color_bar = if t.inactive { TOKENS.ink_ghost } else { theme::rgb(t.color) };
    p.rect_filled(Rect::from_min_max(pos2(x, r.min.y), pos2(x + 5.0, r.max.y)), 0.0, row.color_bar);
    let mut right = r.max.x - 8.0;
    if r.height() >= 24.0 {
        for (label, on, color, command, applicable) in [
            ("I", t.mixer.input_monitor, TOKENS.monitor, "mix.input_monitor", t.kind == TrackKind::Audio || t.is_rack()),
            ("R", t.mixer.record_arm, TOKENS.record, "mix.record_arm", t.kind.has_playlist()),
            ("S", t.mixer.solo, TOKENS.solo, "mix.solo", true),
            ("M", t.mixer.mute, TOKENS.mute, "mix.mute", true),
        ] {
            if !applicable {
                continue;
            }
            let b = Rect::from_min_size(pos2(right - 19.0, content.center().y - 8.0), vec2(19.0, 16.0));
            right -= 22.0;
            let response = ui.interact(b.intersect(clip), ui.id().with((command, t.id.0)), Sense::click());
            p.rect_filled(b, crate::theme::RADIUS_SM, if on { color } else { TOKENS.button });
            p.text(b.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), if on { theme::text_on(color) } else { TOKENS.text });
            match command {
                "mix.input_monitor" => row.monitor = r4(b),
                "mix.record_arm" => row.arm = r4(b),
                "mix.solo" => row.solo = r4(b),
                _ => row.mute = r4(b),
            }
            if response.clicked() {
                let _ = app.run(command, json!({"tracks": [t.id.0], "value": !on}));
            }
        }
    }
    let mut status = Vec::new();
    if t.mixer.record_arm {
        status.push("Armed for recording".to_string());
    }
    if t.mixer.input_monitor {
        status.push("Input monitor on".to_string());
    }
    if let Some((plugin, hang)) = app.track_hang(t) {
        row.unresponsive = crate::devices::UNRESPONSIVE.into();
        let name = app.plugin_name(&plugin, false);
        status.push(app.hang_tip(hang, &name));
    }
    if !status.is_empty() {
        let dot = Rect::from_center_size(pos2(right - 4.0, content.center().y), vec2(6.0, 6.0));
        p.circle_filled(dot.center(), 3.0, if t.mixer.record_arm { TOKENS.record } else { TOKENS.solo });
        ui.interact(dot.intersect(clip), ui.id().with(("track_status", t.id.0)), Sense::hover()).on_hover_text(status.join("\n"));
        if !row.unresponsive.is_empty() {
            row.unresponsive_rect = r4(dot);
        }
        right -= 12.0;
    }
    let left = x + if t.is_folder() { 24.0 } else { 12.0 };
    let name = Rect::from_min_max(pos2(left, content.min.y), pos2(right.max(left), content.max.y));
    row.name_rect = r4(name);
    p.with_clip_rect(name.intersect(clip)).text(name.left_center(), Align2::LEFT_CENTER, &t.name, FontId::proportional(13.0), TOKENS.text);
    let response = ui.interact(name.intersect(clip), ui.id().with(("hdr", t.id.0)), Sense::click_and_drag());
    crate::help::hover(ui, &response, crate::help::Id::ArrangeHeaderName);
    let response =
        response.on_hover_text(format!("{}{}", t.name, if status.is_empty() { String::new() } else { format!("\n{}", status.join("\n")) }));
    if response.clicked() {
        let m = ui.input(|i| i.modifiers);
        app.click_header(t.id, m.shift, m.command || m.ctrl);
    }
    if response.double_clicked() {
        app.start_rename(crate::finish::RenameTarget::Track(t.id), row.name_rect);
    }
    if response.drag_started() && app.arrange.height_gesture.is_none() && t.kind != TrackKind::Master {
        app.arrange.header_drag = Some(crate::groups::HeaderDrag { track: t.id, drop: None });
    }
    if t.is_folder() {
        let triangle = Rect::from_center_size(pos2(x + 15.0, content.center().y), vec2(12.0, content.height().min(14.0)));
        super::tracks::disclosure(app, ui, t, triangle, clip, row);
    }
    crate::chrome::context_menu(&response).show(|ui| super::tracks::track_menu(app, ui, t.id, t.kind, t.instrument.is_some()));
}
