//! Clip editing tools of the arrangement (#181 slice 1): Join / Consolidate, Crop, Trim to
//! cursor, Nudge, and the clip menu's Rename and Color. Each tool is one undo step, has a
//! shortcut-table row (or says why it has none), an Edit menu entry and a clip context menu entry,
//! is disabled when nothing applies, and says why on the status line when refused.
//!
//! Conventions (the Ableton/Logic rule, `docs/adr`): the tools follow what Ableton and Logic agree
//! on. Join is Ctrl+J (Ableton Consolidate, Logic Join). Crop is a menu entry only: Ableton and
//! Logic put it on different keys. Trim to cursor uses Logic's `[` and `]` (Ableton has no key).
//! Nudge uses Alt+Left/Right (plain arrows move the cursor here); Alt+Shift is the fine step.
//!
//! Engine commands: `edit.heal` joins neighbors that continue one file, `edit.consolidate` renders
//! or merges the rest; Crop and the trims go through `edit.trim_clip` per selected clip, so
//! unselected clips on the same track are left alone (`edit.trim_to_selection` is track-wide);
//! Nudge is `edit.move_clips` by the arrangement grid's step (`edit.nudge` uses the session's own
//! nudge value, not the grid the user sees).

use crate::StudioApp;
use crate::shortcuts::{Action as A, label};
use serde_json::{Value, json};
use transit_model::{ClipId, TrackId};
use transit_time::{Range, Samples};

/// The clip tools that have menu entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Join,
    Crop,
    TrimStart,
    TrimEnd,
    NudgeEarlier,
    NudgeLater,
}

impl Tool {
    pub const ALL: [Tool; 6] = [Tool::Join, Tool::Crop, Tool::TrimStart, Tool::TrimEnd, Tool::NudgeEarlier, Tool::NudgeLater];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Join => "Join / Consolidate",
            Tool::Crop => "Crop to Selection",
            Tool::TrimStart => "Trim Start to Cursor",
            Tool::TrimEnd => "Trim End to Cursor",
            Tool::NudgeEarlier => "Nudge Earlier",
            Tool::NudgeLater => "Nudge Later",
        }
    }

    /// The table action whose keys the menu shows, if the tool has any.
    pub fn action(self) -> Option<A> {
        match self {
            Tool::Join => Some(A::Join),
            Tool::Crop => None,
            Tool::TrimStart => Some(A::TrimStart),
            Tool::TrimEnd => Some(A::TrimEnd),
            Tool::NudgeEarlier => Some(A::NudgeEarlier),
            Tool::NudgeLater => Some(A::NudgeLater),
        }
    }
}

/// The colors of the clip menu's Color submenu: a name and RGB.
pub const CLIP_COLORS: [(&str, [u8; 3]); 8] = [
    ("Red", [214, 80, 80]),
    ("Orange", [222, 144, 70]),
    ("Yellow", [214, 196, 84]),
    ("Green", [96, 180, 100]),
    ("Teal", [76, 170, 170]),
    ("Blue", [86, 130, 214]),
    ("Purple", [150, 100, 200]),
    ("Pink", [212, 110, 164]),
];

impl StudioApp {
    /// Why `tool` can't run now, or None when it can (the menus disable on this, the shortcuts
    /// and menu clicks report it on the status line).
    pub fn clip_tool_blocked(&self, tool: Tool) -> Option<&'static str> {
        match tool {
            Tool::Join => self.join_plan().err(),
            Tool::Crop => self.crop_plan().err(),
            Tool::TrimStart => self.trim_plan(true).err(),
            Tool::TrimEnd => self.trim_plan(false).err(),
            Tool::NudgeEarlier => self.nudge_plan(-1, false).err(),
            Tool::NudgeLater => self.nudge_plan(1, false).err(),
        }
    }

    /// Run `tool` from a menu entry.
    pub fn run_clip_tool(&mut self, tool: Tool) -> Result<Value, String> {
        match tool {
            Tool::Join => self.join_selected(),
            Tool::Crop => self.crop_to_selection(),
            Tool::TrimStart => self.trim_to_cursor(true),
            Tool::TrimEnd => self.trim_to_cursor(false),
            Tool::NudgeEarlier => self.nudge_selected(-1, false),
            Tool::NudgeLater => self.nudge_selected(1, false),
        }
    }

    fn refuse<T>(&mut self, what: &str, why: &str) -> Result<T, String> {
        self.status = format!("{what}: {why}");
        Err(self.status.clone())
    }

    /// Run `steps` as one undo step; a failure undoes what ran and goes to the status line.
    fn run_as_one_step(&mut self, what: &str, steps: &[(&str, Value)]) -> Result<(), String> {
        let key = format!("clip-tool:{what}:{}", self.engine.undo_pushes());
        let before = self.engine.session_arc();
        self.engine.end_merge();
        let mut failed = None;
        for (id, params) in steps {
            if let Err(e) = self.engine.execute_merged(id, params, &key) {
                failed = Some(e.to_string());
                break;
            }
        }
        self.engine.end_merge();
        if let Some(e) = failed {
            if !std::sync::Arc::ptr_eq(&before, &self.engine.session_arc()) {
                self.engine.undo();
            }
            self.status = format!("{what}: {e}");
            return Err(self.status.clone());
        }
        Ok(())
    }

    // Join / Consolidate.

    /// What Join would do: the selected clips grouped by track (two or more on a track), or else
    /// the time selection on the selected tracks.
    fn join_plan(&self) -> Result<Vec<(TrackId, Range)>, &'static str> {
        const HINT: &str = "select two or more clips on a track, or a time range";
        let sel = self.selected_clips();
        if !sel.is_empty() {
            let mut out: Vec<(TrackId, Range)> = Vec::new();
            for t in self.engine.session().tracks.iter().map(|t| t.id) {
                let mine: Vec<&Range> = sel.iter().filter(|(x, ..)| *x == t).map(|(.., r)| r).collect();
                if mine.len() >= 2 {
                    let start = mine.iter().map(|r| r.start).min().unwrap_or(0);
                    let end = mine.iter().map(|r| r.end).max().unwrap_or(0);
                    out.push((t, Range::new(start, end)));
                }
            }
            return if out.is_empty() { Err(HINT) } else { Ok(out) };
        }
        let e = &self.engine.session().edit;
        if e.selection.is_empty() {
            return Err(HINT);
        }
        if e.selected_tracks.is_empty() {
            return Err("select a track for the time range");
        }
        Ok(e.selected_tracks.iter().map(|t| (*t, e.selection)).collect())
    }

    /// Selected adjacent clips become one clip: `edit.heal` for neighbors that continue one file,
    /// `edit.consolidate` for the rest (audio renders to one file, MIDI merges). A time selection
    /// with no clips selected consolidates everything inside it. One undo step.
    pub fn join_selected(&mut self) -> Result<Value, String> {
        let plan = match self.join_plan() {
            Ok(p) => p,
            Err(why) => return self.refuse("Join", why),
        };
        let clips_selected = !self.selected_clips().is_empty();
        let mut joined = 0;
        let key = format!("clip-tool:join:{}", self.engine.undo_pushes());
        let before = self.engine.session_arc();
        self.engine.end_merge();
        let mut failed = None;
        for (t, r) in &plan {
            let span = json!({"tracks": [t.0], "start": r.start, "end": r.end});
            let mut step = Ok(());
            if clips_selected {
                step = self.engine.execute_merged("edit.heal", &span, &key).map(drop);
            }
            if step.is_ok() && (!clips_selected || self.clips_in(*t, *r).len() >= 2) {
                step = self.engine.execute_merged("edit.consolidate", &span, &key).map(drop);
            }
            if let Err(e) = step {
                failed = Some(e.to_string());
                break;
            }
            joined += 1;
        }
        self.engine.end_merge();
        if let Some(e) = failed {
            if !std::sync::Arc::ptr_eq(&before, &self.engine.session_arc()) {
                self.engine.undo();
            }
            self.status = format!("Join: {e}");
            return Err(self.status.clone());
        }
        let ids: Vec<u64> = plan.iter().flat_map(|(t, r)| self.clips_in(*t, *r)).map(|c| c.0).collect();
        if clips_selected && !ids.is_empty() {
            let _ = self.run("edit.select", json!({"clips": ids}));
        }
        Ok(json!({"joined": joined}))
    }

    /// The clips of `t` lying inside `r`.
    fn clips_in(&self, t: TrackId, r: Range) -> Vec<ClipId> {
        let s = self.engine.session();
        s.track(t).map(|t| t.clips().iter().filter(|c| c.start >= r.start && c.end() <= r.end).map(|c| c.id).collect()).unwrap_or_default()
    }

    // Crop and Trim to cursor.

    /// The time selection on the arrangement's grid (unchanged when snapping is off or the snap
    /// would collapse it).
    fn snapped_selection(&self) -> Range {
        let s = self.engine.session();
        let r = s.edit.selection;
        let spp = self.arrange.layout.samples_per_px;
        let g = &self.arrange.grid;
        let snapped = Range::new(crate::automation::snap(s, spp, g, r.start), crate::automation::snap(s, spp, g, r.end));
        if snapped.is_empty() { r } else { snapped }
    }

    /// The selected clips and the range each would keep, for clips that `keep` actually shortens.
    fn trims(&self, keep: impl Fn(Range) -> Option<Range>) -> Vec<(ClipId, Range)> {
        self.selected_clips().into_iter().filter_map(|(_, id, r)| keep(r).filter(|k| !k.is_empty() && *k != r).map(|k| (id, k))).collect()
    }

    fn crop_plan(&self) -> Result<Vec<(ClipId, Range)>, &'static str> {
        if self.selected_clips().is_empty() {
            return Err("select the clips to crop");
        }
        let sel = self.snapped_selection();
        if sel.is_empty() {
            return Err("make a time selection first");
        }
        let plan = self.trims(|r| r.intersect(&sel));
        if plan.is_empty() { Err("the selected clips already lie inside the time selection") } else { Ok(plan) }
    }

    /// Trim the selected clips to the time selection (both edges), on the arrangement's grid.
    /// One undo step.
    pub fn crop_to_selection(&mut self) -> Result<Value, String> {
        let plan = match self.crop_plan() {
            Ok(p) => p,
            Err(why) => return self.refuse("Crop", why),
        };
        let steps: Vec<(&str, Value)> = plan.iter().map(|(c, r)| ("edit.trim_clip", json!({"clip": c.0, "start": r.start, "end": r.end}))).collect();
        self.run_as_one_step("Crop", &steps)?;
        Ok(json!({"trimmed": plan.len()}))
    }

    fn trim_plan(&self, start: bool) -> Result<Vec<(ClipId, Range)>, &'static str> {
        if self.selected_clips().is_empty() {
            return Err("select the clips to trim");
        }
        let at = self.cursor();
        let plan = self.trims(|r| (r.start < at && at < r.end).then(|| if start { Range::new(at, r.end) } else { Range::new(r.start, at) }));
        if plan.is_empty() { Err("put the cursor inside a selected clip") } else { Ok(plan) }
    }

    /// Trim the start (or end) of the selected clips that the cursor is inside to the cursor.
    /// One undo step.
    pub fn trim_to_cursor(&mut self, start: bool) -> Result<Value, String> {
        let what = if start { "Trim start" } else { "Trim end" };
        let plan = match self.trim_plan(start) {
            Ok(p) => p,
            Err(why) => return self.refuse(what, why),
        };
        let edge = if start { "start" } else { "end" };
        let steps: Vec<(&str, Value)> =
            plan.iter().map(|(c, r)| ("edit.trim_clip", json!({"clip": c.0, edge: if start { r.start } else { r.end }}))).collect();
        self.run_as_one_step(what, &steps)?;
        Ok(json!({"trimmed": plan.len()}))
    }

    // Nudge.

    /// The nudge in samples (`dir` is -1 or 1) and the clips it moves. The step is the
    /// arrangement grid's; with snapping off, or for the fine step, a quarter of it.
    fn nudge_plan(&self, dir: i64, fine: bool) -> Result<(Vec<ClipId>, Samples), &'static str> {
        let sel = self.selected_clips();
        let Some(first) = sel.iter().map(|(.., r)| r.start).min() else { return Err("select the clips to nudge") };
        let s = self.engine.session();
        let ticks = crate::automation::grid_ticks(s, self.arrange.layout.samples_per_px, &self.arrange.grid).max(1);
        let ticks = if fine || !self.arrange.grid.snap_enabled { (ticks / 4).max(1) } else { ticks };
        let t0 = s.tempo.samples_to_ticks(first, s.sample_rate);
        let by = if dir < 0 {
            -(first - s.tempo.tick_to_samples((t0 - ticks).max(0), s.sample_rate))
        } else {
            s.tempo.tick_to_samples(t0 + ticks, s.sample_rate) - first
        };
        if by == 0 {
            return Err("the clips are already at the start");
        }
        Ok((sel.iter().map(|(_, c, _)| *c).collect(), by))
    }

    /// Move the selected clips one grid step earlier (`dir` -1) or later (1); `fine` is a quarter
    /// step. One undo step.
    pub fn nudge_selected(&mut self, dir: i64, fine: bool) -> Result<Value, String> {
        let (ids, by) = match self.nudge_plan(dir, fine) {
            Ok(p) => p,
            Err(why) => return self.refuse("Nudge", why),
        };
        let ids: Vec<u64> = ids.iter().map(|c| c.0).collect();
        let r = self.run("edit.move_clips", json!({"clips": ids, "by": by}))?;
        let _ = self.run("edit.select", json!({"clips": ids}));
        Ok(r)
    }

    /// Color the selected clips (`None` restores the track's color).
    pub fn color_selected(&mut self, color: Option<[u8; 3]>) -> Result<Value, String> {
        let ids: Vec<u64> = self.selected_clips().iter().map(|(_, c, _)| c.0).collect();
        if ids.is_empty() {
            return self.refuse("Color", "select a clip first");
        }
        self.run("clip.color", json!({"clips": ids, "color": color}))
    }
}

/// One menu entry for `tool`: disabled with the reason as its hover text when nothing applies.
fn entry(app: &mut StudioApp, ui: &mut egui::Ui, tool: Tool) {
    let blocked = app.clip_tool_blocked(tool);
    let mut button = egui::Button::new(tool.label());
    if let Some(a) = tool.action() {
        button = button.shortcut_text(label(a));
    }
    let r = ui.add_enabled(blocked.is_none(), button).on_disabled_hover_text(blocked.unwrap_or_default());
    if r.clicked() {
        let _ = app.run_clip_tool(tool);
        ui.close();
    }
}

/// The tools as Edit menu entries.
pub fn edit_menu_items(app: &mut StudioApp, ui: &mut egui::Ui) {
    for tool in Tool::ALL {
        entry(app, ui, tool);
    }
}

/// The tools, Rename and Color as clip context menu entries (the right-clicked clip is selected).
pub fn context_menu_items(app: &mut StudioApp, ui: &mut egui::Ui) {
    for tool in Tool::ALL {
        entry(app, ui, tool);
    }
    let has_clip = !app.selected_clips().is_empty();
    if ui.add_enabled(has_clip, egui::Button::new("Rename").shortcut_text(label(A::Rename))).clicked() {
        app.rename_selected();
        ui.close();
    }
    ui.add_enabled_ui(has_clip, |ui| {
        ui.menu_button("Color", |ui| {
            if ui.button("Track color").clicked() {
                let _ = app.color_selected(None);
                ui.close();
            }
            for (name, rgb) in CLIP_COLORS {
                if ui.button(name).clicked() {
                    let _ = app.color_selected(Some(rgb));
                    ui.close();
                }
            }
        });
    });
}

/// Keyboard handling for the tools (called from [`crate::menus::shortcuts`]).
pub fn shortcuts(app: &mut StudioApp, ctx: &egui::Context) {
    use crate::shortcuts::pressed;
    if pressed(ctx, A::Join) {
        let _ = app.join_selected();
    }
    if pressed(ctx, A::TrimStart) {
        let _ = app.trim_to_cursor(true);
    }
    if pressed(ctx, A::TrimEnd) {
        let _ = app.trim_to_cursor(false);
    }
    // The fine step first: the rows match exact modifiers, so the order only reads better.
    if pressed(ctx, A::NudgeEarlierFine) {
        let _ = app.nudge_selected(-1, true);
    }
    if pressed(ctx, A::NudgeLaterFine) {
        let _ = app.nudge_selected(1, true);
    }
    if pressed(ctx, A::NudgeEarlier) {
        let _ = app.nudge_selected(-1, false);
    }
    if pressed(ctx, A::NudgeLater) {
        let _ = app.nudge_selected(1, false);
    }
}
