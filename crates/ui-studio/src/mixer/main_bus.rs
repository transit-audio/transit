//! The main bus strip's monitoring parts (#366, the board's main strip): the Mono, Dim and Ref
//! toggles beside M in the 2 by 2 grid, and the footer line under the readouts with the live integrated loudness ("INT  −14.2 LUFS", "INT  —" before the
//! first gated block). Monitor state is a machine preference (`crate::main_monitor`): the toggles
//! run `mix.monitor`, which is never an undo step.

use super::channel::{NUMBER, text_in};
use super::*;
use crate::main_monitor::loudness_text;
use crate::tokens::TypeRole;
use crate::widgets::toggle::{self, Role};

/// The footer line: Mono 400 10 ("INT" in `text.faint`, the figure in `text`).
const FOOTER: TypeRole = NUMBER;

/// Mono, Dim and Ref in the grid cells after M (`cells`: the top right, bottom left and bottom
/// right cells), per the board's roles: Mono and Ref take the monitor role, Dim the mute role.
/// Alt+click on Ref stores the current monitor level as the reference.
pub(super) fn monitor_toggles(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, cells: [Rect; 3], sl: &mut StripLayout) {
    let m = app.ui.monitor.sane();
    let items = [
        ("Mono", Role::Monitor, m.mono, "mono", crate::help::Id::MixerStripMono),
        ("Dim", Role::Mute, m.dim, "dim", crate::help::Id::MixerStripDim),
        ("Ref", Role::Monitor, m.reference, "ref", crate::help::Id::MixerStripRef),
    ];
    for ((label, role, on, key, help), r) in items.into_iter().zip(cells) {
        let resp = ui.interact(r.intersect(clip), ui.id().with(("main_monitor", key)), Sense::click());
        let lay = toggle::paint(p, r, label, role, on, true);
        crate::help::hover(ui, &resp, help);
        let tip = match key {
            "mono" => "Mono: hear the mix summed to mono. Never in a bounce",
            "dim" => "Dim: the monitor level 20 dB down. Never in a bounce",
            _ => "Ref: hear the stored reference level. Alt+click stores the current monitor level as the reference",
        };
        let resp = resp.on_hover_text(tip);
        if resp.clicked() {
            let alt = ui.input(|i| i.modifiers.alt);
            let params = if key == "ref" && alt { json!({"store_ref": true}) } else { json!({ key: !on }) };
            let _ = app.run("mix.monitor", params);
        }
        match key {
            "mono" => sl.mono = r4(r),
            "dim" => sl.dim = r4(r),
            _ => sl.reference = r4(r),
        }
        sl.toggles.push(lay);
    }
}

/// The footer line: "INT" left, the figure right. A click resets the measurement.
pub(super) fn footer(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, r: Rect, sl: &mut StripLayout) {
    let text = loudness_text(app.loudness());
    let resp = ui.interact(r.intersect(clip), ui.id().with("main_loudness"), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripLoudness);
    text_in(p, &FOOTER, "INT", TOKENS.text_faint, r, Align2::LEFT_CENTER);
    text_in(p, &FOOTER, &text, TOKENS.text, r, Align2::RIGHT_CENTER);
    let tip = "Integrated loudness of the mix since the last reset. Click to reset; starting the transport from a stop resets it too";
    if resp.on_hover_text(tip).clicked() {
        app.reset_loudness();
    }
    sl.lufs_rect = r4(r);
    sl.lufs_text = text;
}
