//! Pad menu edits. Clearing a zone also unbinds its pad; the metadata reset shares its undo step.

use super::{SamplerLayout, behavior, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::widgets::r4;
use serde_json::json;

/// Clear the selected binding and its metadata, preserving its absolute trigger note.
fn clear(app: &mut StudioApp, dev: Dev, index: u8) {
    let Ok(state) = app.engine.execute("mix.sampler_state", &json!({"track": dev.key.0.0})) else { return };
    let Some(pad) = state["pads"].as_array().and_then(|pads| pads.get(usize::from(index))) else { return };
    // Nothing to clear (no zone, no name, no choke group, mute and solo off): no step at all.
    let unset = pad["zone"].is_null() && pad["name"].is_null() && pad["choke"] == 0 && pad["mute"] == false && pad["solo"] == false;
    if unset {
        return;
    }
    let mut steps = Vec::new();
    if let Some(zone) = pad["zone"].as_u64() {
        // A zone has at most one pad binding. Clearing it unbinds that pad in the same checked
        // store, without temporarily making a pad zone play over its unbound key range.
        steps.push(("mix.sampler_zone_clear", json!({"zone": zone})));
    } else {
        steps.push(("mix.sampler_pad_unbind", json!({"pad": index + 1})));
    }
    steps.push(("mix.sampler_pad_set", json!({"pad": index + 1, "name": null, "choke": 0, "mute": false, "solo": false})));
    app.engine.end_merge();
    let before = app.engine.session_arc();
    let key = format!("sampler_clear:{}:{index}", dev.uid());
    for (id, mut params) in steps {
        params["track"] = json!(dev.key.0.0);
        if let Err(error) = app.execute_ui_merged(id, &params, &key) {
            app.engine.end_merge();
            if !std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()) {
                app.engine.undo();
            }
            let text = error.to_string();
            app.status = text.clone();
            view(app, dev.uid()).note = Some(text);
            return;
        }
    }
    app.engine.end_merge();
    view(app, dev.uid()).note = None;
}

pub(crate) fn menu(app: &mut StudioApp, response: &egui::Response, dev: Dev, index: u8, stored: bool, out: &mut SamplerLayout) {
    if response.secondary_clicked() {
        view(app, dev.uid()).selected = index;
    }
    let mut clear_clicked = false;
    let mut reset_clicked = false;
    crate::chrome::context_menu(response, false).show(|ui| {
        let shortcut = crate::shortcuts::rows(crate::shortcuts::Action::SamplerClearPad).map(crate::shortcuts::text).collect::<Vec<_>>().join(" / ");
        let row = ui.add(crate::chrome::MenuRow::new("Clear pad").shortcut(shortcut));
        out.pad_menu.push(("Clear pad".into(), r4(row.rect)));
        clear_clicked = row.clicked();
        if stored {
            let row = ui.add(crate::chrome::MenuRow::new("Reset note"));
            out.pad_menu.push(("Reset note".into(), r4(row.rect)));
            reset_clicked = row.clicked();
        }
    });
    if clear_clicked {
        clear(app, dev, index);
    } else if reset_clicked {
        behavior::set_pad(app, dev, index, json!({"note": null}));
    }
}

/// A pad drag lives only while a button is down (#481): one whose release no Pads view saw (its
/// window closed or its view switched under the held button) is dropped here, so it never swaps on
/// a later click. Escape during a pad drag cancels it: nothing is sent, the window stays (as the
/// Slices and Instrument views' drags). Runs every frame, before the device window's Escape.
pub(crate) fn drag_keys(app: &mut StudioApp, ctx: &egui::Context) {
    let (down, released) = ctx.input(|i| (i.pointer.any_down(), i.pointer.primary_released()));
    if !down {
        // The release frame keeps the drag for the Pads view's own drop (`behavior::end_drag`).
        if !released {
            app.builtin_editor.devices.sampler.views.values_mut().for_each(|v| v.drag = None);
        }
        return;
    }
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    let modal = ctx.memory(|m| m.top_modal_layer().is_some());
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() || modal {
        return;
    }
    let dragging = app.builtin_editor.devices.sampler.views.get(&uid).is_some_and(|v| v.drag.is_some());
    if dragging && crate::shortcuts::pressed(ctx, crate::shortcuts::Action::SamplerCancelZoneDrag) {
        view(app, uid).drag = None;
    }
}

/// Runs before arrangement shortcuts, only for the focused Sampler with its Pads view visible.
pub(crate) fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() {
        return;
    }
    let Some(key) =
        app.ui.plugin_windows.iter().copied().find(|key| app.builtin_at(*key).is_some_and(|i| i.uid.get() == uid && super::handles(&i.plugin)))
    else {
        return;
    };
    if !super::has_patch(app.engine.session(), key) || !super::view_bar::is_pads(app, uid) || view(app, uid).kit_preview.is_some() {
        return;
    }
    let pressed = crate::shortcuts::pressed(ctx, crate::shortcuts::Action::SamplerClearPad);
    // An open menu (the pad's own, or any other popup) holds the keyboard, as system menus do:
    // the key is taken here and does nothing, so it never reaches the arrangement's Delete.
    if pressed && !egui::Popup::is_any_open(ctx) {
        let Some(ins) = app.builtin_at(key) else { return };
        let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return };
        let selected = view(app, uid).selected;
        clear(app, Dev { key, ins: &ins, info }, selected);
    }
}
