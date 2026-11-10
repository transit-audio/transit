//! Several MIDI clips in the piano roll at once (#329): every open clip's notes in the roll's ticks,
//! which clip each note is in, the foreground switch, and where new notes and copies go.

use super::*;

const MAX_OPEN_CLIPS: usize = 16;

/// The selected MIDI clips that may open, in track order then clip time, with `clip` in the
/// foreground when it is admitted. Returns the foreground, the other clips and the omitted count.
pub(super) fn open_set(s: &Session, clip: ClipId) -> (ClipId, Vec<ClipId>, usize) {
    if !s.edit.selected_clips.contains(&clip) {
        return (clip, Vec::new(), 0);
    }
    let mut clips = s
        .edit
        .selected_clips
        .iter()
        .filter_map(|id| {
            let (track, c) = s.find_clip(*id)?;
            if !matches!(c.content, ClipContent::Midi { .. }) {
                return None;
            }
            let track_order = s.tracks.iter().position(|t| t.id == track)?;
            Some((track_order, c.start, *id))
        })
        .collect::<Vec<_>>();
    clips.sort_unstable_by_key(|(track, start, id)| (*track, *start, id.0));
    let omitted = clips.len().saturating_sub(MAX_OPEN_CLIPS);
    clips.truncate(MAX_OPEN_CLIPS);
    let foreground = if clips.iter().any(|(_, _, id)| *id == clip) { clip } else { clips.first().map_or(clip, |(_, _, id)| *id) };
    let others = clips.into_iter().filter_map(|(_, _, id)| (id != foreground).then_some(id)).collect();
    (foreground, others, omitted)
}

/// One clip open in the roll, at `off` ticks from the foreground's start; its notes are
/// `notes[base..base + count]` of the [`Composite`].
#[derive(Debug, Clone)]
pub(super) struct Part {
    pub(super) clip: ClipId,
    pub(super) track: TrackId,
    pub(super) off: i64,
    /// The span the roll edits (a looped clip's first pass), from its own start.
    pub(super) len: i64,
    pub(super) base: usize,
    pub(super) count: usize,
    pub(super) color: [u8; 3],
}

/// Every open clip's notes in the roll's ticks (the foreground's first, then each other's shifted
/// by its offset), and where each clip's notes are. One clip open: its notes, unshifted.
pub(super) struct Composite {
    pub(super) notes: Vec<Note>,
    pub(super) parts: Vec<Part>,
}

impl Composite {
    pub(super) fn new(s: &Session, fg: ClipId, others: &[ClipId]) -> Option<Composite> {
        let f0 = clip_view(s, fg)?.c0;
        let mut out = Composite { notes: Vec::new(), parts: Vec::new() };
        for clip in std::iter::once(fg).chain(others.iter().copied().filter(|c| *c != fg)) {
            let Some(cv) = clip_view(s, clip) else { continue };
            let color = s.find_clip(clip).and_then(|(_, c)| c.color).or_else(|| s.track(cv.track).map(|t| t.color)).unwrap_or_default();
            let off = cv.c0 - f0;
            out.parts.push(Part { clip, track: cv.track, off, len: cv.len, base: out.notes.len(), count: cv.notes.len(), color });
            out.notes.extend(cv.notes.iter().map(|n| Note { start: n.start + off, ..*n }));
        }
        Some(out)
    }

    pub(super) fn several(&self) -> bool {
        self.parts.len() > 1
    }

    /// The clip note `i` of [`Composite::notes`] is in, and its index there.
    pub(super) fn locate(&self, i: usize) -> Option<(&Part, usize)> {
        self.parts.iter().find(|p| i >= p.base && i < p.base + p.count).map(|p| (p, i - p.base))
    }

    /// The notes of `want` found again (each matched once), grouped by clip in `want`'s order:
    /// each clip with the local indices of its notes.
    pub(super) fn groups(&self, want: &[Note]) -> Vec<(Part, Vec<usize>)> {
        let mut out: Vec<(Part, Vec<usize>)> = Vec::new();
        for i in resolve(&self.notes, want) {
            let Some((part, local)) = self.locate(i) else { continue };
            match out.iter_mut().find(|(p, _)| p.clip == part.clip) {
                Some((_, idx)) => idx.push(local),
                None => out.push((part.clone(), vec![local])),
            }
        }
        out
    }

    /// The clip each note of `want` is in (each note matched once; none for a note not found).
    pub(super) fn owners(&self, want: &[Note]) -> Vec<Option<&Part>> {
        let mut used = vec![false; self.notes.len()];
        want.iter()
            .map(|w| {
                let i = (0..self.notes.len()).find(|i| !used[*i] && self.notes[*i] == *w);
                i.inspect(|i| used[*i] = true).and_then(|i| self.locate(i)).map(|(p, _)| p)
            })
            .collect()
    }

    /// The offset of the clip each note of `want` is in (0 for a note not found).
    pub(super) fn offsets(&self, want: &[Note]) -> Vec<i64> {
        self.owners(want).into_iter().map(|p| p.map_or(0, |p| p.off)).collect()
    }

    /// The earliest of `want`'s starts in its own clip (how far left they may move).
    pub(super) fn local_min(&self, want: &[Note]) -> i64 {
        want.iter().zip(self.offsets(want)).map(|(n, off)| n.start - off).min().unwrap_or(0)
    }

    /// The clip a new note at `start` goes to: the foreground when it is the only one, else the
    /// foreground or the first other clip whose span holds `start`; none outside every clip.
    pub(super) fn target(&self, start: i64) -> Option<&Part> {
        if !self.several() {
            return self.parts.first();
        }
        self.parts.iter().find(|p| start >= p.off && start < p.off + p.len)
    }
}

impl StudioApp {
    /// An edit across several clips is all or nothing: a frozen track refuses it before any
    /// command runs (one clip alone gets the engine's own refusal, as before).
    pub(super) fn keys_refuse_frozen(&mut self, clips: &[ClipId]) -> Result<(), String> {
        let s = self.engine.session();
        let mut distinct: Vec<ClipId> = clips.to_vec();
        distinct.sort_unstable_by_key(|c| c.0);
        distinct.dedup();
        let frozen = distinct.len() > 1 && distinct.iter().any(|c| s.find_clip(*c).and_then(|(t, _)| s.track(t)).is_some_and(|t| t.freeze.is_some()));
        if frozen {
            self.status = "Track is frozen; unfreeze to edit".into();
            return Err(self.status.clone());
        }
        Ok(())
    }

    /// Every open clip's notes in the roll's ticks (#329).
    pub(super) fn keys_all(&self) -> Option<Composite> {
        Composite::new(self.engine.session(), self.keys.clip?, if self.keys.focus { &[] } else { &self.keys.others })
    }

    /// N: a quick press toggles Focus Mode; holding it focuses only until release (Live).
    pub(super) fn keys_focus_edge(&mut self, down: bool, now: f64) {
        if self.keys.others.is_empty() {
            return;
        }
        if down {
            if self.keys.focus_down.is_none() {
                let was = self.keys.focus;
                self.keys.focus_down = Some((now, was));
                self.keys_focus_set(true);
            }
        } else if let Some((at, was)) = self.keys.focus_down.take() {
            // The same 350 ms window as a double-click separates a toggle from a momentary hold.
            self.keys_focus_set(if now - at < 0.35 { !was } else { was });
        }
    }

    /// Window blur cancels a held N and restores the mode it found.
    pub(super) fn keys_focus_cancel(&mut self) {
        if let Some((_, was)) = self.keys.focus_down.take() {
            self.keys_focus_set(was);
        }
    }

    /// Focus Mode edits only the foreground; entering it drops every other clip's selection.
    pub(super) fn keys_focus_set(&mut self, on: bool) {
        if self.keys.focus == on {
            return;
        }
        if on
            && let Some(fg) = self.keys.clip
            && let Some(all) = Composite::new(self.engine.session(), fg, &self.keys.others)
        {
            self.keys.selection = resolve(&all.notes, &self.keys.selection)
                .into_iter()
                .filter(|i| all.locate(*i).is_some_and(|(part, _)| part.clip == fg))
                .map(|i| all.notes[i])
                .collect();
        }
        self.keys.focus = on;
        self.keys.gesture = None;
    }

    /// Make `clip`, one of the open clips, the foreground: the roll's ticks become its ticks, so
    /// the selection, the view and the last click move by the difference (#329).
    pub(super) fn keys_foreground(&mut self, clip: ClipId) {
        let Some(fg) = self.keys.clip.filter(|fg| *fg != clip) else { return };
        let Some(by) =
            Composite::new(self.engine.session(), fg, &self.keys.others).and_then(|all| all.parts.iter().find(|p| p.clip == clip).map(|p| p.off))
        else {
            return;
        };
        for n in &mut self.keys.selection {
            n.start -= by;
        }
        self.keys.scroll -= by as f64;
        self.keys.click_tick = self.keys.click_tick.map(|t| t - by);
        if self.keys.step.on {
            self.status = "Step entry off: another clip came to the front".into();
        }
        self.keys.step = StepState::default();
        self.keys.cc.cancel();
        self.keys.length_edit = None;
        if let Some(slot) = self.keys.others.iter_mut().find(|c| **c == clip) {
            *slot = fg;
        }
        self.keys.clip = Some(clip);
    }

    /// Add new notes as one undo step, each to the clip [`Composite::target`] gives (with one clip
    /// open, always that clip); returns them as stored (the new selection). Outside every open clip
    /// the whole add is refused.
    pub fn keys_add(&mut self, notes: &[Note]) -> Result<Vec<Note>, String> {
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let mut to = Vec::new();
        for n in notes {
            match all.target(n.start) {
                Some(part) => to.push((*n, part.clone())),
                None => {
                    self.status = "Outside the open clips: draw inside one of them".into();
                    return Err(self.status.clone());
                }
            }
        }
        self.keys_add_to(to)
    }

    /// Add copies as one undo step, each to the clip its source note is in.
    pub(super) fn keys_add_copies(&mut self, pairs: &[(Note, Note)]) -> Result<Vec<Note>, String> {
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let sources: Vec<Note> = pairs.iter().map(|(a, _)| *a).collect();
        let to = pairs.iter().zip(all.owners(&sources)).filter_map(|((_, b), part)| part.map(|p| (*b, p.clone()))).collect();
        self.keys_add_to(to)
    }

    fn keys_add_to(&mut self, notes: Vec<(Note, Part)>) -> Result<Vec<Note>, String> {
        self.keys_refuse_frozen(&notes.iter().map(|(_, p)| p.clip).collect::<Vec<_>>())?;
        let key = self.keys_merge_key();
        let mut out = Vec::new();
        for (n, part) in notes {
            let stored = Note { release_velocity: 64, ..n };
            self.keys_exec(
                "midi.note_add",
                json!({"clip": part.clip.0, "pitch": n.pitch, "start_ticks": n.start - part.off, "length_ticks": n.length, "velocity": n.velocity, "channel": n.channel}),
                &key,
            )?;
            out.push(stored);
        }
        Ok(out)
    }
}
