//! Cutting a take and joining its pieces again (#25, ADR 0039).
//!
//! Choosing a shorter take inside a take cuts the take at the shorter one's edges ([`cut`]). The
//! cut is the ordinary split ([`crate::edit::split_clip`]), so a MIDI note that crosses it stays
//! only in the left piece (and is cut short there) and a fade that crosses it is clamped to its
//! piece. The pieces remember the whole take's fade lengths ([`transit_model::WholeFades`]).
//!
//! When the take is chosen back, its pieces end up next to each other on one playlist, and
//! [`rejoin`] joins them into one clip again: notes that crossed a cut and clamped fades come back
//! whole, so the take renders as it did before it was cut. A run of clips is joined only when it
//! is exactly what splitting the joined clip would give (checked by splitting it again), so
//! nothing that was edited after the cut is joined or changed.

use crate::edit::{TimeCtx, split_clip};
use transit_model::{Clip, ClipContent, ClipId, Session, TrackId, WholeFades};
use transit_time::Samples;

/// The fade lengths of the whole take `c` is a piece of: its kept whole lengths where its own
/// fade still spans the piece (a fade edited shorter since is the piece's own), else its fades.
fn whole(c: &Clip) -> (Samples, Samples) {
    let w = c.whole_fades.unwrap_or_default();
    let fade_in = if w.fade_in > c.fade_in.len && c.fade_in.len == c.length { w.fade_in } else { c.fade_in.len };
    let fade_out = if w.fade_out > c.fade_out.len && c.fade_out.len == c.length { w.fade_out } else { c.fade_out.len };
    (fade_in, fade_out)
}

/// Keep on `c` the whole fade lengths it was clamped from (none when nothing was clamped).
fn keep_whole(c: &mut Clip, fade_in: Samples, fade_out: Samples) {
    let w = WholeFades { fade_in: if fade_in > c.fade_in.len { fade_in } else { 0 }, fade_out: if fade_out > c.fade_out.len { fade_out } else { 0 } };
    c.whole_fades = (w != WholeFades::default()).then_some(w);
}

/// Split every clip on the track's active playlist that spans `at`, as
/// [`crate::edit::separate_at`] does, keeping the whole take's fade lengths on the two pieces.
pub(super) fn cut(s: &mut Session, track: TrackId, at: Samples) {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let n = s.track(track).map_or(0, |t| t.clips().iter().filter(|c| at > c.start && at < c.end()).count());
    if n == 0 {
        return;
    }
    let mut ids: Vec<ClipId> = (0..n).map(|_| s.new_clip_id()).collect();
    let Some(pl) = s.track_mut(track).and_then(transit_model::Track::playlist_mut) else { return };
    let mut out = Vec::with_capacity(pl.clips.len() + n);
    for c in pl.clips.drain(..) {
        if at > c.start
            && at < c.end()
            && let Some(id) = ids.pop()
            && let Some((mut l, mut r)) = split_clip(&c, at, id, tc)
        {
            let (fade_in, fade_out) = whole(&c);
            keep_whole(&mut l, fade_in, 0);
            keep_whole(&mut r, 0, fade_out);
            out.push(l);
            out.push(r);
            continue;
        }
        out.push(c);
    }
    pl.clips = out;
    pl.sort();
}

/// Whether `a` is `b` but for the kept whole fade lengths.
fn same_piece(a: &Clip, b: &Clip) -> bool {
    let mut x = a.clone();
    x.whole_fades = b.whole_fades;
    x == *b
}

/// The clip `pieces` (touching end to end, in time order) were split from, if splitting it at
/// their edges gives exactly them.
fn join(pieces: &[&Clip], tc: TimeCtx<'_>) -> Option<Clip> {
    let (first, last) = (*pieces.first()?, *pieces.last()?);
    let mut j = first.clone();
    j.length = last.end() - first.start;
    // An unlooped MIDI piece holds only the notes that start in it; a looped one keeps them all.
    if first.looping.is_none()
        && let ClipContent::Midi { sequence } = &mut j.content
    {
        let base = tc.tempo.samples_to_ticks(first.start, tc.sr);
        for p in &pieces[1..] {
            let ClipContent::Midi { sequence: q } = &p.content else { return None };
            let shift = tc.tempo.samples_to_ticks(p.start, tc.sr) - base;
            sequence.notes.extend(q.notes.iter().cloned().map(|mut n| {
                n.start += shift;
                n
            }));
            sequence.ctrls.extend(q.ctrls.iter().cloned().map(|mut c| {
                c.tick += shift;
                c
            }));
        }
    }
    if j.sync_point == 0
        && let Some(p) = pieces[1..].iter().find(|p| p.sync_point != 0)
    {
        j.sync_point = p.sync_point + (p.start - first.start);
    }
    let (fade_in, _) = whole(first);
    let (_, fade_out) = whole(last);
    j.fade_out = last.fade_out;
    j.fade_in.len = fade_in;
    j.fade_out.len = fade_out;
    j.clamp_fades();
    keep_whole(&mut j, fade_in, fade_out);
    // Splitting the joined clip again must give the pieces back.
    let mut rest = j.clone();
    for w in pieces.windows(2) {
        let (l, r) = split_clip(&rest, w[1].start, w[1].id, tc)?;
        if !same_piece(&l, w[0]) {
            return None;
        }
        rest = r;
    }
    same_piece(&rest, last).then_some(j)
}

/// The run of clips on `clips` of take `take` touching end to end through `at`, in time order.
fn run(clips: &[Clip], at: Samples) -> Vec<ClipId> {
    let Some(right) = clips.iter().find(|c| c.start == at && c.take.is_some()) else { return Vec::new() };
    let take = right.take;
    let Some(left) = clips.iter().find(|c| c.end() == at && c.start < at && c.take == take) else { return Vec::new() };
    let mut ids = vec![left.id, right.id];
    let mut start = left.start;
    while let Some(c) = clips.iter().find(|c| c.end() == start && c.start < start && c.take == take) {
        ids.insert(0, c.id);
        start = c.start;
    }
    let mut end = right.end();
    while let Some(c) = clips.iter().find(|c| c.start == end && c.take == take) {
        ids.push(c.id);
        end = c.end();
    }
    ids
}

/// Join the pieces of one take that meet at `at` on playlist `playlist` back into one clip, when
/// they are exactly the pieces a cut made (see the module docs). The joined clip keeps the first
/// piece's id. Returns whether it joined.
pub(super) fn rejoin(s: &mut Session, track: TrackId, playlist: usize, at: Samples) -> bool {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(p) = s.track_mut(track).and_then(|t| t.playlists.get_mut(playlist)) else { return false };
    let ids = run(&p.clips, at);
    if ids.len() < 2 {
        return false;
    }
    let pick = |ids: &[ClipId]| -> Option<Clip> {
        let pieces: Vec<&Clip> = ids.iter().filter_map(|id| p.clip(*id)).collect();
        join(&pieces, tc)
    };
    // The whole run first; failing that (a neighbor edited since), the two pieces at `at`.
    let at_idx = ids.iter().position(|id| p.clip(*id).is_some_and(|c| c.start == at)).unwrap_or(1);
    let pair = [ids[at_idx - 1], ids[at_idx]];
    let (joined, used) = match pick(&ids) {
        Some(j) => (j, ids.clone()),
        None if ids.len() > 2 => match pick(&pair) {
            Some(j) => (j, pair.to_vec()),
            None => return false,
        },
        None => return false,
    };
    p.clips.retain(|c| !used.contains(&c.id));
    p.clips.push(joined);
    p.sort();
    true
}

#[cfg(test)]
#[path = "takes_join_tests.rs"]
mod tests;
