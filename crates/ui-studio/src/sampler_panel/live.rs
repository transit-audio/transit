//! The pads' live state (README, Pad: Hit and Choked): each frame the panel reads the words the
//! instrument published through its tap (`transit_dsp::PadWord`, `ScopeHub::pad_activity`) and
//! compares each pad's counts with the ones it saw last. A new trigger restarts the pad's hit at its
//! velocity; a new cut drops the fill and inverts the badge. The ages are read from the frame clock
//! (`egui::InputState::time`), so a test steps them at a fixed dt.
//!
//! The first words read after the panel opens (or after the instance is replaced) are taken as
//! seen: what happened before the panel looked does not flash.

use super::view;
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::widgets::pad;
use transit_dsp::PadWord;

/// One pad's live state.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Live {
    /// The trigger and cut counts last seen (`None` before the first read).
    seen: Option<(u8, u8)>,
    /// The last trigger: velocity and frame time (s).
    hit: Option<(u8, f64)>,
    /// The last cut's frame time (s).
    cut: Option<f64>,
}

impl Live {
    /// Takes `word`, read at `now`.
    pub fn take(&mut self, word: PadWord, now: f64) {
        let counts = (word.hits(), word.chokes());
        if let Some((hits, chokes)) = self.seen {
            if counts.0 != hits {
                self.hit = Some((word.velocity(), now));
            }
            if counts.1 != chokes {
                self.cut = Some(now);
            }
        }
        self.seen = Some(counts);
    }

    /// The fill height (0..1) and whether the badge shows inverted, at `now`.
    pub fn look(&self, now: f64) -> (f32, bool) {
        let ms = |t: f64| ((now - t) * 1000.0) as f32;
        let fill = pad::live_fill(self.hit.map(|(v, t)| (v, ms(t))), self.cut.map(ms));
        (fill, self.cut.is_some_and(|t| pad::choked(ms(t))))
    }

    /// Whether anything still moves at `now` (the window keeps repainting).
    pub fn moving(&self, now: f64) -> bool {
        let (fill, choked) = self.look(now);
        fill > 0.0 || choked
    }
}

/// Reads the instrument's published pads into the view of `dev`.
pub(crate) fn update(app: &mut StudioApp, dev: Dev, now: f64) {
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let words = app.scope_hub().and_then(|h| h.pad_activity(dev.key.0, uid));
    let v = view(app, dev.uid());
    let Some(words) = words else {
        // No tap (the instance is being replaced): what comes next is seen fresh.
        v.live = [Live::default(); transit_dsp::PAD_SLOTS];
        return;
    };
    for (l, w) in v.live.iter_mut().zip(words) {
        l.take(PadWord(w), now);
    }
}

/// Whether any pad of the view of `uid` still moves.
pub(crate) fn moving(app: &StudioApp, uid: u64, now: f64) -> bool {
    app.builtin_editor.devices.sampler.views.get(&uid).is_some_and(|v| v.live.iter().any(|l| l.moving(now)))
}
