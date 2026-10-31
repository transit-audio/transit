//! Take stacks as edits (#25, ADR 0039): choose the take that plays, flatten, delete a take, and
//! number a recording's new takes. The model's [`transit_model::TakeStack`] says what a stack is;
//! these functions change the session directly, and the `takes.*` commands make each one undo step.
//!
//! The take that plays is the one on the active playlist (all the mix renders). Making a clip's
//! take play moves that clip onto the active playlist and the material it replaces, cut at the
//! clip's edges, onto the playlist the clip came from: no clip is copied or lost, and each keeps
//! its take number, so the lanes stay where they were.
//!
//! Conventions: Logic's Take Folder and Ableton's Take Lanes (Live 11) agree that clicking a take
//! makes it the one that plays and that Flatten keeps what plays and discards the other takes.

use transit_model::{ClipId, Session, TakeStack, TrackId};
use transit_time::{Range, Samples};

/// Why a take edit did nothing.
pub type Refusal = String;

fn stacks(s: &Session, track: TrackId) -> Result<Vec<TakeStack>, Refusal> {
    s.track(track).map(transit_model::Track::take_stacks).ok_or_else(|| format!("no track {}", track.0))
}

/// Write the numbers of `stack`'s takes onto its clips that have none, so they stay what they are
/// when clips move between playlists.
pub fn materialize(s: &mut Session, track: TrackId, stack: &TakeStack) {
    let Some(t) = s.track_mut(track) else { return };
    for take in stack.takes.iter().filter(|t| !t.explicit) {
        for (pl, id) in &take.clips {
            if let Some(c) = t.playlists.get_mut(*pl).and_then(|p| p.clip_mut(*id)) {
                c.take = Some(take.number);
            }
        }
    }
}

/// Make `clip`'s take play over the clip's range: the clip moves onto the active playlist and
/// what the active playlist held there moves to the clip's playlist. False when it already plays.
pub fn activate_clip(s: &mut Session, track: TrackId, clip: ClipId) -> Result<bool, Refusal> {
    let t = s.track(track).ok_or_else(|| format!("no track {}", track.0))?;
    let active = t.active_playlist;
    let from = t.playlists.iter().position(|p| p.clip(clip).is_some()).ok_or_else(|| format!("no clip {} on that track", clip.0))?;
    if from == active {
        return Ok(false);
    }
    let range = t.playlists.get(from).and_then(|p| p.clip(clip)).map(transit_model::Clip::range).unwrap_or_default();
    crate::edit::separate_at(s, track, range.start);
    crate::edit::separate_at(s, track, range.end);
    let t = s.track_mut(track).ok_or_else(|| format!("no track {}", track.0))?;
    let Some(act) = t.playlists.get_mut(active) else { return Err("the track has no active playlist".into()) };
    let mut moved = Vec::new();
    act.clips.retain(|c| {
        let inside = c.start >= range.start && c.end() <= range.end;
        if inside {
            moved.push(c.clone());
        }
        !inside
    });
    let Some(src) = t.playlists.get_mut(from) else { return Err("the take's playlist is gone".into()) };
    let Some(pos) = src.clips.iter().position(|c| c.id == clip) else { return Err("the take's clip is gone".into()) };
    let taken = src.clips.remove(pos);
    src.clips.extend(moved);
    src.sort();
    if let Some(act) = t.playlists.get_mut(active) {
        act.clips.push(taken);
        act.sort();
    }
    Ok(true)
}

/// Make take `number` play wherever it has material in stack `index` (the longest clip first; a
/// clip already covered by the take on the active playlist stays where it is). Returns the clips
/// moved onto the active playlist.
pub fn set_active(s: &mut Session, track: TrackId, index: usize, number: u32) -> Result<usize, Refusal> {
    let all = stacks(s, track)?;
    let stack = all.get(index).ok_or_else(|| format!("the track has {} take stack(s); there is no stack {index}", all.len()))?;
    let take = stack.take(number).ok_or_else(|| format!("stack {index} has no take {number}"))?;
    materialize(s, track, stack);
    let active = s.track(track).map_or(0, |t| t.active_playlist);
    let mut clips: Vec<(Samples, ClipId)> = take
        .clips
        .iter()
        .filter(|(pl, _)| *pl != active)
        .filter_map(|(pl, id)| s.track(track)?.playlists.get(*pl)?.clip(*id).map(|c| (c.length, c.id)))
        .collect();
    clips.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.0.cmp(&b.1.0)));
    let mut moved = 0;
    for (_, id) in clips {
        let Some(t) = s.track(track) else { break };
        let Some(range) = t.playlists.iter().find_map(|p| p.clip(id)).map(transit_model::Clip::range) else { continue };
        if covered(t.clips(), range, number) {
            continue;
        }
        if activate_clip(s, track, id)? {
            moved += 1;
        }
    }
    Ok(moved)
}

/// Whether the active clips numbered `number` cover all of `range`.
fn covered(active: &[transit_model::Clip], range: Range, number: u32) -> bool {
    let mut parts: Vec<Range> = active.iter().filter(|c| c.take == Some(number)).filter_map(|c| c.range().intersect(&range)).collect();
    parts.sort_by_key(|r| r.start);
    let mut at = range.start;
    for p in parts {
        if p.start > at {
            return false;
        }
        at = at.max(p.end);
    }
    at >= range.end
}

/// Remove the playlists (other than the active one) that held clips in `had` and hold none now,
/// keeping the active playlist's place.
fn drop_emptied(s: &mut Session, track: TrackId, had: &[bool]) {
    let Some(t) = s.track_mut(track) else { return };
    let active = t.active_playlist;
    let mut i = t.playlists.len();
    while i > 0 {
        i -= 1;
        if i != active && had.get(i).copied().unwrap_or(false) && t.playlists.get(i).is_some_and(|p| p.clips.is_empty()) {
            t.playlists.remove(i);
            if i < t.active_playlist {
                t.active_playlist -= 1;
            }
        }
    }
}

fn had_clips(s: &Session, track: TrackId) -> Vec<bool> {
    s.track(track).map(|t| t.playlists.iter().map(|p| !p.clips.is_empty()).collect()).unwrap_or_default()
}

/// The stacks `index` names: that one, or all when `None`.
fn chosen(all: Vec<TakeStack>, index: Option<usize>) -> Result<Vec<TakeStack>, Refusal> {
    if all.is_empty() {
        return Err("the track has no stacked takes".into());
    }
    match index {
        None => Ok(all),
        Some(i) => {
            let n = all.len();
            all.into_iter().nth(i).map(|s| vec![s]).ok_or_else(|| format!("the track has {n} take stack(s); there is no stack {i}"))
        }
    }
}

/// Flatten: keep what plays in stack `index` (all stacks when `None`) and remove the other takes'
/// clips; their playlists go when nothing else is on them, and what stays loses its take number.
/// Returns the clips removed.
pub fn flatten(s: &mut Session, track: TrackId, index: Option<usize>) -> Result<usize, Refusal> {
    let chosen = chosen(stacks(s, track)?, index)?;
    let had = had_clips(s, track);
    let Some(t) = s.track_mut(track) else { return Err(format!("no track {}", track.0)) };
    let active = t.active_playlist;
    let mut removed = 0;
    for st in &chosen {
        for take in &st.takes {
            for (pl, id) in &take.clips {
                let Some(p) = t.playlists.get_mut(*pl) else { continue };
                if *pl == active {
                    if let Some(c) = p.clip_mut(*id) {
                        c.take = None;
                    }
                } else if let Some(pos) = p.clips.iter().position(|c| c.id == *id) {
                    p.clips.remove(pos);
                    removed += 1;
                }
            }
        }
    }
    drop_emptied(s, track, &had);
    Ok(removed)
}

/// Delete take `number` from stack `index` (every stack when `None`). A take that plays in a
/// stack is refused there: choose another take first, or flatten. Returns the clips removed.
pub fn delete_take(s: &mut Session, track: TrackId, number: u32, index: Option<usize>) -> Result<usize, Refusal> {
    let chosen: Vec<TakeStack> = chosen(stacks(s, track)?, index)?.into_iter().filter(|st| st.take(number).is_some()).collect();
    if chosen.is_empty() {
        return Err(format!("there is no take {number}"));
    }
    if chosen.iter().any(|st| st.take(number).is_some_and(transit_model::StackTake::is_active)) {
        return Err(format!("take {number} is playing; choose another take there first, or flatten"));
    }
    for st in &chosen {
        materialize(s, track, st);
    }
    let had = had_clips(s, track);
    let Some(t) = s.track_mut(track) else { return Err(format!("no track {}", track.0)) };
    let mut removed = 0;
    for st in &chosen {
        for (pl, id) in st.take(number).map(|t| t.clips.as_slice()).unwrap_or_default() {
            if let Some(p) = t.playlists.get_mut(*pl)
                && let Some(pos) = p.clips.iter().position(|c| c.id == *id)
            {
                p.clips.remove(pos);
                removed += 1;
            }
        }
    }
    drop_emptied(s, track, &had);
    Ok(removed)
}

/// Before a take is placed over material: the playlist was just duplicated (`previous` is the one
/// that was active), and the active copy's clips under `range` are about to be replaced. Number
/// them (a stack keeps its numbers; material that was no take yet becomes the lowest free number,
/// 1 for a first take) and give the same number to their originals on `previous`, so the old
/// material is one take, wherever its pieces end up.
pub fn number_covered(s: &mut Session, track: TrackId, range: Range, previous: usize) {
    let Ok(all) = stacks(s, track) else { return };
    for st in all.iter().filter(|st| st.range.overlaps(&range)) {
        materialize(s, track, st);
    }
    let Some(t) = s.track_mut(track) else { return };
    let active = t.active_playlist;
    let used: Vec<u32> = t.playlists.iter().flat_map(|p| p.clips.iter()).filter(|c| c.range().overlaps(&range)).filter_map(|c| c.take).collect();
    let free = (1..).find(|n| !used.contains(n)).unwrap_or(1);
    let mut numbered = Vec::new();
    if let Some(p) = t.playlists.get_mut(active) {
        for c in p.clips.iter_mut().filter(|c| c.range().overlaps(&range)) {
            if c.take.is_none() {
                c.take = Some(free);
            }
            numbered.push(c.clone());
        }
    }
    if previous == active {
        return;
    }
    if let Some(p) = t.playlists.get_mut(previous) {
        for c in &numbered {
            if let Some(orig) = p.clips.iter_mut().find(|o| o.take.is_none() && transit_model::same_material(o, c)) {
                orig.take = c.take;
            }
        }
    }
}

/// After a recording placed `new` clips (in pass order) on `track`: the playlist the track played
/// before the passes (`base`) plays again, the new clips are numbered after the takes already
/// there, and each pass is made to play in turn, so the newest plays over its range and an
/// earlier, longer one around it (Live's main lane; Logic's newest take). A single new clip over
/// nothing gets no number: it is no take stack.
pub fn settle_new_takes(s: &mut Session, track: TrackId, base: usize, new: &[ClipId]) {
    let Some(t) = s.track(track) else { return };
    if base < t.playlists.len()
        && let Some(t) = s.track_mut(track)
    {
        t.active_playlist = base;
    }
    let Some(t) = s.track(track) else { return };
    let ranges: Vec<Range> = new.iter().filter_map(|id| t.playlists.iter().find_map(|p| p.clip(*id)).map(transit_model::Clip::range)).collect();
    let Some(span) = ranges.iter().copied().reduce(|a, b| Range::new(a.start.min(b.start), a.end.max(b.end))) else { return };
    let before = t.take_stacks_except(new);
    for st in before.iter().filter(|st| st.range.overlaps(&span)) {
        materialize(s, track, st);
    }
    let Some(t) = s.track_mut(track) else { return };
    let max = t
        .playlists
        .iter()
        .flat_map(|p| p.clips.iter())
        .filter(|c| !new.contains(&c.id) && c.range().overlaps(&span))
        .filter_map(|c| c.take)
        .max()
        .unwrap_or(0);
    if new.len() < 2 && max == 0 {
        return;
    }
    for (k, id) in new.iter().enumerate() {
        if let Some(c) = t.playlists.iter_mut().find_map(|p| p.clip_mut(*id)) {
            c.take = Some(max.saturating_add(u32::try_from(k).unwrap_or(u32::MAX)).saturating_add(1));
        }
    }
    for id in new {
        let _ = activate_clip(s, track, *id);
    }
}

#[cfg(test)]
#[path = "takes_tests.rs"]
mod tests;
