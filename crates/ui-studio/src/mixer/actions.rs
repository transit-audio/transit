use super::*;

impl StudioApp {
    /// The focused track (device chain, highlight): the last clicked one, else the cursor's
    /// track, else the first selected track.
    pub fn focus_track(&self) -> Option<TrackId> {
        let s = self.engine.session();
        [self.ui.focus_track, self.ui.cursor_track, s.edit.selected_tracks.first().copied()].into_iter().flatten().find(|t| s.track(*t).is_some())
    }

    /// The track the user chose (clicked or selected), for highlights: `ui.focus_track`, else the
    /// first selected track. Unlike [`Self::focus_track`] it ignores the edit cursor's track, so
    /// Escape drops the highlight while the cursor keeps its track.
    pub fn chosen_track(&self) -> Option<TrackId> {
        let s = self.engine.session();
        [self.ui.focus_track, s.edit.selected_tracks.first().copied()].into_iter().flatten().find(|t| s.track(*t).is_some())
    }

    /// Focus a track from either view: it becomes the selected track (as a click on its header
    /// does) and the cursor's track. Selection is not part of the saved document.
    pub fn focus(&mut self, t: TrackId) {
        self.ui.focus_track = Some(t);
        if self.engine.session().edit.selected_tracks != [t] {
            let _ = self.run("edit.select", json!({"tracks": [t.0]}));
        }
        let at = self.cursor();
        self.set_cursor(at, Some(t));
    }

    /// A click on a track's header (arrangement) or channel strip (mixer): focus it, and let
    /// Delete take it. `additive` (Shift) adds it to the selected tracks, or removes it again.
    pub fn focus_header(&mut self, t: TrackId, additive: bool) {
        if !additive {
            self.focus(t);
            self.tracks_selected_by_header = true;
            return;
        }
        let mut tracks: Vec<u64> = self.engine.session().edit.selected_tracks.iter().map(|x| x.0).collect();
        if let Some(i) = tracks.iter().position(|x| *x == t.0) {
            tracks.remove(i);
        } else {
            tracks.push(t.0);
        }
        self.ui.focus_track = Some(t);
        let _ = self.run("edit.select", json!({"tracks": tracks}));
        self.tracks_selected_by_header = !tracks.is_empty();
    }

    /// Balance knob (L10): both panners of a stereo track, one undo step per gesture.
    pub fn set_balance(&mut self, t: TrackId, b: f32) -> Result<Value, String> {
        self.set_balance_keyed(t, b, &format!("pan:{}", t.0))
    }

    fn set_balance_keyed(&mut self, t: TrackId, b: f32, key: &str) -> Result<Value, String> {
        let n = self.engine.session().track(t).map_or(0, |tr| tr.mixer.pan.len());
        let key = key.to_string();
        let mut last = json!({});
        for (i, p) in pans_for(b, n).into_iter().enumerate() {
            last = self.engine.execute_merged("mix.pan", &json!({"tracks": [t.0], "pan": p, "index": i}), &key).map_err(|e| e.to_string())?;
        }
        Ok(last)
    }

    /// What a control holds now: dB for a fader or send, balance for a knob.
    pub fn value_of(&self, t: Target) -> Option<f32> {
        let s = self.engine.session();
        match t {
            Target::Fader(id) => s.track(id).map(|tr| tr.mixer.volume_db),
            Target::Pan(id) => s.track(id).map(|tr| balance_of(&tr.mixer.pan)),
            Target::Send(id, slot) => s.track(id)?.mixer.sends.get(slot)?.as_ref().map(|x| x.level_db),
            Target::SendPan(id, slot) => s.track(id)?.mixer.sends.get(slot)?.as_ref().map(|x| x.pan),
        }
    }

    /// What a control shows: the automated value at the playhead under Read, else [`Self::value_of`].
    pub fn shown_value(&self, t: Target) -> Option<f32> {
        let (id, lane) = crate::automation::Lane::of_target(t);
        let s = self.engine.session_arc();
        s.track(id).and_then(|tr| self.followed(tr, &lane)).or_else(|| self.value_of(t))
    }

    /// Set a control (dB, or balance for a knob) as part of a merged gesture: `key` is the
    /// gesture's merge key, by default the control's own drag key.
    pub fn set_value_merged(&mut self, t: Target, v: f32, key: Option<&str>) -> Result<Value, String> {
        let r = self.set_value_keyed(t, v, key);
        // The touched control's own commands belong to a running pass.
        crate::automation::note_pass_edit(self);
        r
    }

    fn set_value_keyed(&mut self, t: Target, v: f32, key: Option<&str>) -> Result<Value, String> {
        match t {
            Target::Fader(id) => {
                let key = key.map_or_else(|| fader_key(id), str::to_string);
                self.engine.execute_merged("mix.volume", &json!({"tracks": [id.0], "db": v}), &key).map_err(|e| e.to_string())
            }
            Target::Send(id, slot) => {
                let key = key.map_or_else(|| format!("send:{}:{slot}", id.0), str::to_string);
                self.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": v}), &key).map_err(|e| e.to_string())
            }
            Target::Pan(id) => {
                let key = key.map_or_else(|| format!("pan:{}", id.0), str::to_string);
                self.set_balance_keyed(id, v, &key)
            }
            Target::SendPan(id, slot) => {
                let key = key.map_or_else(|| format!("sendpan:{}:{slot}", id.0), str::to_string);
                self.engine
                    .execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "pan": v.clamp(-1.0, 1.0)}), &key)
                    .map_err(|e| e.to_string())
            }
        }
    }

    /// Set a control to `v` as one undo step of its own (a reset, or a typed value).
    pub fn set_value(&mut self, t: Target, v: f32) -> Result<Value, String> {
        self.engine.end_merge();
        self.mixer.wheel = None;
        self.mixer.serial += 1;
        let r = self.set_value_merged(t, v, Some(&format!("set:{}", self.mixer.serial)));
        self.engine.end_merge();
        r
    }

    /// Reset a control to its default: 0.0 dB, or center.
    pub fn reset_control(&mut self, t: Target) {
        let _ = self.set_value(t, 0.0);
    }

    /// A wheel notch or several on a control: 0.1 dB each (Shift: 0.01; a knob 1 %, Shift 0.1 %).
    /// The nudges of one wheel gesture are one undo step.
    pub(super) fn wheel_nudge(&mut self, t: Target, notches: f32, shift: bool, rect: Rect, now: f64) {
        let Some(cur) = self.shown_value(t) else { return };
        let step = match (t.is_pan(), shift) {
            (true, false) => fine::WHEEL_PAN,
            (true, true) => fine::WHEEL_PAN_FINE,
            (false, false) => fine::WHEEL_DB,
            (false, true) => fine::WHEEL_DB_FINE,
        };
        let (lo, hi) = if t.is_pan() { (-1.0, 1.0) } else { (fine::DB_MIN, fine::DB_MAX) };
        let v = fine::nudge(cur, notches, step).clamp(lo, hi);
        if self.mixer.wheel.map(|w| w.target) != Some(t) && !crate::automation::passing(self) {
            self.engine.end_merge();
        }
        self.mixer.wheel = Some(fine::WheelMerge { target: t, rect, last: now });
        let _ = self.set_value_merged(t, v, Some(&t.wheel_key()));
    }

    /// Ends a wheel gesture when the pointer has left the control or 500 ms have passed since
    /// its last notch. Returns whether one is still going (its merge must not be ended yet).
    pub(crate) fn drive_wheel(&mut self, ctx: &egui::Context, now: f64) -> bool {
        let Some(w) = self.mixer.wheel else { return false };
        let inside = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| w.rect.contains(p));
        if inside && now - w.last <= fine::WHEEL_MERGE_S {
            ctx.request_repaint_after_secs((fine::WHEEL_MERGE_S - (now - w.last)).max(0.0) as f32 + 0.02);
            return true;
        }
        self.mixer.wheel = None;
        if !crate::automation::passing(self) {
            self.engine.end_merge();
        }
        false
    }

    /// Route a track's output (`track.output`), by the value an output choice carries. Picking
    /// what the output already is changes nothing (no document edit, no undo step).
    pub fn set_output(&mut self, t: TrackId, value: &str) -> Result<Value, String> {
        let s = self.engine.session_arc();
        if let Some(tr) = s.track(t) {
            let cur = route_label(&s, &tr.mixer.output, false, tr.channels());
            let label = output_choices(&s, tr, self.engine.io_map())
                .into_iter()
                .find_map(|choice| match choice {
                    RoutingChoice::Entry { label, value: v } if v == value => Some(label),
                    _ => None,
                })
                .unwrap_or_else(|| value.to_string());
            if same_route(&s, &tr.mixer.output, &cur, value, &label) {
                return Ok(json!({"unchanged": true}));
            }
        }
        self.run("track.output", json!({"tracks": [t.0], "output": value}))
    }

    /// Route a track's input (`track.input`). Picking what the input already reads as (an audio
    /// track's default "In 1-2", L15) changes nothing.
    pub fn set_input(&mut self, t: TrackId, value: &str) -> Result<Value, String> {
        let s = self.engine.session_arc();
        if let Some(tr) = s.track(t) {
            let ch = if tr.kind == TrackKind::Audio { tr.channels() } else { 0 };
            let cur = route_label(&s, &tr.mixer.input, true, ch);
            let label = input_choices(&s, tr, self.device_inputs(), self.engine.io_map())
                .into_iter()
                .find_map(|choice| match choice {
                    RoutingChoice::Entry { label, value: v } if v == value => Some(label),
                    _ => None,
                })
                .unwrap_or_else(|| value.to_string());
            if same_route(&s, &tr.mixer.input, &cur, value, &label) {
                return Ok(json!({"unchanged": true}));
            }
        }
        self.run("track.input", json!({"tracks": [t.0], "input": value}))
    }

    /// Add a device at the end of the track's chain: the slot after the last occupied one, so it
    /// lands after every device already there. Ten slots at most (plan 06, L1): when the last
    /// slot is taken but an earlier one is free, the device goes there and the status line says so.
    pub fn add_device(&mut self, t: TrackId, plugin: &str) -> Result<Value, String> {
        let ins = self.engine.session().track(t).map(|tr| tr.mixer.inserts.clone()).unwrap_or_default();
        let next = ins.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
        if next < ins.len() {
            return self.run("mix.insert", json!({"track": t.0, "slot": next, "plugin": plugin}));
        }
        let r = self.run("mix.insert", json!({"track": t.0, "plugin": plugin}));
        if r.is_ok() {
            self.status =
                "The last device slot is in use, so the new device went into a free slot earlier in the chain (ten slots, plan 06 L1).".into();
        }
        r
    }

    pub fn toggle_bypass(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        self.run("mix.insert_bypass", json!({"track": t.0, "slot": slot}))
    }

    /// Sets (or with `from` `None` clears) the sidechain key of `t`'s insert in `slot`: one
    /// `mix.insert_key`. The engine refuses a key that would close a loop (or comes from a
    /// source that renders later); the reason goes to the status line.
    pub fn set_insert_key(&mut self, t: TrackId, slot: usize, from: Option<KeyFrom>, pre: bool) -> Result<Value, String> {
        // A bus key is always post-fader.
        let pre = pre && !matches!(from, Some(KeyFrom::Bus(_)));
        let from = match from {
            None => Value::Null,
            Some(KeyFrom::Track(id)) => json!({"track": id.0}),
            Some(KeyFrom::Bus(id)) => json!({"bus": id.0}),
        };
        self.run("mix.insert_key", json!({"track": t.0, "slot": slot, "from": from, "tap": if pre { "pre" } else { "post" }}))
    }

    /// Reorder (`copy` false: `mix.insert_move`) or duplicate (`copy` true: `mix.insert_copy`) a
    /// device in `t`'s chain, one undo step. Open and wanted plug-in editors follow their inserts
    /// to the new slots (matched by `Insert.uid`, not by slot).
    pub fn reorder_device(&mut self, t: TrackId, from: usize, to: usize, copy: bool) -> Result<Value, String> {
        let uids = |app: &Self| -> Vec<Option<u64>> {
            app.engine.session().track(t).map(|tr| tr.mixer.inserts.iter().map(|i| i.as_ref().map(|i| i.uid.get())).collect()).unwrap_or_default()
        };
        if copy {
            // The copy clones `Insert.state`, which only a capture refreshes: take the live
            // state (editor tweaks included) first, as save does.
            self.capture_plugin_states()?;
        }
        let before = uids(self);
        let id = if copy { "mix.insert_copy" } else { "mix.insert_move" };
        let r = self.run(id, json!({"track": t.0, "from": from, "to": to}));
        if r.is_ok() {
            let after = uids(self);
            let follow = |key: (TrackId, usize)| -> (TrackId, usize) {
                let new = (key.0 == t).then(|| before.get(key.1).copied().flatten()).flatten().and_then(|u| after.iter().position(|a| *a == Some(u)));
                (key.0, new.unwrap_or(key.1))
            };
            for k in &mut self.ui.plugin_windows {
                *k = follow(*k);
            }
            let settle = self.last_frame.unwrap_or_default() + 3.0;
            for e in &mut self.editors {
                let key = follow(e.key);
                if key != e.key {
                    e.key = key;
                    e.settle_until = settle;
                }
            }
        }
        r
    }

    /// Remove a device; its editor window closes first.
    pub fn remove_device(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        if let Some(p) = &self.player {
            self.player_requests.result(&mut self.status, p.close_editor(t, slot)).ok_or_else(|| self.status.clone())?;
        }
        self.ui.plugin_windows.retain(|k| *k != (t, slot));
        self.run("mix.insert_remove", json!({"track": t.0, "slot": slot}))
    }

    /// Put `plugin` on an instrument track as its instrument (`mix.instrument`, one undo step),
    /// replacing the one there. The old instrument's editor window closes first. Picking the
    /// instrument the track already has changes nothing.
    pub fn set_instrument(&mut self, t: TrackId, plugin: &str) -> Result<Value, String> {
        if self.engine.session().track(t).and_then(|tr| tr.instrument.as_ref()).is_some_and(|i| i.plugin == plugin) {
            return Ok(json!({"unchanged": true}));
        }
        let key = (t, transit_mix::INSTRUMENT_SLOT);
        if let Some(p) = &self.player {
            self.player_requests.result(&mut self.status, p.close_editor(key.0, key.1)).ok_or_else(|| self.status.clone())?;
        }
        self.ui.plugin_windows.retain(|k| *k != key);
        self.editors.retain(|e| e.key != key);
        self.run("mix.instrument", json!({"track": t.0, "plugin": plugin}))
    }

    /// "+ FX Channel" (mixer button, Track menu, arrange quick-add): a new FX channel and its
    /// bus, with no send, whatever is selected or focused (Logic's new aux, Live's new return).
    /// The send comes from a strip's "+ Send" (see [`Self::add_send`]).
    pub fn add_fx_channel(&mut self) -> Result<TrackId, String> {
        self.new_fx_channel(None, 0.0)
    }

    /// New FX Channel (plan 06, L5): a bus, an FX (aux) channel listening to it and, from `from`,
    /// a send to it at `level_db`, as one undo step. Returns the new channel's track id.
    ///
    /// When `from` has no free send slot nothing is made. If a step fails anyway, the steps
    /// already run are undone, so no bus or channel is left behind. The undo step reads "New Bus"
    /// (its first command's label): the engine has no way to name a composite (plan 06, L20).
    pub fn new_fx_channel(&mut self, from: Option<TrackId>, level_db: f32) -> Result<TrackId, String> {
        let s = self.engine.session_arc();
        let name =
            (1..).map(|i| format!("FX {i}")).find(|n| s.bus_by_name(n).is_none() && s.track_by_name(n).is_none()).unwrap_or_else(|| "FX".into());
        let slot = from.and_then(|f| s.track(f)).map(|t| (t.name.clone(), t.mixer.sends.iter().position(Option::is_none)));
        if let Some((track, None)) = &slot {
            self.status = format!("New FX Channel: all ten sends of {track} are in use");
            return Err(self.status.clone());
        }
        let slot = slot.and_then(|(_, s)| s);
        self.mixer.serial += 1;
        let key = format!("fx-channel:{}", self.mixer.serial);
        self.engine.end_merge();
        let r = (|| {
            let e = &mut self.engine;
            e.execute_merged("mix.new_bus", &json!({"name": name}), &key)?;
            let r = e.execute_merged("track.new", &json!({"kind": "aux", "format": "stereo", "name": name}), &key)?;
            let id = r.get("tracks").and_then(|t| t.get(0)).and_then(Value::as_u64).unwrap_or(0);
            e.execute_merged("track.input", &json!({"tracks": [id], "input": name}), &key)?;
            if let Some(f) = from {
                let mut p = json!({"track": f.0, "bus": name, "level_db": level_db});
                if let Some(slot) = slot {
                    p["slot"] = json!(slot);
                }
                e.execute_merged("mix.send", &p, &key)?;
            }
            Ok::<u64, transit_engine::EngineError>(id)
        })();
        self.engine.end_merge();
        match r {
            Ok(id) => {
                self.status = format!("Added {name}{}", from.map_or(String::new(), |_| " with a send to it".into()));
                Ok(TrackId(id))
            }
            Err(e) => {
                // Take back what the composite did so far (one merged undo step).
                if !std::sync::Arc::ptr_eq(&s, &self.engine.session_arc()) {
                    self.engine.undo();
                }
                self.status = format!("New FX Channel: {e}");
                Err(self.status.clone())
            }
        }
    }

    /// "+ Send": a send from `t` to bus `value` at 0 dB in the first free slot, or a new FX
    /// channel with its send from `t` at -12 dB for [`NEW_FX`], as one undo step.
    pub fn add_send(&mut self, t: TrackId, value: &str) -> Result<Value, String> {
        if value == NEW_FX {
            return self.new_fx_channel(Some(t), -12.0).map(|id| json!({"track": id.0}));
        }
        self.run("mix.send", json!({"track": t.0, "bus": value, "level_db": 0.0}))
    }

    /// Send slot `slot` somewhere else; its level and pre/post setting stay.
    pub fn retarget_send(&mut self, t: TrackId, slot: usize, bus: &str) -> Result<Value, String> {
        self.run("mix.send", json!({"track": t.0, "slot": slot, "bus": bus}))
    }

    pub fn remove_send(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        self.run("mix.send_remove", json!({"track": t.0, "slot": slot}))
    }

    /// A send's mute (slice 7): one undo step.
    pub fn set_send_mute(&mut self, t: TrackId, slot: usize, mute: bool) -> Result<Value, String> {
        self.run("mix.send_level", json!({"track": t.0, "slot": slot, "mute": mute}))
    }

    /// Pre-fader or post-fader (`mix.send_level {pre_fader}`).
    pub fn set_send_pre(&mut self, t: TrackId, slot: usize, pre: bool) -> Result<Value, String> {
        self.run("mix.send_level", json!({"track": t.0, "slot": slot, "pre_fader": pre}))
    }

    /// The open input's channel count (or the stand-in device's). Offline editing offers 8.
    pub(crate) fn device_inputs(&self) -> usize {
        self.recorder.as_ref().map_or_else(|| self.audio_devices.fake.as_ref().map_or(8, |f| f.input_channels_for(&self.ui.audio)), |r| r.channels)
    }
}
