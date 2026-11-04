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
    // Refused before anything renders, so a refusal leaves every track as it was.
    for t in &tracks {
        if let Some(tr) = s.track(*t) {
            super::external_instrument::refuse_offline_print(id, tr)?;
            if tr.inactive {
                return Err(bad("track.bounce", crate::io::inactive_source_refusal(&tr.name)));
            }
        }
    }
    let tail = s.sample_rate.samples(2.0);
    let mut out = Vec::new();
    for (t, start, end) in spans {
        let span = Range::new(start, end + tail);
        if let Some(nt) = crate::io::bounce_track_range(e, t, destination, source, Some(span))? {
            out.push(nt);
        }
    }
    Ok(json!({"tracks": out, "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}))
}
