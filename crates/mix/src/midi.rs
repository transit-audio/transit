use super::*;

/// The track's MIDI Track Offset (`midi.offset.<id>` in the edit values), in samples. Looked up
/// without formatting the key, since this runs on the audio thread every block.
pub(crate) fn midi_offset(s: &Session, t: &Track) -> i64 {
    const PREFIX: &str = "midi.offset.";
    s.edit
        .values
        .range::<str, _>((std::ops::Bound::Included(PREFIX), std::ops::Bound::Unbounded))
        .take_while(|(k, _)| k.starts_with(PREFIX))
        .find(|(k, _)| k.get(PREFIX.len()..).and_then(|id| id.parse::<u64>().ok()) == Some(t.id.0))
        .map(|(_, v)| *v)
        .filter(|v| v.is_finite())
        .map_or(0, |v| v as i64)
}

/// Send note on/offs and controllers for MIDI clips overlapping the block.
/// Plan mode routes them through the instrument's note and pedal table as [`Source::CLIP`].
/// With `internal` false (the track sends to an external port) only note-offs reach the instrument.
///
/// A member of a rack (ADR 0034) also plays the rack's clips, merged in time with its own: a
/// note-on the member's zones exclude is dropped and one inside a fade range has its velocity
/// scaled ([`transit_model::RackMember::velocity`]); offs and controllers pass, and an off reaches
/// the instrument only when its on did (the rack's notes are a source of their own in the table).
pub(crate) fn schedule_notes(s: &Session, t: &Track, pos: Samples, frames: usize, render_frame: u64, inst: &mut PluginSlot, internal: bool) {
    let rack = s.rack_of(t);
    sorted_events(s, t, rack.map(|(r, _)| r), pos, frames, |at, from_rack, event| {
        let event = match (from_rack, event, rack) {
            (true, ClipEvent::On(key, velocity, ch), Some((_, member))) => match member.velocity(key, velocity) {
                Some(v) => ClipEvent::On(key, v, ch),
                None => return,
            },
            (_, e, _) => e,
        };
        if internal || matches!(event, ClipEvent::Off(..)) {
            deliver_clip_event(inst, render_frame, at, event, from_rack);
        }
    });
}

/// The clip events of the block in delivery order: by sample offset, and at one offset in
/// [`visit_clip_events`] traversal order (offs before ons). Allocation-free.
pub(crate) fn sorted_clip_events(s: &Session, t: &Track, pos: Samples, frames: usize, mut deliver: impl FnMut(usize, ClipEvent)) {
    sorted_events(s, t, None, pos, frames, |at, _, event| deliver(at, event));
}

/// [`sorted_clip_events`] of `t` merged with those of `rack` (flagged `true`): at one offset, the
/// track's own events first.
fn sorted_events(s: &Session, t: &Track, rack: Option<&Track>, pos: Samples, frames: usize, mut deliver: impl FnMut(usize, bool, ClipEvent)) {
    // Sort before touching the identity/holder tables: sorting only in the plug-in is too late.
    // Fixed scratch bounds memory, not event count. If a range overflows, split its time span
    // and rescan; a single sample can stream directly in the visitor's off-before-on order.
    // Usually the entire block fits and needs just one visit and one allocation-free sort.
    let mut scratch = [(0usize, 0usize, false, ClipEvent::Off(0, 0)); 256];
    let (mut from, mut until) = (0, frames);
    while from < frames {
        let mut used = 0usize;
        let mut overflow = false;
        let single_sample = until - from == 1;
        for (track, from_rack) in [(Some(t), false), (rack, true)] {
            let Some(track) = track else { continue };
            visit_clip_events(s, track, pos, frames, |at, event| {
                if at < from || at >= until {
                    return;
                }
                if single_sample {
                    deliver(at, from_rack, event);
                } else if let Some(slot) = scratch.get_mut(used) {
                    *slot = (at, used, from_rack, event);
                    used += 1;
                } else {
                    overflow = true;
                }
            });
        }
        if overflow {
            until = from + (until - from) / 2;
            continue;
        }
        if let Some(events) = scratch.get_mut(..used) {
            // The traversal index preserves off-before-on at ties, and on-then-off for a
            // note whose duration rounds to zero samples. No stable-sort allocation.
            events.sort_unstable_by_key(|&(at, order, _, _)| (at, order));
            for &(at, _, from_rack, event) in events.iter() {
                deliver(at, from_rack, event);
            }
        }
        from = until;
        until = frames;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ClipEvent {
    /// Pitch, velocity, recorded channel (zero-based).
    On(u8, u8, u8),
    /// Pitch, recorded channel.
    Off(u8, u8),
    /// A channel-1 message and the recorded channel; the instrument path ignores the channel.
    Control([u8; 3], u8),
}

fn deliver_clip_event(inst: &mut PluginSlot, render_frame: u64, at: usize, event: ClipEvent, rack: bool) {
    if let Some(e) = inst.events.as_deref_mut() {
        // Only earlier live events: at a tie, chase and clips precede live events. Applying
        // a future clip pedal first could otherwise absorb an earlier live pedal release.
        e.deliver(&mut *inst.plugin, render_frame, at);
        let offset = u32::try_from(at).unwrap_or(u32::MAX);
        match event {
            ClipEvent::On(key, velocity, _) => e.clip_on(&mut *inst.plugin, offset, key, velocity, rack),
            ClipEvent::Off(key, _) => e.clip_off(&mut *inst.plugin, offset, key, rack),
            ClipEvent::Control(msg, _) => e.clip_ctrl(&mut *inst.plugin, offset, msg, rack),
        }
    } else {
        match event {
            ClipEvent::On(key, velocity, _) => inst.plugin.note_on(at, key, velocity),
            ClipEvent::Off(key, _) => inst.plugin.note_off(at, key),
            ClipEvent::Control(msg, _) => inst.plugin.midi(at, msg),
        }
    }
}

/// MIDI Real-Time Properties of a track (non-destructive velocity/transpose/duration/delay).
#[derive(Clone, Copy)]
pub(crate) struct Rtp {
    velocity: i64,
    transpose: i64,
    duration: f64,
    delay: i64,
}

impl Rtp {
    pub(crate) fn of(s: &Session, t: &Track) -> Self {
        if s.edit.values.is_empty() {
            return Self { velocity: 0, transpose: 0, duration: 1.0, delay: 0 };
        }
        let rtp = |name: &str, d: f64| Key::new(format_args!("rtp.{}.{name}", t.id.0)).map_or(d, |k| s.edit.value(k.as_str(), d));
        Self {
            velocity: rtp("velocity", 0.0).clamp(-127.0, 127.0) as i64,
            transpose: rtp("transpose", 0.0).clamp(-127.0, 127.0) as i64,
            duration: rtp("duration", 100.0).clamp(1.0, 1000.0) / 100.0,
            delay: rtp("delay", 0.0).clamp(-1e7, 1e7) as i64,
        }
    }
}

/// A clip note as it plays: real-time properties applied, its on and off in session samples (the
/// off clamped to the clip end). `None` when it starts at or after the clip end.
pub(crate) fn played_note(
    s: &Session,
    clip: &Clip,
    base: i64,
    rtp: Rtp,
    n0: &transit_midi::Note,
    keep_negative: bool,
) -> Option<(transit_midi::Note, Samples, Samples)> {
    let sr = s.sample_rate;
    let mut n = *n0;
    n.velocity = (i64::from(n.velocity) + rtp.velocity).clamp(1, 127) as u8;
    n.pitch = (i64::from(n.pitch) + rtp.transpose).clamp(0, 127) as u8;
    n.length = ((n.length as f64 * rtp.duration) as i64).max(1);
    // A looped clip keeps notes a start trim moved before its start (negative ticks).
    n.start = n.start.saturating_add(rtp.delay);
    if !keep_negative {
        n.start = n.start.max(0);
    }
    let on = s.tempo.tick_to_samples(base.saturating_add(n.start), sr);
    let off = s.tempo.tick_to_samples(base.saturating_add(n.start).saturating_add(n.length), sr).min(clip.end());
    (on < clip.end()).then_some((n, on, off))
}

/// Every span `(on, off)` a note sounds in a clip, as playback and the external chase both need it,
/// for the cycles that touch `pos..end` (the first pass is always visited). `on`/`off` are the
/// note's first-pass times from [`played_note`]; each span is cut at the clip end.
///
/// A looped clip (ADR 0019): the first pass plays what starts before the loop end; later cycles
/// play what starts inside the range. A note crossing the seam ends at it and sounds again at its
/// own position in the next cycle. One implementation, so playback and the chase cannot drift.
pub(crate) fn note_spans(
    clip: &Clip,
    looping: Option<(Samples, ClipLoop)>,
    on: Samples,
    off: Samples,
    pos: Samples,
    end: Samples,
    mut f: impl FnMut(Samples, Samples),
) {
    let mut emit = |on: Samples, off: Samples| {
        let off = off.min(clip.end());
        if on < clip.end() {
            f(on, off);
        }
    };
    let Some((origin, l)) = looping else {
        emit(on, off);
        return;
    };
    let (rel_on, seam) = (on.saturating_sub(origin), origin.saturating_add(l.end));
    if rel_on >= l.end {
        return;
    }
    // A note the start trim cut off the front (negative position) only sounds in later cycles.
    if rel_on >= 0 {
        emit(on, off.min(seam));
    }
    if rel_on >= l.start {
        let span = off.min(seam).saturating_sub(on).max(0);
        for k in cycles_touching(origin, l, rel_on, span, pos, end) {
            let at = cycle_at(origin, l, rel_on, k);
            emit(at, at.saturating_add(span));
        }
    }
}

/// Visit candidates without mutating delivery state. At each offset all nonzero-length offs
/// precede ons; a note whose on and off round to the same sample keeps on-then-off.
pub(crate) fn visit_clip_events(s: &Session, t: &Track, pos: Samples, frames: usize, mut visit: impl FnMut(usize, ClipEvent)) {
    let sr = s.sample_rate;
    // MIDI Track Offsets: the track's notes sound `offset` samples later (earlier if negative), so
    // the block looks that far back into the clips.
    let offset = midi_offset(s, t);
    let pos = pos.saturating_sub(offset);
    let end = pos.saturating_add(frames as i64);
    // Two passes, so at any one sample offset every note-off precedes every note-on: a clip that
    // ends where the next begins (or a repeated pitch) must not have its new note killed by the
    // old one's release. Allocation-free; a note whose on and off share a sample keeps on-then-off.
    for ons in [false, true] {
        for clip in t.clips() {
            if clip.muted {
                continue;
            }
            let ClipContent::Midi { sequence } = &clip.content else { continue };
            // A note-off is clamped to `clip.end()`, which the half-open range excludes: when that
            // sample is the first of this block (`pos == clip.end()`) the clip still has to be visited
            // or the off is never sent and the note holds forever.
            if clip.end() < pos || clip.start.min(grid_start(s, clip)) >= end {
                continue;
            }
            let base = s.tempo.samples_to_ticks(clip.start, sr);
            let rtp = Rtp::of(s, t);
            let (dp, delay) = (rtp.transpose, rtp.delay);
            let looping = midi_loop_frame(s, clip);
            for n0 in &sequence.notes {
                let Some((n, on, off)) = played_note(s, clip, base, rtp, n0, looping.is_some()) else { continue };
                note_spans(clip, looping, on, off, pos, end, |on, off| {
                    if ons && on >= pos && on < end {
                        let at = usize::try_from(on - pos).unwrap_or(0);
                        visit(at, ClipEvent::On(n.pitch, n.velocity, n.channel));
                    }
                    if off >= pos && off < end && (ons == (off == on)) {
                        let at = usize::try_from(off - pos).unwrap_or(0);
                        visit(at, ClipEvent::Off(n.pitch, n.channel));
                    }
                });
            }
            if !ons {
                // A pedal the clip leaves held is released at its end, in the off pass with the
                // note-offs, so a pedal-down at the same sample (the next clip's) follows it.
                let t_end = clip.end();
                if t_end >= pos && t_end < end {
                    let mine = pedal_state_before(s, clip, rtp, t_end);
                    for (slot, held) in mine.iter().enumerate() {
                        let Some((at, true)) = *held else { continue };
                        // The track's pedal belongs to whichever clip wrote it last: only that
                        // clip's end releases it. Otherwise a later write (an up, or a down some
                        // overlapping clip will release itself) already decided the state.
                        let superseded = t.clips().iter().any(|other| {
                            let other_midi = matches!(other.content, ClipContent::Midi { .. });
                            // A clip whose tick grid starts at or after this end (its events sit on the grid, up to half a
                            // tick before `start`), or that ended before the press, cannot
                            // have written the pedal after it: not scanned.
                            !std::ptr::eq(other, clip) && !other.muted && other_midi && grid_start(s, other) < t_end && other.end() > at && {
                                let theirs = pedal_state_before(s, other, rtp, t_end.min(other.end()));
                                matches!(theirs.get(slot), Some(Some((a, _))) if *a > at)
                            }
                        });
                        if !superseded {
                            let (cc, channel) = pedal_slot_cc(slot);
                            visit(usize::try_from(t_end - pos).unwrap_or(0), ClipEvent::Control([0xB0, cc, 0], channel));
                        }
                    }
                }
                continue;
            }
            let near = s.tempo.samples_to_ticks(pos, sr).saturating_sub(base).saturating_sub(1)
                ..=s.tempo.samples_to_ticks(end, sr).saturating_sub(base).saturating_add(1);
            // Events before tick 0 are what a start trim hid: they don't play, except in a looped
            // clip, where later cycles do.
            for c in sequence.ctrls.iter().filter(|c| c.tick >= 0 || looping.is_some()) {
                let tick = c.tick.saturating_add(delay);
                let tick = if looping.is_some() { tick } else { tick.max(0) };
                if let Some((origin, l)) = looping {
                    let at = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
                    let rel = at.saturating_sub(origin);
                    if rel >= l.end {
                        continue;
                    }
                    let msg = ctrl_message(c.kind, c.value, dp);
                    if rel >= 0 && at >= pos && at < end && at < clip.end() {
                        visit(usize::try_from(at - pos).unwrap_or(0), ClipEvent::Control(msg, c.channel));
                    }
                    if rel >= l.start {
                        for k in cycles_touching(origin, l, rel, 0, pos, end) {
                            let at = cycle_at(origin, l, rel, k);
                            if at >= pos && at < end && at < clip.end() {
                                visit(usize::try_from(at - pos).unwrap_or(0), ClipEvent::Control(msg, c.channel));
                            }
                        }
                    }
                    continue;
                }
                if !near.contains(&tick) {
                    continue;
                }
                let at = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
                if at >= pos && at < end && at < clip.end() {
                    visit(usize::try_from(at - pos).unwrap_or(0), ClipEvent::Control(ctrl_message(c.kind, c.value, dp), c.channel));
                }
            }
        }
    }
}

// Test hook: how many clips the clip-end release scanned (the cost bound is a count, not a time).
#[cfg(test)]
thread_local! {
    pub(crate) static PEDAL_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The sample a clip's tick grid starts at: its events sit there, up to half a tick before `start`.
pub(crate) fn grid_start(s: &Session, clip: &Clip) -> Samples {
    let sr = s.sample_rate;
    s.tempo.tick_to_samples(s.tempo.samples_to_ticks(clip.start, sr), sr)
}

/// Pedal controllers the clip-end release covers: 16 channels times sustain, sostenuto, soft, hold 2.
const PEDAL_SLOTS: usize = 64;

fn pedal_slot_of(cc: u8, channel: u8) -> Option<usize> {
    let kind = match cc {
        CC_SUSTAIN => 0,
        CC_SOSTENUTO => 1,
        CC_SOFT => 2,
        CC_HOLD2 => 3,
        _ => return None,
    };
    (channel < 16).then(|| usize::from(channel) * 4 + kind)
}

/// The controller number and recorded channel of a pedal slot.
fn pedal_slot_cc(slot: usize) -> (u8, u8) {
    let cc = [CC_SUSTAIN, CC_SOSTENUTO, CC_SOFT, CC_HOLD2].get(slot % 4).copied().unwrap_or(CC_SUSTAIN);
    (cc, (slot / 4) as u8)
}

/// Per pedal slot, the clip's last pedal event strictly before `until` (a session sample): its time
/// and whether it holds (value >= 64). Looped cycles count, so a loop that ends on a held pedal is
/// seen at the clip end. Fixed-size and allocation-free: runs on the audio thread.
fn pedal_state_before(s: &Session, clip: &Clip, rtp: Rtp, until: Samples) -> [Option<(Samples, bool)>; PEDAL_SLOTS] {
    #[cfg(test)]
    PEDAL_SCANS.with(|n| n.set(n.get() + 1));
    let mut last = [None; PEDAL_SLOTS];
    let ClipContent::Midi { sequence } = &clip.content else { return last };
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(clip.start, sr);
    let looping = midi_loop_frame(s, clip);
    for c in sequence.ctrls.iter().filter(|c| c.tick >= 0 || looping.is_some()) {
        let CtrlKind::Cc(cc) = c.kind else { continue };
        let Some(slot) = pedal_slot_of(cc & 0x7F, c.channel) else { continue };
        let tick = c.tick.saturating_add(rtp.delay);
        let tick = if looping.is_some() { tick } else { tick.max(0) };
        let first = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
        // The latest time this event sounds before `until`.
        let mut latest: Option<Samples> = None;
        match looping {
            None => latest = (first < until).then_some(first),
            Some((origin, l)) => {
                let rel = first.saturating_sub(origin);
                if rel < l.end {
                    if rel >= 0 && first < until {
                        latest = Some(first);
                    }
                    let repeat = cycle_at(origin, l, rel, 1);
                    if rel >= l.start && repeat < until {
                        let k = until.saturating_sub(1).saturating_sub(repeat).div_euclid(l.cycle().max(1));
                        latest = latest.max(Some(repeat.saturating_add(k.saturating_mul(l.cycle().max(1)))));
                    }
                }
            }
        }
        if let (Some(at), Some(entry)) = (latest, last.get_mut(slot))
            && entry.is_none_or(|(prev, _)| at >= prev)
        {
            *entry = Some((at, c.value >= 64));
        }
    }
    last
}

/// The note-ons a track's clips play in `pos..pos + frames`, as (absolute sample, key): what the
/// audio thread would send. Support for tests of edits that must not change how a clip sounds.
pub fn clip_note_ons(s: &Session, track: TrackId, pos: Samples, frames: usize) -> Vec<(Samples, u8)> {
    let mut out = Vec::new();
    if let Some(t) = s.track(track) {
        visit_clip_events(s, t, pos, frames, |at, e| {
            if let ClipEvent::On(key, _, _) = e {
                out.push((pos + at as Samples, key));
            }
        });
    }
    out.sort();
    out
}

/// A looped MIDI clip written out as it sounds: every cycle's notes and controllers as plain
/// events, ticks from the clip start, cut at the clip end (ADR 0019). `None` for an unlooped or
/// non-MIDI clip, or when the result would be unreasonably large.
pub fn unroll_midi_loop(s: &Session, clip: &Clip) -> Option<transit_midi::Sequence> {
    let (origin, l) = midi_loop_frame(s, clip)?;
    let ClipContent::Midi { sequence } = &clip.content else { return None };
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(clip.start, sr);
    let end = clip.end();
    let cycles = clip.length / l.cycle().max(1) + 2;
    if cycles.saturating_mul((sequence.notes.len() + sequence.ctrls.len()) as i64) > 2_000_000 {
        return None;
    }
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr) - base;
    let mut out = transit_midi::Sequence::default();
    // Every sounding position of a content point `rel` samples in: the first pass, then each cycle.
    let places = |rel: Samples| {
        let first = (0..l.end).contains(&rel).then_some(origin + rel);
        let later = (rel >= l.start && rel < l.end).then(|| (1..).map(move |k| cycle_at(origin, l, rel, k)).take_while(|at| *at < end));
        first.into_iter().chain(later.into_iter().flatten()).filter(|at| *at >= origin).collect::<Vec<_>>()
    };
    for n in &sequence.notes {
        let on = s.tempo.tick_to_samples(base.saturating_add(n.start), sr);
        let off = s.tempo.tick_to_samples(base.saturating_add(n.start).saturating_add(n.length), sr);
        let rel = on - origin;
        let span = off.min(origin + l.end) - on;
        for at in places(rel) {
            let to = at.saturating_add(span.max(0)).min(end);
            let mut m = *n;
            m.start = tick(at);
            m.length = (tick(to) - m.start).max(1);
            out.notes.push(m);
        }
    }
    for c in &sequence.ctrls {
        let at = s.tempo.tick_to_samples(base.saturating_add(c.tick), sr);
        for at in places(at - origin) {
            let mut m = *c;
            m.tick = tick(at);
            out.ctrls.push(m);
        }
    }
    out.sort();
    Some(out)
}

/// Where cycle `k` (1 is the first repeat) of a content point `rel` samples into a looped clip
/// lands: the point must lie inside the loop range.
pub fn cycle_at(clip_start: Samples, l: ClipLoop, rel: Samples, k: i64) -> Samples {
    clip_start.saturating_add(l.end).saturating_add(rel - l.start).saturating_add((k - 1).saturating_mul(l.cycle()))
}

/// A looped MIDI clip's frame (ADR 0019): the sample its tick grid starts at and the loop range
/// measured from there. Notes sound on the tick grid (`tick_to_samples(base + tick)`), and `base`
/// is `clip.start` rounded to a tick, so a clip that starts between ticks has its grid origin up to
/// half a tick away from `clip.start`. Cycles stepped from `clip.start` with the cycle measured
/// there let every cycle slip by that gap against the grid; stepping from the grid origin with the
/// loop edges rounded to ticks keeps every cycle on the first cycle's grid offset. For a clip that
/// starts on a tick with a loop on ticks the frame is `(clip.start, loop)` unchanged.
pub(crate) fn midi_loop_frame(s: &Session, clip: &Clip) -> Option<(Samples, ClipLoop)> {
    let l = clip.looping.filter(|l| l.is_valid())?;
    Some(loop_frame(s, clip.start, l))
}

/// [`midi_loop_frame`] for a clip starting at `start` with loop `l`: playback, consolidate and the
/// arranger's drawing all take their cycles from this one function, so what is drawn is what is heard.
pub fn loop_frame(s: &Session, start: Samples, l: ClipLoop) -> (Samples, ClipLoop) {
    let sr = s.sample_rate;
    let origin = s.tempo.tick_to_samples(s.tempo.samples_to_ticks(start, sr), sr);
    let snap = |x: Samples| s.tempo.tick_to_samples(s.tempo.samples_to_ticks(start.saturating_add(x), sr), sr).saturating_sub(origin);
    let snapped = ClipLoop { start: snap(l.start), end: snap(l.end) };
    // A range that rounds to under the minimum cycle keeps the sample frame.
    if snapped.is_valid() { (origin, snapped) } else { (start, l) }
}

/// The repeat cycles (`k >= 1`) in which a point `rel` samples into a looped clip, sounding for
/// `span` samples, touches the block `pos..end`. A handful at most; capped against a degenerate loop.
fn cycles_touching(clip_start: Samples, l: ClipLoop, rel: Samples, span: Samples, pos: Samples, end: Samples) -> std::ops::RangeInclusive<i64> {
    let cycle = l.cycle().max(1);
    let first = cycle_at(clip_start, l, rel, 1);
    // The last cycle that starts before `end`, and the first still sounding at `pos`.
    let last = end.saturating_sub(1).saturating_sub(first).div_euclid(cycle) + 1;
    let ahead = first.saturating_add(span).saturating_sub(pos);
    let k_min = (1 - ahead.div_euclid(cycle)).max(1);
    k_min..=last.min(k_min.saturating_add(4096))
}

/// A controller event as a MIDI 1.0 message on channel 1 (where the notes go), with `transpose`
/// applied to a poly-pressure key.
fn ctrl_message(kind: CtrlKind, value: i32, transpose: i64) -> [u8; 3] {
    let v7 = value.clamp(0, 127) as u8;
    match kind {
        CtrlKind::Cc(n) => [0xB0, n & 0x7F, v7],
        CtrlKind::PitchBend => {
            let v = value.saturating_add(8192).clamp(0, 16383);
            [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
        }
        CtrlKind::ChannelPressure => [0xD0, v7, 0],
        CtrlKind::PolyPressure(k) => [0xA0, (i64::from(k) + transpose).clamp(0, 127) as u8, v7],
        CtrlKind::Program => [0xC0, v7, 0],
    }
}

/// Before an instrument plays from `pos`: reset its controllers, then send each one the last value
/// it had on this track before `pos`.
pub(crate) fn chase_controllers(s: &Session, t: &Track, pos: Samples, inst: &mut PluginSlot) {
    clip_midi(inst, 0, [0xB0, 121, 0]);
    let pos = pos.saturating_sub(midi_offset(s, t));
    const BEND: usize = 128;
    const PRESSURE: usize = 129;
    const PROGRAM: usize = 130;
    // The last write per controller: time, value, and whether the clip that wrote it ended at or
    // before `pos`. A pedal such a clip leaves held was released at its end, so it is restored up.
    let mut last = [(i64::MIN, 0i32, false); 131];
    let sr = s.sample_rate;
    for clip in t.clips() {
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        if clip.muted || clip.start.min(grid_start(s, clip)) >= pos {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        let until = pos.min(clip.end());
        let looping = midi_loop_frame(s, clip);
        for c in sequence.ctrls.iter().filter(|c| c.tick >= 0 || looping.is_some()) {
            let slot = match c.kind {
                CtrlKind::Cc(n) => usize::from(n & 0x7F),
                CtrlKind::PitchBend => BEND,
                CtrlKind::ChannelPressure => PRESSURE,
                CtrlKind::Program => PROGRAM,
                CtrlKind::PolyPressure(_) => continue,
            };
            let mut at = Some(s.tempo.tick_to_samples(base.saturating_add(c.tick), sr));
            if let (Some((origin, l)), Some(first_pass)) = (looping, at) {
                // The latest cycle that sounds before `until` (ADR 0019); a controller a start trim
                // moved before the clip start only sounds in the cycles.
                let rel = first_pass.saturating_sub(origin);
                if rel >= l.end {
                    continue;
                }
                if rel < 0 {
                    at = None;
                }
                if rel >= l.start {
                    let first = cycle_at(origin, l, rel, 1);
                    if until > first {
                        at = Some(cycle_at(origin, l, rel, (until - 1 - first).div_euclid(l.cycle().max(1)) + 1));
                    }
                }
            }
            if let Some(at) = at
                && at < until
                && let Some(l) = last.get_mut(slot)
                && at >= l.0
            {
                *l = (at, c.value, clip.end() <= pos);
            }
        }
    }
    for (slot, &(at, value, ended)) in last.iter().enumerate() {
        let value = if ended && value >= 64 && pedal_slot_of(slot as u8, 0).is_some() { 0 } else { value };
        let kind = match slot {
            BEND => CtrlKind::PitchBend,
            PRESSURE => CtrlKind::ChannelPressure,
            PROGRAM => CtrlKind::Program,
            n => CtrlKind::Cc(u8::try_from(n).unwrap_or(0)),
        };
        if at != i64::MIN {
            clip_midi(inst, 0, ctrl_message(kind, value, 0));
        }
    }
}

/// A clip controller: in plan mode through the instrument's pedal table (as [`Source::CLIP`],
/// keeping live sources' pedals), else straight to the plug-in.
fn clip_midi(inst: &mut PluginSlot, offset: usize, msg: [u8; 3]) {
    match inst.events.as_deref_mut() {
        Some(e) => e.clip_ctrl(&mut *inst.plugin, u32::try_from(offset).unwrap_or(u32::MAX), msg, false),
        None => inst.plugin.midi(offset, msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{PlanRig, render_block};
    use transit_model::Insert;

    struct Ears {
        inner: Box<dyn Plugin>,
        heard: Arc<std::sync::Mutex<Vec<(usize, [u8; 3])>>>,
    }

    impl Plugin for Ears {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
        fn midi(&mut self, offset: usize, msg: [u8; 3]) {
            self.heard.lock().unwrap().push((offset, msg));
        }
    }

    #[test]
    fn controllers_play_at_their_offsets_and_are_chased_after_a_seek() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears"));
        let cc = |tick, kind, value| transit_midi::CtrlEvent { tick, channel: 0, kind, value };
        let ctrls = vec![cc(4, CtrlKind::Cc(64), 127), cc(960, CtrlKind::PitchBend, -8192), cc(1920, CtrlKind::Cc(64), 0)];
        let seq = transit_midi::Sequence { ctrls, ..Default::default() };
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000 * 4, seq));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears { inner: transit_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, ears);
        rig.publish(&s);
        render_block(&mut rig, 0);
        assert_eq!(std::mem::take(&mut *heard.lock().unwrap()), vec![(0, [0xB0, 121, 0]), (100, [0xB0, 64, 127])]);
        render_block(&mut rig, 36_000);
        assert_eq!(
            std::mem::take(&mut *heard.lock().unwrap()),
            vec![(0, [0xB0, 64, 0]), (0, [0xB0, 121, 0]), (0, [0xB0, 64, 127]), (0, [0xE0, 0, 0])]
        );
        render_block(&mut rig, 36_512);
        assert!(heard.lock().unwrap().is_empty());
    }

    /// Chase on a seek into a looped clip that starts between ticks measures cycles from the tick
    /// grid origin, like playback: a controller whose next cycle lands a few samples after the seek
    /// is not yet in effect (the clip start is 7 samples into tick 0; 25 samples per tick).
    #[test]
    fn controller_chase_in_a_looped_clip_that_starts_between_ticks_measures_from_the_grid_origin() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears"));
        let q = transit_time::TICKS_PER_QUARTER;
        let bar = 4 * s.tempo.tick_to_samples(q, s.sample_rate);
        let cc = |tick, value| transit_midi::CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value };
        // Value 10 at beat 1, value 20 two ticks (50 samples) later.
        let seq = transit_midi::Sequence { ctrls: vec![cc(q, 10), cc(q + 2, 20)], ..Default::default() };
        let cid = s.new_clip_id();
        let mut c = Clip::midi(cid, "m", 7, 5 * bar - 7, seq);
        c.looping = Some(ClipLoop { start: 0, end: bar - 7 });
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears { inner: transit_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, ears);
        rig.publish(&s);
        render_block(&mut rig, 0);
        heard.lock().unwrap().clear();
        // The value-10 event's second sounding is at bar + 24000; seek 4 samples before it.
        let seek = bar + 24_000 - 4;
        render_block(&mut rig, seek);
        let got = std::mem::take(&mut *heard.lock().unwrap());
        assert!(got.contains(&(0, [0xB0, 1, 20])), "the latest value before the seek is 20: {got:?}");
        assert!(!got.contains(&(0, [0xB0, 1, 10])), "10 sounds 4 samples after the seek, not before: {got:?}");
    }

    struct Keys {
        inner: Box<dyn Plugin>,
        heard: Arc<std::sync::Mutex<Vec<(u8, u8)>>>,
    }

    impl Plugin for Keys {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
        fn note_on(&mut self, _offset: usize, note: u8, velocity: u8) {
            self.heard.lock().unwrap().push((note, velocity));
        }
    }

    #[test]
    fn real_time_properties_reach_the_instrument() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.keys"));
        let mut seq = transit_midi::Sequence::default();
        seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000, seq));
        s.edit.values.insert(format!("rtp.{}.transpose", t.0), 12.0);
        s.edit.values.insert(format!("rtp.{}.velocity", t.0), -20.0);
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let keys = Keys { inner: transit_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, keys);
        rig.publish(&s);
        render_block(&mut rig, 0);
        assert_eq!(*heard.lock().unwrap(), vec![(72, 80)]);
    }

    struct Ears2 {
        inner: Box<dyn Plugin>,
        heard: Arc<std::sync::Mutex<Vec<(&'static str, usize, u8)>>>,
    }

    impl Plugin for Ears2 {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
        fn note_on(&mut self, offset: usize, note: u8, _velocity: u8) {
            self.heard.lock().unwrap().push(("on", offset, note));
        }
        fn note_off(&mut self, offset: usize, note: u8) {
            self.heard.lock().unwrap().push(("off", offset, note));
        }
    }

    #[test]
    fn a_note_off_on_the_clip_end_block_boundary_is_sent() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears2"));
        let mut seq = transit_midi::Sequence::default();
        // Far longer than the clip, so the off is clamped to the clip end (sample 1024).
        seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 1024, seq));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears2 { inner: transit_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, ears);
        rig.publish(&s);
        for pos in [0, 512, 1024] {
            render_block(&mut rig, pos);
        }
        assert_eq!(*heard.lock().unwrap(), vec![("on", 0, 60), ("off", 0, 60)]);
        // Past the clip nothing more is sent.
        render_block(&mut rig, 1536);
        assert_eq!(heard.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_note_off_at_a_clip_boundary_precedes_the_next_clips_note_on() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears2"));
        let note = || {
            let mut seq = transit_midi::Sequence::default();
            seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
            seq
        };
        // The later clip is first in the playlist, so playlist order alone would send its on first.
        let (late, early) = (s.new_clip_id(), s.new_clip_id());
        let clips = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips;
        clips.push(Clip::midi(late, "b", 24_000, 24_000, note()));
        clips.push(Clip::midi(early, "a", 0, 24_000, note()));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears2 { inner: transit_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut rig = PlanRig::new(512).fake(t, INSTRUMENT_SLOT, ears);
        rig.publish(&s);
        // One beat at 120 bpm / 48 kHz is 24 000 samples, so the boundary sits on a tick and lands
        // at offset 256 of the block starting 256 samples before it.
        for pos in (0..23_552).step_by(512) {
            render_block(&mut rig, pos);
        }
        rig.eng.render_active(23_552, 192, &mut vec![vec![0.0; 512]; 2]);
        heard.lock().unwrap().clear();
        render_block(&mut rig, 23_744);
        assert_eq!(*heard.lock().unwrap(), vec![("off", 256, 60), ("on", 256, 60)]);
    }
}
