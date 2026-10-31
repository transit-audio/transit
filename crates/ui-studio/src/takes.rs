//! Take lanes as app actions (#25, ADR 0039): show a track's take lanes, choose the take that
//! plays, flatten and delete a take. Every change runs a `takes.*` engine command, one undo step
//! each; showing the lanes is view state. The lanes are drawn by `arrange::take_lanes`.
//!
//! Conventions: Logic's Take Folder and Ableton's Take Lanes (Live 11) agree on lanes under the
//! track, one per take; a click on a take makes it the one that plays; Flatten keeps what plays
//! and removes the lanes. After cycle recording the track's lanes open (Logic opens the take
//! folder, Live shows the lanes) with the newest take playing.

use crate::StudioApp;
use serde_json::{Value, json};
use transit_model::{ClipId, TrackId};

impl StudioApp {
    /// Show or hide a track's take lanes (view state only).
    pub fn toggle_takes(&mut self, t: TrackId) {
        if let Some(i) = self.ui.takes_open.iter().position(|x| *x == t) {
            self.ui.takes_open.remove(i);
        } else {
            self.ui.takes_open.push(t);
        }
    }

    /// Show a track's take lanes if it has stacked takes (after a recording made some).
    pub fn show_takes(&mut self, t: TrackId) {
        let stacked = self.engine.session().track(t).is_some_and(|tr| !tr.take_stacks().is_empty());
        if stacked && !self.ui.takes_open.contains(&t) {
            self.ui.takes_open.push(t);
        }
    }

    pub(crate) fn track_name(&self, t: TrackId) -> String {
        self.engine.session().track(t).map_or_else(String::new, |t| t.name.clone())
    }

    fn take_run(&mut self, id: &str, p: Value) -> Result<Value, String> {
        self.run(id, p).inspect_err(|e| self.status.clone_from(e))
    }

    /// A take lane's clip was clicked: its take plays over the clip's range.
    pub fn choose_take_clip(&mut self, t: TrackId, clip: ClipId) -> Result<Value, String> {
        let r = self.take_run("takes.set_active", json!({"track": t.0, "clip": clip.0}))?;
        if let Some(n) = r["take"].as_u64() {
            self.status = format!("Take {n} plays on {}", self.track_name(t));
        }
        Ok(r)
    }

    /// A take lane's label was clicked: take `number` plays in every stack it is in.
    pub fn choose_take(&mut self, t: TrackId, number: u32) -> Result<Value, String> {
        let r = self.take_run("takes.set_active", json!({"track": t.0, "take": number}))?;
        self.status = format!("Take {number} plays on {}", self.track_name(t));
        Ok(r)
    }

    /// Flatten the track's takes (stack `stack`, or all): keep what plays, remove the other
    /// takes. The lanes close when no stack is left.
    pub fn flatten_takes(&mut self, t: TrackId, stack: Option<usize>) -> Result<Value, String> {
        let mut p = json!({"track": t.0});
        if let Some(i) = stack {
            p["stack"] = json!(i);
        }
        let r = self.take_run("takes.flatten", p)?;
        if self.engine.session().track(t).is_some_and(|tr| tr.take_stacks().is_empty()) {
            self.ui.takes_open.retain(|x| *x != t);
        }
        let n = r["removed"].as_u64().unwrap_or(0);
        self.status = format!("Flattened the takes of {}: kept what plays, removed {n} clip{}", self.track_name(t), if n == 1 { "" } else { "s" });
        Ok(r)
    }

    /// Delete take `number` everywhere it does not play (refused where it plays).
    pub fn delete_take(&mut self, t: TrackId, number: u32) -> Result<Value, String> {
        let r = self.take_run("takes.delete", json!({"track": t.0, "take": number}))?;
        if self.engine.session().track(t).is_some_and(|tr| tr.take_stacks().is_empty()) {
            self.ui.takes_open.retain(|x| *x != t);
        }
        self.status = format!("Deleted take {number} of {}", self.track_name(t));
        Ok(r)
    }
}
