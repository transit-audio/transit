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
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use transit_model::{ClipId, TakeStack, Track, TrackId};

/// A track's take stacks and its lane numbers (newest first), as the arrange view draws them.
#[derive(Debug, Default)]
pub(crate) struct TrackTakes {
    pub stacks: Vec<TakeStack>,
    pub numbers: Vec<u32>,
}

/// [`Track::take_stacks`] per track, kept until the document changes (#25): the arrange view asks
/// several times a frame per track, and a frame with no edit recomputes nothing. The key is the
/// engine's document revision (it bumps on every document change but zoom and scroll) and the
/// document's address (another engine or a replaced document never matches); only the live
/// document's tracks are cached, so a track from an older snapshot is computed, never stored.
#[derive(Default)]
pub(crate) struct TakeCache {
    key: Cell<(u64, usize)>,
    tracks: RefCell<HashMap<TrackId, Rc<TrackTakes>>>,
    /// How many times stacks were computed (tests count recomputations).
    #[cfg(test)]
    pub(crate) computed: Cell<usize>,
}

impl TakeCache {
    fn compute(&self, t: &Track) -> Rc<TrackTakes> {
        #[cfg(test)]
        self.computed.set(self.computed.get() + 1);
        if t.playlists.len() < 2 {
            return Rc::new(TrackTakes::default());
        }
        let stacks = t.take_stacks();
        let mut numbers: Vec<u32> = stacks.iter().flat_map(|s| s.takes.iter().map(|k| k.number)).collect();
        numbers.sort_unstable_by(|a, b| b.cmp(a));
        numbers.dedup();
        Rc::new(TrackTakes { stacks, numbers })
    }
}

impl StudioApp {
    /// `t`'s take stacks and lane numbers, from the cache while the document is unchanged.
    pub(crate) fn takes_of(&self, t: &Track) -> Rc<TrackTakes> {
        let doc = self.engine.session();
        if t.playlists.len() < 2 || !doc.track(t.id).is_some_and(|live| std::ptr::eq(live, t)) {
            return self.take_cache.compute(t);
        }
        let c = &self.take_cache;
        let key = (self.engine.revision, std::ptr::from_ref(doc) as usize);
        if c.key.get() != key {
            c.key.set(key);
            c.tracks.borrow_mut().clear();
        }
        if let Some(hit) = c.tracks.borrow().get(&t.id) {
            return Rc::clone(hit);
        }
        let v = c.compute(t);
        c.tracks.borrow_mut().insert(t.id, Rc::clone(&v));
        v
    }

    /// Whether the live document's track `t` has stacked takes.
    pub(crate) fn has_takes(&self, t: TrackId) -> bool {
        self.engine.session().track(t).is_some_and(|tr| !self.takes_of(tr).stacks.is_empty())
    }

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
        if self.has_takes(t) && !self.ui.takes_open.contains(&t) {
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
        if !self.has_takes(t) {
            self.ui.takes_open.retain(|x| *x != t);
        }
        let n = r["removed"].as_u64().unwrap_or(0);
        self.status = format!("Flattened the takes of {}: kept what plays, removed {n} clip{}", self.track_name(t), if n == 1 { "" } else { "s" });
        Ok(r)
    }

    /// Delete take `number` everywhere it does not play (refused where it plays).
    pub fn delete_take(&mut self, t: TrackId, number: u32) -> Result<Value, String> {
        let r = self.take_run("takes.delete", json!({"track": t.0, "take": number}))?;
        if !self.has_takes(t) {
            self.ui.takes_open.retain(|x| *x != t);
        }
        self.status = format!("Deleted take {number} of {}", self.track_name(t));
        Ok(r)
    }
}
