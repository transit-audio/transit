//! The Instrument view's keys on the focused Sampler (README, Sampler views, round 2 "Keyboard";
//! Help › Keyboard Shortcuts, the Sampler and Sampler instrument scopes):
//!
//! - **Escape** cancels a key-map or waveform handle drag (nothing is sent; the window stays).
//! - **Left, Right** select the zone beside the selected one on the key map; **Up, Down** the
//!   next velocity layer over the same keys ([`super::key_map::neighbor`]). UI state: nothing sent.
//! - **Alt+Left, Alt+Right** nudge the zone waveform's handle last pressed (S until then) one
//!   frame, **Shift+Alt** ten: one `mix.sampler_zone_edit` per press, as the Slices view's nudges.
//!   A slice-bound zone's handles do not move (they are locked, as on the waveform).
//! - **Delete** asks to remove the selected zone (the destructive-action question: the default
//!   button names the loss, Escape cancels, Undo brings it back); the answer sends one
//!   `mix.sampler_zone_clear`.
//! - **Enter, Escape** answer the Group panel's Notice while it asks about a pad's zone: Make
//!   group (one step) and Cancel (nothing sent); Escape is then the Notice's, so the window stays.
//!
//! Never while typing, a modal or `app.dialog` is open, or another window has the keyboard. While
//! a menu or other popup is open the view's keys are taken and do nothing, so they never reach the
//! arrangement (Delete deleting clips, Alt+arrows nudging them); Escape stays the popup's. During a
//! drag only Escape acts.

use super::group_panel::Intent;
use super::key_map::{self, Toward};
use super::zone_wave::Handle;
use super::{refresh, send};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::shortcuts::{self, Action};
use serde_json::json;

/// The question's default button.
pub const DELETE_LABEL: &str = "Delete Zone";

/// The question's id for instance `uid` (its layout is read by `chrome::modal_layout`).
pub fn confirm_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_delete_zone", uid))
}

/// The remove question's words when the removal will be refused.
pub fn refusal_detail(reason: &str) -> String {
    format!("This will be refused: {reason}.")
}

/// The Sampler window of instance `uid`, when it is shown.
fn window_of(app: &StudioApp, uid: u64) -> Option<(transit_model::TrackId, usize)> {
    app.ui.plugin_windows.iter().copied().find(|key| app.builtin_at(*key).is_some_and(|i| i.uid.get() == uid && super::super::handles(&i.plugin)))
}

/// Runs `f` with the instance's device.
fn with_dev(app: &mut StudioApp, key: (transit_model::TrackId, usize), f: impl FnOnce(&mut StudioApp, Dev)) {
    let Some(ins) = app.builtin_at(key) else { return };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return };
    f(app, Dev { key, ins: &ins, info });
}

/// The remove question, while one is open: drawn every frame until answered; it goes when its
/// window closes or the view changes.
fn confirm(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    let Some(zone) = app.builtin_editor.devices.sampler.views.get(&uid).and_then(|v| v.instrument.confirm) else { return };
    let key = window_of(app, uid);
    let on_view = app.builtin_editor.devices.sampler.views.get(&uid).is_some_and(|v| v.view == super::super::View::Instrument);
    let Some(key) = key.filter(|_| on_view) else {
        super::super::view(app, uid).instrument.confirm = None;
        return;
    };
    let mc = app.ui.middle_c;
    let data = super::super::view(app, uid).instrument.snapshot.data.clone();
    let label = data.as_ref().and_then(|s| s.zone(zone).map(|z| key_map::label(s, z, mc)));
    let question = match label {
        Some(l) => format!("Delete zone {l}?"),
        None => format!("Delete zone {zone}?"),
    };
    // What will happen, in the words of the checks the command's store runs: a removal they
    // refuse says so before the answer (the command is still sent; the engine decides).
    let refused = data.as_ref().and_then(|s| super::key_map_gestures::removal(s, zone, app.engine.session(), mc));
    let detail = refused.map_or_else(|| crate::menus::undo_detail(false).to_string(), |r| refusal_detail(&r));
    let ask = crate::chrome::Ask { question, detail: Some(detail), default: DELETE_LABEL, cancel: "Cancel", third: None };
    let Some(choice) = crate::chrome::modal(ctx, confirm_id(uid), &ask) else { return };
    super::super::view(app, uid).instrument.confirm = None;
    if choice == crate::chrome::Choice::Default {
        with_dev(app, key, |app, dev| {
            let mut v = std::mem::take(&mut super::super::view(app, uid).instrument);
            send(app, dev, &mut v, "mix.sampler_zone_clear", json!({ "zone": zone }));
            super::super::view(app, uid).instrument = v;
        });
    }
}

/// The view's keys (see the module notes). Runs before the device window's Escape and the
/// arrangement's keys.
pub(crate) fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    confirm(app, ctx);
    let Some(uid) = super::super::key_owner(app) else { return };
    let modal = ctx.memory(|m| m.top_modal_layer().is_some());
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() || modal {
        return;
    }
    let Some(key) = window_of(app, uid) else { return };
    if !super::super::has_patch(app.engine.session(), key) || super::super::view(app, uid).view != super::super::View::Instrument {
        return;
    }
    let iv = &mut super::super::view(app, uid).instrument;
    // A drag is live only while a button is down (a stale one never takes Escape from the window).
    let down = ctx.input(|i| i.pointer.any_down());
    let dragging = down && (iv.drag.is_some() || iv.wave_drag.is_some());
    if dragging && shortcuts::pressed(ctx, Action::SamplerCancelZoneDrag) {
        if let Some(d) = iv.drag.as_mut() {
            d.cancelled = true;
        }
        if let Some(d) = iv.wave_drag.as_mut() {
            d.cancelled = true;
        }
    }
    // The Group panel's Notice answers Enter and Escape while it asks (and only then, so Escape
    // otherwise stays the window's); under a popup the keys stay the popup's.
    let asking = iv.ask.is_some() && iv.ask == iv.selected && !egui::Popup::is_any_open(ctx) && !dragging;
    let make = asking && shortcuts::pressed(ctx, Action::SamplerMakeGroup);
    let cancel = asking && shortcuts::pressed(ctx, Action::SamplerCancelMakeGroup);
    // The view's keys are taken here, so they never reach the arrangement; they act only when no
    // menu or popup holds the keyboard and no drag is in progress.
    let to = [
        (Action::SamplerPreviousZone, Toward::Left),
        (Action::SamplerNextZone, Toward::Right),
        (Action::SamplerZoneAbove, Toward::Above),
        (Action::SamplerZoneBelow, Toward::Below),
    ]
    .into_iter()
    .fold(None, |hit, (a, t)| if shortcuts::pressed(ctx, a) { Some(t) } else { hit });
    let nudge = [
        (Action::SamplerNudgeHandleEarlier, -1i64),
        (Action::SamplerNudgeHandleLater, 1),
        (Action::SamplerNudgeHandleEarlierTen, -10),
        (Action::SamplerNudgeHandleLaterTen, 10),
    ]
    .into_iter()
    .fold(0, |n, (a, step)| if shortcuts::pressed(ctx, a) { n + step } else { n });
    let delete = shortcuts::pressed(ctx, Action::SamplerRemoveZone);
    if egui::Popup::is_any_open(ctx) || dragging {
        return;
    }
    with_dev(app, key, |app, dev| {
        refresh(app, dev);
        let mut v = std::mem::take(&mut super::super::view(app, uid).instrument);
        if let (Some(state), Some(sel)) = (v.snapshot.data.clone(), v.selected) {
            if (make || cancel)
                && let Some(z) = state.zone(sel).cloned()
            {
                let answer = if make { Intent::MakeGroup } else { Intent::Cancel };
                super::group_actions::apply(app, dev, &mut v, &state, &z, answer);
            }
            if let Some(to) = to
                && let Some(n) = key_map::neighbor(&state, sel, to)
            {
                v.selected = Some(n);
            }
            let z = v.selected.and_then(|n| state.zone(n).cloned());
            if let Some(z) = z.as_ref().filter(|z| nudge != 0 && z.slice.is_none())
                && let Some(span) = state.span(z)
            {
                let handle = v.handle.unwrap_or(Handle::Start);
                let (field, now) = if handle == Handle::Start { ("start", span.start) } else { ("end", span.end) };
                let frame = (now as i64 + nudge).max(0);
                send(app, dev, &mut v, "mix.sampler_zone_edit", json!({"zone": z.zone, "region": { field: frame }}));
            }
            if delete && let Some(z) = z {
                v.confirm = Some(z.zone);
            }
        }
        super::super::view(app, uid).instrument = v;
    });
}
