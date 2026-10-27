use super::*;

/// The write pass, once per frame. While playing: tracks in Write write their volume and the
/// control being moved; Touch writes the control while it is held; Latch keeps writing it after
/// release until Stop. Points go in every [`WRITE_STEP_S`] with `from`, which replaces what the
/// pass runs over. The whole pass, and whatever is dragged during it, is one undo step.
pub(crate) fn write_pass(app: &mut StudioApp, ctx: &egui::Context) {
    use AutomationMode as M;
    let playing = app.is_playing();
    let held = ctx.input(|i| i.pointer.any_down());
    let now = ctx.input(|i| i.time);
    app.auto.touches.retain(|x| x.fresh || (x.hold == Hold::Pointer && held) || matches!(x.hold, Hold::Until(u) if now <= u));
    let any = app.engine.session().tracks.iter().any(|t| matches!(t.mixer.automation_mode, M::Write | M::Touch | M::Latch | M::TouchLatch));
    let may = playing && any;
    app.engine.hold_merge(may.then_some(PASS));
    if !may {
        // Stopped: nothing is written, and a click leaves no pulse for the next Play.
        app.auto.touches.iter_mut().for_each(|x| x.fresh = false);
        if app.auto.writing {
            app.auto.writing = false;
            let tracks = std::mem::take(&mut app.auto.passed);
            thin_after_pass(app, &tracks, true);
            app.engine.end_merge();
        }
        app.auto.passed.clear();
        app.auto.spans.clear();
        app.auto.last.clear();
        app.auto.last_v.clear();
        app.auto.skipped.clear();
        app.auto.latched.clear();
        app.auto.last_at = None;
        return;
    }
    // A track whose Write ended while others go on: thin what it wrote.
    let done: Vec<TrackId> = app
        .auto
        .passed
        .iter()
        .copied()
        .filter(|id| {
            !app.engine.session().track(*id).is_some_and(|t| matches!(t.mixer.automation_mode, M::Write | M::Touch | M::Latch | M::TouchLatch))
        })
        .collect();
    if !done.is_empty() {
        app.auto.passed.retain(|id| !done.contains(id));
        thin_after_pass(app, &done, true);
    }
    let pos = app.engine.transport.position;
    let step = app.engine.session().sample_rate.samples(WRITE_STEP_S);
    if let Some(last) = app.auto.last_at {
        if pos < last {
            // A loop or a locate: the next points start a new run.
            app.auto.last.clear();
            app.auto.last_v.clear();
            app.auto.skipped.clear();
        } else if pos - last < step {
            return;
        } else if pos - last > step * WRITE_GAP_STEPS {
            // A forward locate: a new segment, so the skipped range is not cleared.
            app.auto.last.clear();
            app.auto.last_v.clear();
            app.auto.skipped.clear();
        }
    }
    app.auto.last_at = Some(pos);
    let header = match &app.arrange.gesture {
        Some(crate::arrange::Gesture::Volume { track, .. }) => Some((*track, Lane::Volume)),
        _ => None,
    };
    let touched: Option<(TrackId, Lane)> = app.mixer.gesture.map(|g| Lane::of_target(g.target)).or(header);
    let mut touched: Vec<(TrackId, Lane)> = touched.filter(|_| held).into_iter().collect();
    touched.extend(app.auto.touches.iter().map(|x| (x.track, x.lane.clone())));
    for x in &mut app.auto.touches {
        x.fresh = false;
    }
    let s = app.engine.session_arc();
    // Touch stops writing on release: the next touch is a new segment, so the automation between
    // the touches is not cleared and the written span ends here.
    let released: Vec<(TrackId, String)> = app
        .auto
        .last
        .keys()
        .filter(|(id, lane)| {
            s.track(*id).is_some_and(|t| t.mixer.automation_mode == M::Touch) && !touched.iter().any(|(i, l)| i == id && l.id() == *lane)
        })
        .cloned()
        .collect();
    for k in released {
        app.auto.last.remove(&k);
        app.auto.last_v.remove(&k);
        app.auto.skipped.remove(&k);
    }
    let mut writes: Vec<(TrackId, Lane)> = Vec::new();
    for t in &s.tracks {
        let mine: Vec<Lane> = touched.iter().filter(|(id, _)| *id == t.id).map(|(_, l)| l.clone()).collect();
        match t.mixer.automation_mode {
            M::Write => {
                writes.push((t.id, Lane::Volume));
                writes.extend(mine.into_iter().filter(|l| *l != Lane::Volume).map(|l| (t.id, l)));
            }
            M::Touch => writes.extend(mine.into_iter().map(|l| (t.id, l))),
            M::Latch | M::TouchLatch => {
                for l in mine {
                    if !app.auto.latched.contains(&(t.id, l.id())) {
                        app.auto.latched.push((t.id, l.id()));
                    }
                }
                writes.extend(app.auto.latched.iter().filter(|(id, _)| *id == t.id).filter_map(|(id, l)| Lane::parse(l).map(|l| (*id, l))));
            }
            _ => {}
        }
    }
    let mut wrote = false;
    for (id, lane) in writes {
        let Some(t) = s.track(id) else { continue };
        if !lane.valid(t) {
            continue;
        }
        let key = (id, lane.id());
        let mut from = app.auto.last.get(&key).copied();
        let started = from.is_none();
        let v = lane.static_value(t);
        let held_lane = touched.iter().any(|(i, l)| *i == id && *l == lane);
        let mut ok = true;
        if let Some(old) = app.auto.last_v.get(&key).copied() {
            if !held_lane && (old - v).abs() <= f32::EPSILON {
                // Still: no point, but remember where the hold ran to.
                app.auto.skipped.insert(key, pos);
                continue;
            }
            if let Some(hold) = app.auto.skipped.remove(&key) {
                // The value moved after a hold: close the hold at its old value first.
                for (cmd, p) in lane.set_cmds(t, hold, old, from) {
                    ok &= app.engine.execute_merged(cmd, &p, PASS).is_ok();
                }
                from = Some(hold);
            }
        }
        app.auto.skipped.remove(&key);
        for (cmd, p) in lane.set_cmds(t, pos, v, from) {
            ok &= app.engine.execute_merged(cmd, &p, PASS).is_ok();
        }
        if ok {
            app.auto.last_v.insert(key.clone(), v);
            app.auto.writing = true;
            let segs = app.auto.spans.entry(key.clone()).or_default();
            match segs.last_mut() {
                Some(seg) if !started => seg.1 = pos,
                _ => segs.push((pos, pos)),
            }
            wrote = true;
            open_plugin_lane(app, id, &lane);
            if !app.auto.passed.contains(&id) {
                app.auto.passed.push(id);
            }
            app.auto.points_written += 1;
            app.auto.last.insert(key, pos);
        }
    }
    if wrote {
        app.auto.pass_mark = app.engine.revision;
    }
}

/// A plug-in lane that has just taken its first point opens under the track's other lanes, when
/// the track's automation is showing.
fn open_plugin_lane(app: &mut StudioApp, t: TrackId, lane: &Lane) {
    let id = lane.id();
    if matches!(lane, Lane::Plugin { .. }) && app.ui.auto_shown.contains(&t) && !app.ui.auto_lanes.iter().any(|v| v.track == t && v.lane == id) {
        app.ui.auto_lanes.push(LaneView { track: t, lane: id, height: LANE_H });
    }
}

/// The tolerance of every thinning (the engine's default, passed explicitly so the dry run agrees).
const THIN_TOLERANCE: f32 = 0.1;

/// How many points `automation.thin` would remove from `lane` (within `range`), without touching
/// the document: a thin that removes nothing must not enter the undo history.
fn removable(lane: Option<&transit_model::AutomationLane>, range: Option<(Samples, Samples)>) -> usize {
    let Some(l) = lane else { return 0 };
    let mut part = l.clone();
    if let Some((a, b)) = range {
        part.points.retain(|p| p.at >= a && p.at <= b);
    }
    part.thin(THIN_TOLERANCE)
}

/// `automation.thin` with the default tolerance on exactly what a pass wrote: per lane, per written
/// time span, so hand-drawn points outside it and the track's other lanes stay as they are. It runs
/// under the pass's merge key. Nothing happens when it would remove nothing, or when an Undo has
/// taken the pass off the stack (a Thin step would wipe Redo). When an edit or a mode change has
/// already closed the pass's undo step it would be a step of its own, so (with `squash`) it is
/// folded into the pass if nothing came after the pass, and otherwise left as a separate step
/// named "Thin".
pub(super) fn thin_after_pass(app: &mut StudioApp, tracks: &[TrackId], squash: bool) {
    let mut plan: Vec<(TrackId, String, Samples, Samples)> = Vec::new();
    let mut wrote = false;
    {
        let s = app.engine.session_arc();
        for id in tracks {
            let keys: Vec<(TrackId, String)> = app.auto.spans.keys().filter(|k| k.0 == *id).cloned().collect();
            for k in keys {
                let Some(segs) = app.auto.spans.remove(&k) else { continue };
                wrote = true;
                let (Some(t), Some(lane)) = (s.track(k.0), Lane::parse(&k.1)) else { continue };
                for (start, end) in segs.into_iter().filter(|(a, b)| b > a) {
                    for param in lane.param_ids(t) {
                        if removable(AutoParam::parse(&param).and_then(|p| t.lane(&p)), Some((start, end))) > 0 {
                            plan.push((k.0, param, start, end));
                        }
                    }
                }
            }
        }
    }
    // The session handle is dropped: holding it would make every command copy the document.
    if !wrote || app.engine.can_redo() {
        return;
    }
    let contiguous = app.engine.revision == app.auto.pass_mark;
    let top0 = app.engine.undo_label().map(str::to_string);
    let mut removed = 0;
    if app.ui.thin_after_write {
        for (id, param, start, end) in plan {
            let p = json!({"tracks": [id.0], "param": param, "start": start, "end": end, "tolerance": THIN_TOLERANCE});
            if let Ok(r) = app.engine.execute_merged("automation.thin", &p, PASS) {
                removed += r["removed"].as_u64().unwrap_or(0) as usize;
            }
        }
        app.auto.thinned = removed;
    }
    // The thins share the pass's merge key, so they add at most one step: a "Thin" on top of a
    // stack that did not end in one.
    let added = usize::from(app.engine.undo_label() == Some("Thin") && top0.as_deref() != Some("Thin"));
    let plural = if removed == 1 { "" } else { "s" };
    if squash && contiguous {
        // The pass is one step labeled for what it is, thinned or not.
        let n = (added + 1).min(app.engine.undo_history().len());
        if n > 0 {
            let _ = app.engine.squash_undo(n, "Write Automation");
        }
        if removed > 0 {
            app.status = format!("Thinned the write pass: {removed} point{plural} removed");
        }
    } else if added > 0 {
        app.status =
            format!("Thinned the write pass as its own step (Thin), since the pass is no longer the newest step: {removed} point{plural} removed");
    } else if removed > 0 {
        app.status = format!("Thinned the write pass: {removed} point{plural} removed");
    }
}

impl StudioApp {
    /// The time spans the running pass has written to `t`, over all its lanes (tests).
    #[cfg(test)]
    pub(crate) fn pass_spans(&self, t: TrackId) -> Vec<(Samples, Samples)> {
        let mut v: Vec<_> = self.auto.spans.iter().filter(|(k, _)| k.0 == t).flat_map(|(_, s)| s.iter().copied()).collect();
        v.sort_unstable();
        v
    }

    /// A write pass is recording on `t` (Thin Focused Track waits for it).
    pub fn pass_on(&self, t: TrackId) -> bool {
        self.auto.writing && self.auto.passed.contains(&t)
    }

    /// Automation > Thin Focused Track: thin the focused track's lanes now, one undo step.
    pub fn thin_focused(&mut self) -> Result<usize, String> {
        let Some(t) = self.focus_track() else { return Err("No focused track".into()) };
        if self.pass_on(t) {
            return Err("A write pass is running on this track; stop it first".into());
        }
        let s = self.engine.session_arc();
        let any = s.track(t).is_some_and(|tr| tr.automation.iter().any(|l| removable(Some(l), None) > 0));
        drop(s);
        if !any {
            self.status = "Nothing to thin".into();
            return Ok(0);
        }
        let r = self.run("automation.thin", json!({"tracks": [t.0], "tolerance": THIN_TOLERANCE}))?;
        let n = r["removed"].as_u64().unwrap_or(0) as usize;
        self.status = format!("Thinned the automation: {n} point{} removed", if n == 1 { "" } else { "s" });
        Ok(n)
    }
}

/// A control the user is moving ran a command under the pass's merge key: it is part of the pass,
/// so the pass is still the newest step.
pub(crate) fn note_pass_edit(app: &mut StudioApp) {
    if app.auto.writing {
        app.auto.pass_mark = app.engine.revision;
    }
}

/// Whether a write pass holds the undo merge (the frame must not end it).
pub(crate) fn passing(app: &StudioApp) -> bool {
    app.auto.writing
}
