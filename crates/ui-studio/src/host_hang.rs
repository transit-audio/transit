//! A plug-in host thread that stopped responding (issue #205): the main window names the plug-in
//! and offers what is possible. Audio goes on (the instance processes on the audio thread), a save
//! keeps the plug-in's stored state, and "Give up" quarantines the stuck thread's plug-ins and
//! asks its windows to hide (`transit_plugin_host::Host::give_up`), so ending the ghosted editor
//! window with End Task, which ends transit, is never the only way out.

use crate::StudioApp;
use crate::arrange::{R4, r4};
use std::sync::Arc;
use std::time::Duration;
use transit_playback::{RELOG_EVERY, StuckCall};

/// The window as last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HostHangLayout {
    /// The window's text (empty: not shown).
    pub lines: Vec<String>,
    /// The "Give up" button's label (it names the plug-in and counts the others on its thread).
    pub give_up_label: String,
    pub save: Option<R4>,
    pub give_up: Option<R4>,
    pub later: Option<R4>,
}

/// A Later: the stuck call it was chosen for and how long that call had run then.
struct Later {
    thread: String,
    plugin: Arc<str>,
    call: &'static str,
    at: Duration,
}

impl Later {
    /// Whether this Later still hides `s` (#205 review): only the same call on the same thread,
    /// and only until the watchdog's next relog ([`RELOG_EVERY`]); a different stuck call, or the
    /// same one a minute later, shows the message again.
    fn hides(&self, s: &StuckCall) -> bool {
        self.thread == s.thread && self.plugin == s.plugin && self.call == s.call && s.stuck_for >= self.at && s.stuck_for < self.at + RELOG_EVERY
    }
}

/// What the window remembers.
#[derive(Default)]
pub struct HostHang {
    /// The stuck calls the user chose Later for, each hidden until the next relog.
    later: Vec<Later>,
    pub layout: HostHangLayout,
}

/// What a Give up ends on this platform: the window part is Windows only (macOS hosts inline and
/// Linux hosts no editor windows yet).
#[cfg(windows)]
const END_TASK: &str = "Do not end its window with End Task: that ends transit. Save now first.";
#[cfg(not(windows))]
const END_TASK: &str = "Save now first: transit cannot stop the plug-in's call.";
#[cfg(windows)]
const WINDOW_PART: &str = ", hides its window once Windows allows";
#[cfg(not(windows))]
const WINDOW_PART: &str = "";

/// The Give up button's label: the plug-in, and the other plug-ins its thread hosts (a Give up
/// quarantines them all, #205 review).
#[must_use]
pub fn give_up_label(s: &StuckCall) -> String {
    match s.others() {
        0 => format!("Give up on {}", s.plugin),
        1 => format!("Give up on {} and 1 other plug-in on its thread", s.plugin),
        n => format!("Give up on {} and {n} other plug-ins on its thread", s.plugin),
    }
}

/// The message for the stuck call `s`, with how many other host threads are stuck.
#[must_use]
pub fn lines(s: &StuckCall, others: usize) -> Vec<String> {
    let mut out = vec![
        format!("{} has not returned from {} for {} s.", s.plugin, s.call, s.stuck_for.as_secs()),
        "Audio keeps playing. Saving keeps the state this plug-in had when it was last read, so changes made in it since then are not saved.".into(),
        END_TASK.into(),
        format!(
            "Give up stops sending requests to the {} plug-in(s) on its thread until transit restarts (their audio keeps playing){WINDOW_PART}, and lets the plug-ins you add next load normally. If the plug-in may still recover (a dialog of its own), choose Later first.",
            s.others() + 1
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
/// chosen Later for this call in the last minute.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    app.host_hang.layout = HostHangLayout::default();
    let Some(p) = &app.player else { return };
    let all: Vec<StuckCall> = p.plugin_host_stuck().into_iter().filter(|s| !s.given_up).collect();
    // A Later lasts until the relog; one whose call ended or moved on is forgotten.
    app.host_hang.later.retain(|l| all.iter().any(|s| l.hides(s)));
    let stuck: Vec<&StuckCall> = all.iter().filter(|s| !app.host_hang.later.iter().any(|l| l.hides(s))).collect();
    let Some(first) = stuck.first().copied() else { return };
    let lines = lines(first, stuck.len() - 1);
    let give_up_label = give_up_label(first);
    let mut layout = HostHangLayout { lines: lines.clone(), give_up_label: give_up_label.clone(), ..HostHangLayout::default() };
    let mut act = None;
    crate::theme::window(ctx, "A plug-in is not responding")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
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
        });
    app.host_hang.layout = layout;
    let later = Later { thread: first.thread.clone(), plugin: Arc::clone(&first.plugin), call: first.call, at: first.stuck_for };
    let plugin = Arc::clone(&first.plugin);
    match act {
        Some(Act::Save) => {
            if app.save() {
                app.status = format!("Saved; {plugin} keeps the state it had when it was last read");
            }
        }
        Some(Act::GiveUp) => app.give_up_on_stuck_plugins(),
        Some(Act::Later) => app.host_hang.later.push(later),
        None => {}
    }
}

impl StudioApp {
    /// "Give up on <plug-in>" (issue #205): quarantines every stuck host thread's plug-ins and asks
    /// their windows to hide; the status line says what happened and how many plug-ins it took.
    pub fn give_up_on_stuck_plugins(&mut self) {
        let Some(p) = &self.player else { return };
        let gave = p.give_up_on_plugin_host();
        // The thread no longer counts as not responding: no "it has returned" message follows.
        self.host_wedged_shown = 0;
        let names: Vec<&str> = gave.plugins.iter().map(|n| &**n).collect();
        self.status = if names.is_empty() {
            "No plug-in host thread is stuck now".into()
        } else {
            let others = gave.quarantined.saturating_sub(names.len());
            let with = match others {
                0 => String::new(),
                1 => " and 1 other plug-in on its thread".into(),
                n => format!(" and {n} other plug-ins on its thread"),
            };
            format!(
                "Gave up on {}{with}: no more requests until transit restarts, their audio keeps playing and saves keep their stored state; new plug-ins load normally",
                names.join(", ")
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stuck(call: &'static str, secs: u64, hosted: usize) -> StuckCall {
        StuckCall {
            thread: "plugin-host-0".into(),
            plugin: Arc::from("Guitar Rig 7"),
            call,
            stuck_for: Duration::from_secs(secs),
            given_up: false,
            hosted,
        }
    }

    #[test]
    fn a_later_lasts_until_the_next_relog_and_only_for_its_call() {
        let s = stuck(transit_plugin_host::call::WINDOW_MESSAGE, 130, 6);
        let later = Later { thread: s.thread.clone(), plugin: Arc::clone(&s.plugin), call: s.call, at: s.stuck_for };
        assert!(later.hides(&s));
        assert!(later.hides(&stuck(s.call, 189, 6)), "within the minute");
        assert!(!later.hides(&stuck(s.call, 190, 6)), "back at the relog");
        assert!(!later.hides(&stuck(transit_plugin_host::call::EDITOR_OPEN, 130, 6)), "another call shows");
        assert!(!later.hides(&stuck(s.call, 5, 6)), "the thread stuck again in a new call shows");
        assert!(!later.hides(&StuckCall { thread: "plugin-host-1".into(), ..s.clone() }), "another thread shows");
    }

    #[test]
    fn the_give_up_label_counts_the_plug_ins_on_the_thread() {
        let call = transit_plugin_host::call::WINDOW_MESSAGE;
        assert_eq!(give_up_label(&stuck(call, 130, 1)), "Give up on Guitar Rig 7");
        assert_eq!(give_up_label(&stuck(call, 130, 2)), "Give up on Guitar Rig 7 and 1 other plug-in on its thread");
        assert_eq!(give_up_label(&stuck(call, 130, 6)), "Give up on Guitar Rig 7 and 5 other plug-ins on its thread");
    }
}
