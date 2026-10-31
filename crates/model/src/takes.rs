//! Take stacks (#25, ADR 0039): the takes a track holds over one stretch of time.
//!
//! A track's playlists are where its clips are stored; the active playlist is what the mix plays.
//! Recording a take over material (or a lap of cycle recording) leaves the earlier material on
//! another playlist. A **take stack** is a set of clips, across the track's playlists, that overlap
//! one another in time (transitively) and hold at least two different takes on at least two
//! playlists. A **take** in a stack is identified by its number ([`crate::Clip::take`]): every clip
//! with that number, wherever it is stored. A clip without a number belongs to its playlist's
//! untagged take, which gets the lowest number no tagged take uses (playlists in order).
//!
//! The **active take** of a stack is explicit in the data: it is the take whose clips are on the
//! active playlist, which is all the mix renders. Making another take active moves its clips onto
//! the active playlist and the material they replace onto the playlist they came from, so storage
//! changes and identity (the number) does not. A stack can play more than one take where takes
//! differ in length (the newest lap over its range, an older take around it): Live's main lane and
//! Logic's comp both do the same.
//!
//! A clip on another playlist that is an exact copy of one on the active playlist (what Duplicate
//! Playlist makes) is not a take: it is left out, so a duplicated playlist does not stack its
//! whole length.
//!
//! Conventions (Logic's Take Folder, Ableton's Take Lanes since Live 11): takes are lanes under
//! the track, one per take; clicking a take makes it the one that plays; Flatten keeps what plays
//! and removes the lanes.

use crate::{Clip, ClipId, Track};
use transit_time::{Range, Samples};

/// One take inside a [`TakeStack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackTake {
    /// The take's number, shown as "Take n" (1 for the first).
    pub number: u32,
    /// Its clips in this stack, as (playlist index, clip id), in time order.
    pub clips: Vec<(usize, ClipId)>,
    /// Samples of this take on the active playlist (0: it does not play here).
    pub active_len: Samples,
    /// Whether every clip of it carries `number` (false: the number is derived).
    pub explicit: bool,
}

impl StackTake {
    /// Whether any of this take plays (has clips on the active playlist).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active_len > 0
    }
}

/// Clips across a track's playlists that overlap in time and hold two or more takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeStack {
    /// From the earliest clip start to the latest clip end.
    pub range: Range,
    /// The takes, by number.
    pub takes: Vec<StackTake>,
}

impl TakeStack {
    /// The take that plays most of the stack (None if nothing in it is on the active playlist).
    #[must_use]
    pub fn active(&self) -> Option<u32> {
        self.takes.iter().filter(|t| t.is_active()).max_by_key(|t| (t.active_len, t.number)).map(|t| t.number)
    }

    #[must_use]
    pub fn take(&self, number: u32) -> Option<&StackTake> {
        self.takes.iter().find(|t| t.number == number)
    }

    /// The take a clip of this stack belongs to.
    #[must_use]
    pub fn take_of(&self, clip: ClipId) -> Option<&StackTake> {
        self.takes.iter().find(|t| t.clips.iter().any(|(_, c)| *c == clip))
    }

    #[must_use]
    pub fn contains(&self, clip: ClipId) -> bool {
        self.take_of(clip).is_some()
    }

    /// Whether every take number is written on its clips.
    #[must_use]
    pub fn explicit(&self) -> bool {
        self.takes.iter().all(|t| t.explicit)
    }
}

/// Whether `a` and `b` are the same material: equal in everything but id, take number and the
/// whole-take fade lengths. Compares field by field (cheap fields first) and clones nothing; the
/// destructuring names every field, so a new one cannot be forgotten here.
#[must_use]
pub fn same_material(a: &Clip, b: &Clip) -> bool {
    let Clip {
        id: _,
        take: _,
        whole_fades: _,
        name,
        start,
        length,
        content,
        scale_mode,
        gain_db,
        gain_env,
        fade_in,
        fade_out,
        muted,
        edit_locked,
        time_locked,
        rating,
        sync_point,
        color,
        stretch,
        group,
        loop_id,
        looping,
    } = a;
    *start == b.start
        && *length == b.length
        && *muted == b.muted
        && *edit_locked == b.edit_locked
        && *time_locked == b.time_locked
        && *rating == b.rating
        && *sync_point == b.sync_point
        && *color == b.color
        && *group == b.group
        && *loop_id == b.loop_id
        && *gain_db == b.gain_db
        && *stretch == b.stretch
        && *fade_in == b.fade_in
        && *fade_out == b.fade_out
        && *scale_mode == b.scale_mode
        && *looping == b.looping
        && *name == b.name
        && *gain_env == b.gain_env
        && *content == b.content
}

/// Who a clip's take is, before numbering: its number, or its playlist's untagged take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Tagged(u32),
    Untagged(usize),
}

impl Track {
    /// The track's take stacks in time order (see the module docs). Empty for a track whose
    /// playlists hold no overlapping takes.
    #[must_use]
    pub fn take_stacks(&self) -> Vec<TakeStack> {
        self.take_stacks_except(&[])
    }

    /// As [`Track::take_stacks`], leaving out the clips in `skip` (a recording numbers its new
    /// takes after the ones already there).
    #[must_use]
    pub fn take_stacks_except(&self, skip: &[ClipId]) -> Vec<TakeStack> {
        let active = self.active_playlist;
        let act = self.clips();
        let sorted = act.windows(2).all(|w| w[0].start <= w[1].start);
        let mut entries: Vec<(usize, &Clip)> = Vec::new();
        for (i, p) in self.playlists.iter().enumerate() {
            for c in &p.clips {
                if skip.contains(&c.id) {
                    continue;
                }
                // On a sorted active playlist only the clips starting where `c` does can be its
                // copy, so a frame does not compare every pair.
                let copy = || {
                    if sorted {
                        let from = act.partition_point(|a| a.start < c.start);
                        act[from..].iter().take_while(|a| a.start == c.start).any(|a| same_material(a, c))
                    } else {
                        act.iter().any(|a| same_material(a, c))
                    }
                };
                if i != active && copy() {
                    continue;
                }
                entries.push((i, c));
            }
        }
        entries.sort_by_key(|(i, c)| (c.start, *i, c.id.0));
        let mut out = Vec::new();
        let mut group: Vec<(usize, &Clip)> = Vec::new();
        let mut end = Samples::MIN;
        for (i, e) in entries.iter().enumerate() {
            let e = *e;
            // Clips that only touch the group at its end still join it when one of them continues
            // a take of the group (the pieces a take is cut into when a shorter take is chosen
            // inside it).
            let continues = || {
                entries
                    .iter()
                    .skip(i)
                    .take_while(|x| x.1.start == end)
                    .any(|x| x.1.take.is_some() && group.iter().any(|(_, c)| c.take == x.1.take && c.end() == end))
            };
            if !group.is_empty() && (e.1.start > end || (e.1.start == end && !continues())) {
                out.extend(stack_of(&group, active));
                group.clear();
            }
            end = if group.is_empty() { e.1.end() } else { end.max(e.1.end()) };
            group.push(e);
        }
        out.extend(stack_of(&group, active));
        out
    }

    /// The take numbers of all its stacks, newest (highest) first: the take lanes it shows.
    #[must_use]
    pub fn take_numbers(&self) -> Vec<u32> {
        let mut n: Vec<u32> = self.take_stacks().iter().flat_map(|s| s.takes.iter().map(|t| t.number)).collect();
        n.sort_unstable_by(|a, b| b.cmp(a));
        n.dedup();
        n
    }

    /// The stack holding `clip`, with its index.
    #[must_use]
    pub fn take_stack_of(&self, clip: ClipId) -> Option<(usize, TakeStack)> {
        self.take_stacks().into_iter().enumerate().find(|(_, s)| s.contains(clip))
    }
}

/// The stack a group of overlapping clips makes, if it holds two takes on two playlists.
fn stack_of(group: &[(usize, &Clip)], active: usize) -> Option<TakeStack> {
    let key = |(i, c): &(usize, &Clip)| c.take.map_or(Key::Untagged(*i), Key::Tagged);
    let mut keys: Vec<Key> = group.iter().map(key).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut playlists: Vec<usize> = group.iter().map(|(i, _)| *i).collect();
    playlists.sort_unstable();
    playlists.dedup();
    if keys.len() < 2 || playlists.len() < 2 {
        return None;
    }
    let used: Vec<u32> = keys.iter().filter_map(|k| if let Key::Tagged(n) = k { Some(*n) } else { None }).collect();
    let mut next = 1u32;
    let mut numbered: Vec<(Key, u32)> = Vec::new();
    for k in &keys {
        match k {
            Key::Tagged(n) => numbered.push((*k, *n)),
            Key::Untagged(_) => {
                while used.contains(&next) || numbered.iter().any(|(_, n)| *n == next) {
                    next = next.saturating_add(1);
                }
                numbered.push((*k, next));
            }
        }
    }
    let mut takes: Vec<StackTake> = numbered
        .iter()
        .map(|(k, number)| {
            let mine: Vec<&(usize, &Clip)> = group.iter().filter(|e| key(e) == *k).collect();
            StackTake {
                number: *number,
                clips: mine.iter().map(|(i, c)| (*i, c.id)).collect(),
                active_len: mine.iter().filter(|(i, _)| *i == active).map(|(_, c)| c.length).sum(),
                explicit: matches!(k, Key::Tagged(_)),
            }
        })
        .collect();
    takes.sort_by_key(|t| t.number);
    let start = group.iter().map(|(_, c)| c.start).min().unwrap_or(0);
    let end = group.iter().map(|(_, c)| c.end()).max().unwrap_or(start);
    Some(TakeStack { range: Range::new(start, end), takes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelFormat, Playlist, SourceId, TrackId, TrackKind};

    fn clip(id: u64, start: Samples, len: Samples, take: Option<u32>) -> Clip {
        let mut c = Clip::audio(ClipId(id), format!("c{id}"), SourceId(1), id as i64 * 1000, start, len);
        c.take = take;
        c
    }

    fn track(playlists: Vec<Vec<Clip>>, active: usize) -> Track {
        let mut t = Track::new(TrackId(1), "Vox", TrackKind::Audio, ChannelFormat::Mono, [0, 0, 0]);
        t.playlists = playlists.into_iter().enumerate().map(|(i, clips)| Playlist { name: format!("p{i}"), clips }).collect();
        t.active_playlist = active;
        t
    }

    #[test]
    fn three_laps_on_three_playlists_are_one_stack_of_three_takes_and_the_active_playlist_plays() {
        let t = track(vec![vec![clip(1, 100, 50, Some(3))], vec![clip(2, 100, 50, Some(1))], vec![clip(3, 100, 50, Some(2))]], 0);
        let st = t.take_stacks();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].range, Range::new(100, 150));
        assert_eq!(st[0].takes.iter().map(|t| t.number).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(st[0].active(), Some(3));
        assert!(st[0].explicit());
        assert_eq!(t.take_numbers(), vec![3, 2, 1], "newest lane first");
    }

    #[test]
    fn untagged_playlists_take_the_lowest_unused_numbers_in_playlist_order() {
        let t = track(vec![vec![clip(1, 0, 50, None)], vec![clip(2, 0, 50, Some(1))], vec![clip(3, 10, 50, None)]], 2);
        let st = t.take_stacks();
        assert_eq!(st.len(), 1);
        let n: Vec<(u32, ClipId, bool)> = st[0].takes.iter().map(|t| (t.number, t.clips[0].1, t.explicit)).collect();
        assert_eq!(n, vec![(1, ClipId(2), true), (2, ClipId(1), false), (3, ClipId(3), false)]);
        assert_eq!(st[0].active(), Some(3));
    }

    #[test]
    fn a_duplicated_playlist_is_not_a_stack_and_separate_regions_are_separate_stacks() {
        let a = clip(1, 0, 50, None);
        let mut copy = a.clone();
        copy.id = ClipId(9);
        let t = track(vec![vec![copy, clip(5, 500, 50, Some(1))], vec![a, clip(6, 500, 40, Some(2)), clip(7, 900, 10, None)]], 1);
        let st = t.take_stacks();
        assert_eq!(st.len(), 1, "the copy is not a take; the clip at 900 is alone");
        assert_eq!(st[0].range, Range::new(500, 550));
        assert_eq!(t.take_stack_of(ClipId(6)).map(|(i, _)| i), Some(0));
        assert!(t.take_stack_of(ClipId(1)).is_none());
    }

    #[test]
    fn one_playlist_alone_is_never_a_stack_even_with_two_numbers() {
        let t = track(vec![vec![clip(1, 0, 50, Some(1)), clip(2, 50, 50, Some(2))]], 0);
        assert!(t.take_stacks().is_empty());
        // Touching clips do not overlap.
        let t = track(vec![vec![clip(1, 0, 50, Some(1))], vec![clip(2, 50, 50, Some(2))]], 0);
        assert!(t.take_stacks().is_empty());
    }

    #[test]
    fn the_pieces_of_a_take_around_a_shorter_one_stay_in_its_stack() {
        // Take 2 chosen inside take 1: take 1 is cut into three pieces, the middle one moved
        // away. Its outer pieces touch the rest only end to end, and still belong to the stack.
        let t = track(
            vec![
                vec![clip(1, 0, 40, Some(1)), clip(2, 40, 20, Some(2)), clip(3, 60, 40, Some(1))],
                vec![clip(4, 40, 20, Some(1))],
                vec![clip(5, 0, 30, None)],
            ],
            0,
        );
        let st = t.take_stacks();
        assert_eq!(st.len(), 1, "{st:?}");
        assert_eq!(st[0].range, Range::new(0, 100));
        assert_eq!(st[0].take(1).map(|t| (t.clips.len(), t.active_len)), Some((3, 80)));
        // Touching clips of different takes stay apart.
        let t = track(vec![vec![clip(1, 0, 50, Some(1)), clip(2, 50, 50, Some(2))], vec![clip(3, 50, 50, Some(3))]], 0);
        assert_eq!(t.take_stacks()[0].range, Range::new(50, 100));
    }

    #[test]
    fn a_comp_plays_two_takes_and_the_longer_one_is_the_stacks_active_take() {
        // The newest lap over its range, the old take's pieces around it (Live's main lane).
        let t = track(vec![vec![clip(1, 0, 30, Some(1)), clip(2, 30, 20, Some(2)), clip(3, 50, 50, Some(1))], vec![clip(4, 0, 100, Some(1))]], 0);
        let st = t.take_stacks();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].take(1).map(|t| t.active_len), Some(80));
        assert_eq!(st[0].take(2).map(|t| t.active_len), Some(20));
        assert_eq!(st[0].active(), Some(1));
    }
}
