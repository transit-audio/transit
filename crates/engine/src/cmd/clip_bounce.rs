//! Bounce in Place of the selected clips (Logic's Bounce Regions in Place) and the `range` of
//! `track.bounce`. The printing itself is `io::bounce_track_range`.

use super::*;
use crate::cmd;
use crate::io::BounceSource;
use serde_json::json;
use transit_model::{TrackId, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.bounce",
        "Bounce in Place...",
        ["Clip"],
        None,
        "{clips?, destination?: new|track id, source?: leave|mute|mute_deactivate|hide} — defaults new, mute; prints each track's span of the clips",
        has_selection,
        bounce
    )]
}

/// Seconds of tail a bounce prints after the last clip (a whole-track bounce, a clip bounce, and the
/// furthest start a `range` may have past the material).
pub(crate) const BOUNCE_TAIL_SECONDS: f64 = 2.0;

/// The `source` param shared by `track.bounce` and `clip.bounce`.
pub(super) fn source_param(p: &Value) -> Result<BounceSource> {
    match p.get("source") {
        None => Ok(BounceSource::Mute),
        Some(Value::String(s)) if s == "mute" => Ok(BounceSource::Mute),
        Some(Value::String(s)) if s == "leave" => Ok(BounceSource::Leave),
        Some(Value::String(s)) if s == "mute_deactivate" => Ok(BounceSource::MuteDeactivate),
        Some(Value::String(s)) if s == "hide" => Ok(BounceSource::Hide),
        Some(_) => Err(bad("track.bounce", "source must be `leave`, `mute`, `mute_deactivate`, or `hide`")),
    }
}

/// `range: {start, length}` in session samples (or any position form); absent means the whole track.
pub(super) fn range_param(e: &Engine, p: &Value) -> Result<Option<Range>> {
    let Some(r) = p.get("range") else { return Ok(None) };
    let id = "track.bounce";
    if !r.is_object() {
        return Err(bad(id, "range must be {start, length}"));
    }
    let start = position_param(e, id, r, "start")?.ok_or_else(|| bad(id, "range needs `start`"))?;
    let len = position_param(e, id, r, "length")?.ok_or_else(|| bad(id, "range needs `length`"))?;
    if start < 0 || len <= 0 {
        return Err(bad(id, "range must start at or after zero and have a positive length"));
    }
    // Checked when set: a range may start in the tail after the material (a script prints only the
    // tail), up to and including the end of the tail; further out it prints silence and places a
    // clip near the end of the integer range that overflows a later bounce.
    let end = e.session().content_end();
    let limit = end.saturating_add(e.session().sample_rate.samples(BOUNCE_TAIL_SECONDS));
    if start > limit {
        return Err(bad(
            id,
            format!(
                "range starts past the end of the bounce tail (sample {limit}: the end of the session's material, sample {end}, plus {BOUNCE_TAIL_SECONDS} seconds)"
            ),
        ));
    }
    Ok(Some(Range::new(start, start.saturating_add(len))))
}

fn bounce(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "clip.bounce";
    let clips = clip_ids_param(e, p);
    if clips.is_empty() {
        return Err(bad(id, "no clips: pass `clips` or select clips"));
    }
    let destination = match p.get("destination") {
        None => None,
        Some(Value::String(s)) if s == "new" => None,
        Some(v) => Some(TrackId(v.as_u64().ok_or_else(|| bad("track.bounce", "destination must be `new` or a track id"))?)),
    };
    let source = source_param(p)?;
    // Each source track with the span from its earliest selected clip to its latest one, plus the tail.
    let s = e.session();
    let mut spans: Vec<(TrackId, transit_time::Samples, transit_time::Samples)> = Vec::new();
    for c in &clips {
        let (t, clip) = s.find_clip(*c).ok_or_else(|| bad(id, format!("unknown clip {}", c.0)))?;
        let tr = s.track(t).ok_or_else(|| bad(id, "the clip's track does not exist"))?;
        if !tr.kind.is_audio_path() || tr.kind == TrackKind::Master {
            return Err(bad(id, format!("{} is a MIDI clip: Bounce in Place prints audio tracks", clip.name)));
        }
        match spans.iter_mut().find(|x| x.0 == t) {
            Some(x) => {
                x.1 = x.1.min(clip.start);
                x.2 = x.2.max(clip.end());
            }
            None => spans.push((t, clip.start, clip.end())),
        }
    }
    spans.sort_by_key(|x| s.track_index(x.0));
    let tracks: Vec<TrackId> = spans.iter().map(|x| x.0).collect();
    if destination.is_some() && tracks.len() > 1 {
        return Err(bad("track.bounce", "a destination track needs clips on one track; use `new` for several"));
    }
    crate::io::validate_bounce_destination(s, &tracks, destination)?;
    let tail = s.sample_rate.samples(BOUNCE_TAIL_SECONDS);
    // Refused before anything renders, so a refusal leaves every track as it was.
    for (t, _, end) in &spans {
        if let Some(tr) = s.track(*t) {
            super::external_instrument::refuse_offline_print(id, tr)?;
            if tr.inactive {
                return Err(bad("track.bounce", crate::io::inactive_source_refusal(&tr.name)));
            }
            // The print starts at bar 1 (ADR 0052 decision 10): clips wholly in the pre-count
            // zone, tail included, would print nothing that sounds.
            if end.saturating_add(tail) <= 0 {
                return Err(bad(id, crate::io::zone_print_refusal(&tr.name)));
            }
        }
    }
    // A span that starts before bar 1 prints from bar 1 with that part hidden in the file (ADR 0052
    // decision 10); when the source is also silenced, the part before bar 1 stops playing, and the
    // reply says so instead of leaving it silent without a word.
    let silenced_zone = !matches!(source, BounceSource::Leave) && spans.iter().any(|(_, start, _)| *start < 0);
    let mut out = Vec::new();
    for (t, start, end) in spans {
        let span = Range::new(start, end.saturating_add(tail));
        if let Some(nt) = crate::io::bounce_track_range(e, t, destination, source, Some(span))? {
            out.push(nt);
        }
    }
    let mut reply = json!({"tracks": out, "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()});
    if silenced_zone && !out.is_empty() {
        reply["notice"] = json!(ZONE_SILENCED_NOTICE);
    }
    Ok(reply)
}

/// The notice `clip.bounce` returns when the print hides audio before bar 1 and the source is muted,
/// deactivated or hidden: that part no longer plays.
pub(crate) const ZONE_SILENCED_NOTICE: &str =
    "The part before bar 1 is kept in the printed file but no longer plays; choose Leave as the source to keep it sounding.";

/// The notice `track.bounce` and `track.commit` return when a printed track has content before
/// bar 1 and its source is muted, deactivated or hidden: the print starts at bar 1, so that part
/// no longer plays (#700).
pub(crate) const ZONE_LEFT_OUT_NOTICE: &str =
    "The part before bar 1 is not in the print, which starts at bar 1, and no longer plays; choose Leave as the source to keep it sounding.";
