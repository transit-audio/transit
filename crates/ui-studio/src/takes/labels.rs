//! Paint-only take labels (#317). No recording timestamp or label is stored in the session.

use transit_model::{Clip, TakeStack, Track};
use transit_time::{Range, Samples};

pub(crate) struct LaneLabel {
    pub text: String,
    pub hover: String,
}

/// Union the active-playlist pieces within each stack, keeping different stacks distinct.
/// `position` supplies the transport's bars/beats readout; headers use its bar and beat only.
pub(crate) fn lane_label(t: &Track, stacks: &[TakeStack], number: u32, position: impl Fn(Samples) -> String) -> LaneLabel {
    let mut played = Vec::new();
    for stack in stacks {
        let Some(take) = stack.take(number) else { continue };
        let mut pieces: Vec<Range> = take
            .clips
            .iter()
            .filter(|(pl, _)| *pl == t.active_playlist)
            .filter_map(|(pl, id)| t.playlists.get(*pl)?.clip(*id))
            .filter_map(|c| c.range().intersect(&stack.range))
            .collect();
        pieces.sort_by_key(|r| (r.start, r.end));
        let mut union: Vec<Range> = Vec::new();
        for piece in pieces {
            if let Some(last) = union.last_mut()
                && piece.start <= last.end
            {
                last.end = last.end.max(piece.end);
            } else {
                union.push(piece);
            }
        }
        played.extend(union.into_iter().map(|r| (stack.range, r)));
    }
    played.sort_by_key(|(_, r)| (r.start, r.end));
    let at = |sample| position(sample).split('.').take(2).map(str::trim).collect::<Vec<_>>().join(".");
    let name = format!("Take {number}");
    let text = match played.as_slice() {
        [] => name.clone(),
        [(stack, r)] if r == stack => format!("{name} · playing"),
        [(stack, r)] if r.start == stack.start => format!("{name} · playing to {}", at(r.end)),
        [(stack, r)] if r.end == stack.end => format!("{name} · playing from {}", at(r.start)),
        [(_, r)] => format!("{name} · playing {}–{}", at(r.start), at(r.end)),
        ranges => format!("{name} · playing ({} ranges)", ranges.len()),
    };
    let mut hover = if played.is_empty() { format!("{name} is not playing") } else { format!("{name} plays:") };
    for (_, r) in played {
        hover.push_str(&format!("\n{}–{}", at(r.start), at(r.end)));
    }
    hover.push_str(&format!("\nClick to play take {number} wherever it was recorded"));
    LaneLabel { text, hover }
}

/// Membership, including derived numbers in old sessions, determines the painted suffix.
pub(crate) fn clip_label(c: &Clip, stacks: &[TakeStack]) -> String {
    match stacks.iter().find_map(|s| s.take_of(c.id)) {
        Some(take) => format!("{} · take {}", c.name, take.number),
        None => c.name.clone(),
    }
}
