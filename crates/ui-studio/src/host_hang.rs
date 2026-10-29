//! A plug-in host thread that stopped responding (issue #205): the main window names the plug-in
//! and offers what is possible. Audio goes on (the instance processes on the audio thread), a save
//! keeps the plug-in's stored state, and "Give up" quarantines the stuck thread's plug-ins and
//! asks its windows to hide (`transit_plugin_host::Host::give_up`), so ending the ghosted editor
//! window with End Task, which ends transit, is never the only way out.

use crate::StudioApp;
use crate::arrange::{R4, r4};
use transit_playback::StuckCall;

/// The window as last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HostHangLayout {
    /// The window's text (empty: not shown).
    pub lines: Vec<String>,
    /// The "Give up" button's label (it names the plug-in).
    pub give_up_label: String,
    pub save: Option<R4>,
    pub give_up: Option<R4>,
    pub later: Option<R4>,
}

/// What the window remembers.
#[derive(Default)]
pub struct HostHang {
    /// The host threads the user chose Later for: their message is not shown again.
    later: Vec<String>,
    pub layout: HostHangLayout,
}

/// The message for the stuck call `s`, with how many other plug-ins are stuck.
#[must_use]
pub fn lines(s: &StuckCall, others: usize) -> Vec<String> {
    let mut out = vec![
        format!("{} has not returned from {} for {} s.", s.plugin, s.call, s.stuck_for.as_secs()),
        "Audio keeps playing. Saving keeps the state this plug-in had when it was last read, so changes made in it since then are not saved.".into(),
        "Do not end its window with End Task: that ends transit. Save now first.".into(),
        format!(
            "Give up on {} stops sending requests to the plug-ins on its thread until transit restarts (their audio keeps playing), hides its window once Windows allows, and lets the plug-ins you add next load normally.",
            s.plugin
        ),
    ];
    if others > 0 {
        out.push(format!("{others} more plug-in host thread(s) are not responding; Give up covers them too."));
    }
    out
}

#[derive(Clone, Copy)]
enum Act {
    Save,
    GiveUp,
    Later,
}

/// Draws the message while a host thread is stuck and the user has neither given up on it nor
/// chosen Later.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    app.host_hang.layout = HostHangLayout::default();
    let Some(p) = &app.player else { return };
    let stuck: Vec<StuckCall> = p.plugin_host_stuck().into_iter().filter(|s| !s.given_up && !app.host_hang.later.contains(&s.thread)).collect();
    let Some(first) = stuck.first() else { return };
    let lines = lines(first, stuck.len() - 1);
    let give_up_label = format!("Give up on {}", first.plugin);
    let mut layout = HostHangLayout { lines: lines.clone(), give_up_label: give_up_label.clone(), ..HostHangLayout::default() };
    let mut act = None;
    egui::Window::new("A plug-in is not responding").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            ui.set_max_width(460.0);
            for l in &lines {
                ui.label(l);
            }
            ui.add_space(crate::theme::SPACE);
            ui.horizontal(|ui| {
                let save = ui.button("Save now");
                let give_up = ui.button(&give_up_label);
                let later = ui.button("Later");
                layout.save = Some(r4(save.rect));
                layout.give_up = Some(r4(give_up.rect));
                layout.later = Some(r4(later.rect));
                for (r, a) in [(save, Act::Save), (give_up, Act::GiveUp), (later, Act::Later)] {
                    if r.clicked() {
                        act = Some(a);
                    }
                }
            });
        },
    );
    app.host_hang.layout = layout;
    match act {
        Some(Act::Save) => {
            if app.save() {
                app.status = format!("Saved; {} keeps the state it had when it was last read", first.plugin);
            }
        }
        Some(Act::GiveUp) => app.give_up_on_stuck_plugins(),
        Some(Act::Later) => app.host_hang.later.push(first.thread.clone()),
        None => {}
    }
}

impl StudioApp {
    /// "Give up on <plug-in>" (issue #205): quarantines every stuck host thread's plug-ins and asks
    /// their windows to hide; the status line says what happened.
    pub fn give_up_on_stuck_plugins(&mut self) {
        let Some(p) = &self.player else { return };
        let gave = p.give_up_on_plugin_host();
        // The thread no longer counts as not responding: no "it has returned" message follows.
        self.host_wedged_shown = 0;
        let names: Vec<&str> = gave.plugins.iter().map(|n| &**n).collect();
        self.status = if names.is_empty() {
            "No plug-in host thread is stuck now".into()
        } else {
            format!(
                "Gave up on {}: no more requests until transit restarts, its audio keeps playing and saves keep its stored state; new plug-ins load normally",
                names.join(", ")
            )
        };
    }
}
