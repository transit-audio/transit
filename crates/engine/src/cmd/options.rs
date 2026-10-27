//! Options menu toggles and Setup › Session settings.

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::{BitDepthSetting, EditState};
use transit_time::FrameRate;

macro_rules! toggle {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $field:ident) => {
        cmd!(noundo $id, $label, [$($m),*], $sc, "{value?: bool}", always, |e, p| {
            let s = e.session_mut();
            s.edit.$field = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.$field);
            Ok(json!({"value": s.edit.$field}))
        })
    };
}

/// Cycle on/off. Turning it on with something selected makes the region from it, even when a
/// region exists (see [`selection_span`]): the span of the selected clips, else the time selection.
/// With nothing selected an existing region is reused, and with none the region is four bars from
/// the bar containing `at` (default: the playhead), cut at the end of the content when that lies
/// past `at`. A changed region and the switch land as one undo step; turning Cycle off leaves the
/// region, so the next toggle restores it.
fn loop_playback(e: &mut Engine, p: &Value) -> Result<Value> {
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(!e.session().edit.loop_playback);
    let at = position_param(e, "options.loop_playback", p, "at")?;
    let before = e.session_arc();
    let region = if !on {
        None
    } else if let Some(span) = selection_span(e.session()) {
        Some(span)
    } else if e.session().edit.loop_range.is_empty() {
        Some(default_region(e, at))
    } else {
        None
    };
    let region = region.filter(|r| !r.is_empty() && *r != e.session().edit.loop_range);
    let ed = &mut e.session_mut().edit;
    ed.loop_playback = on;
    if let Some(r) = region {
        ed.loop_range = r;
        e.push_undo("Cycle On", before);
    }
    Ok(json!({"value": on}))
}

/// The region a selection asks for: the span of the selected clips (earliest start to latest end),
/// else the time selection when it is not empty. `None` when nothing is selected.
pub(super) fn selection_span(s: &transit_model::Session) -> Option<transit_time::Range> {
    let (mut start, mut end) = (i64::MAX, i64::MIN);
    for id in &s.edit.selected_clips {
        if let Some((_, c)) = s.find_clip(*id) {
            start = start.min(c.start);
            end = end.max(c.end());
        }
    }
    if start < end {
        return Some(transit_time::Range::new(start, end));
    }
    (!s.edit.selection.is_empty()).then_some(s.edit.selection)
}

fn default_region(e: &Engine, at: Option<i64>) -> transit_time::Range {
    let s = e.session();
    let at = at.unwrap_or(if e.transport.playing { e.transport.position } else { s.edit.playhead }).max(0);
    four_bars(&s.tempo, s.sample_rate, at, s.content_end())
}

/// Four bars from the bar holding `at`. Cut at `content` (the end of the content) when that lies
/// ahead of the start, unless the cut would leave less than a beat: then the full four bars.
fn four_bars(tempo: &transit_time::TempoMap, sr: transit_time::SampleRate, at: i64, content: i64) -> transit_time::Range {
    let bar = tempo.bar_beat_at(at, sr).bar;
    let start_tick = tempo.bar_start_tick(bar);
    let start = tempo.tick_to_samples(start_tick, sr);
    let mut end = tempo.tick_to_samples(tempo.bar_start_tick(bar + 4), sr);
    let beat = tempo.tick_to_samples(start_tick + tempo.meter_at_tick(start_tick).ticks_per_beat(), sr) - start;
    if content > start && content < end && content - start >= beat {
        end = content;
    }
    transit_time::Range::new(start, end)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        toggle!("options.quickpunch", "QuickPunch", ["Options"], Some("Cmd+Shift+P"), quickpunch),
        toggle!("options.pre_post_roll", "Pre/Post-Roll", ["Options"], Some("Cmd+K"), pre_post_roll),
        cmd!(
            noundo
            "options.loop_playback",
            "Loop Playback",
            ["Options"],
            Some("Cmd+Shift+L"),
            "{value?: bool, at?: position (where the default region starts when none exists; the playhead when omitted)}",
            always,
            |e, p| loop_playback(e, p)
        ),
        toggle!("options.link_timeline_edit", "Link Timeline and Edit Selection", ["Options"], Some("Shift+/"), link_timeline_edit),
        toggle!("options.link_track_edit", "Link Track and Edit Selection", ["Options"], None, link_track_edit),
        toggle!("options.insertion_follows_playback", "Insertion Follows Playback", ["Options"], None, insertion_follows_playback),
        toggle!("options.tab_to_transient", "Tab to Transient", ["Options"], Some("Cmd+Alt+Tab"), tab_to_transient),
        toggle!("options.mirrored_midi", "Mirrored MIDI Editing", ["Options"], None, mirrored_midi),
        toggle!("options.automation_follows_edit", "Automation Follows Edit", ["Options"], None, automation_follows_edit),
        toggle!("options.markers_follow_edit", "Markers Follow Edit", ["Options"], None, markers_follow_edit),
        toggle!("options.layered_editing", "Layered Editing", ["Options"], None, layered_editing),
        toggle!("options.click", "Click", ["Options"], None, click),
        toggle!("options.countoff", "Count Off", [], None, countoff),
        toggle!("options.midi_merge", "MIDI Merge", [], None, midi_merge),
        toggle!("options.pre_fader_metering", "Pre-Fader Metering", ["Options"], None, pre_fader_metering),
        toggle!("options.delay_compensation", "Delay Compensation", ["Options"], None, delay_compensation),
        cmd!(noundo "options.destructive_record", "Destructive Record", ["Options"], None, "{value?: bool}", always, |e, p| record_mode(e, p, "destructive")),
        cmd!(noundo "options.trackpunch", "TrackPunch", ["Options"], Some("Cmd+Shift+T"), "{value?: bool}", always, |e, p| record_mode(e, p, "trackpunch")),
        cmd!(noundo "options.scrolling", "Edit Window Scrolling", ["Options", "Edit Window Scrolling"], None, "{mode: none|after_playback|page|continuous|center}", always, |e, p| {
            let m = str_param(p, "mode").filter(|m| ["none", "after_playback", "page", "continuous", "center"].contains(m)).ok_or_else(|| bad("options.scrolling", "unknown mode"))?.to_string();
            e.session_mut().edit.scrolling = m.clone();
            Ok(json!({"mode": m}))
        }),
        cmd!(noundo "options.solo_mode", "Solo Mode", ["Options", "Solo Mode"], None, "{mode: sip|afl|pfl, latch?: bool, xor?: bool}", always, |e, p| {
            let m = str_param(p, "mode").unwrap_or("sip").to_string();
            let mut v = m.clone();
            if bool_or(p, "xor", false) { v.push_str("+xor") }
            e.session_mut().edit.solo_mode = v.clone();
            Ok(json!({"mode": v}))
        }),
        cmd!(noundo "options.keyboard_focus", "Keyboard Focus", [], None, "{focus?: commands|clips|groups|none} — toggles Commands Keyboard Focus when omitted", always, |e, p| {
            let s = e.session_mut();
            let f = match str_param(p, "focus") {
                Some(f @ ("commands" | "clips" | "groups" | "none")) => f.to_string(),
                Some(other) => return Err(bad("options.keyboard_focus", format!("unknown focus `{other}`"))),
                None => if s.edit.keyboard_focus == "commands" { "none".into() } else { "commands".into() },
            };
            s.edit.keyboard_focus = f.clone();
            Ok(json!({"focus": f}))
        }),
        cmd!(noundo "options.pre_roll", "Pre-Roll Amount", [], None, "{length: samples|{seconds}}", always, |e, p| {
            let v = position_param(e, "options.pre_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.pre_roll = v;
            Ok(json!({"pre_roll": v}))
        }),
        cmd!(noundo "options.post_roll", "Post-Roll Amount", [], None, "{length}", always, |e, p| {
            let v = position_param(e, "options.post_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.post_roll = v;
            Ok(json!({"post_roll": v}))
        }),
        cmd!(
            "setup.session",
            "Session",
            ["Setup"],
            Some("Cmd+2"),
            "{frame_rate?: '29.97 Drop'…, bit_depth?: 16|24|32f, timecode_start?: position, name?}",
            always,
            session_setup
        ),
        cmd!(noundo "setup.preferences", "Preferences...", ["Setup"], None, "{}", always, |e, _| Ok(json!({"edit": e.session().edit.clone()}))),
        cmd!(noundo "setup.reset_edit_state", "Reset Edit Settings", [], None, "{}", always, |e, _| {
            let s = e.session_mut();
            let keep = (s.edit.selection, s.edit.selected_tracks.clone());
            s.edit = EditState::default();
            s.edit.selection = keep.0;
            s.edit.selected_tracks = keep.1;
            Ok(json!({}))
        }),
    ]
}

fn record_mode(e: &mut Engine, p: &Value, mode: &str) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(s.edit.record_mode != mode);
    s.edit.record_mode = if on { mode.to_string() } else { "normal".into() };
    Ok(json!({"record_mode": s.edit.record_mode}))
}

fn session_setup(e: &mut Engine, p: &Value) -> Result<Value> {
    let fr = match str_param(p, "frame_rate") {
        Some(f) => {
            Some(FrameRate::ALL.into_iter().find(|r| r.label().eq_ignore_ascii_case(f)).ok_or_else(|| bad("setup.session", "unknown frame rate"))?)
        }
        None => None,
    };
    let bd = match p.get("bit_depth").map(|v| v.to_string().trim_matches('"').to_ascii_lowercase()) {
        Some(b) if b == "16" => Some(BitDepthSetting::Int16),
        Some(b) if b == "24" => Some(BitDepthSetting::Int24),
        Some(b) if b.starts_with("32") => Some(BitDepthSetting::Float32),
        Some(_) => return Err(bad("setup.session", "bit_depth must be 16, 24 or 32f")),
        None => None,
    };
    let tc = position_param(e, "setup.session", p, "timecode_start")?;
    let s = e.session_mut();
    if let Some(f) = fr {
        s.frame_rate = f;
    }
    if let Some(b) = bd {
        s.bit_depth = b;
    }
    if let Some(t) = tc {
        s.timecode_start = t;
    }
    if let Some(n) = p.get("name").and_then(Value::as_str) {
        s.name = n.to_string();
    }
    Ok(json!({"frame_rate": s.frame_rate.label(), "bit_depth": s.bit_depth, "sample_rate": s.sample_rate.hz()}))
}

#[cfg(test)]
mod tests {
    use super::four_bars;
    use transit_time::Range;

    #[test]
    fn a_promoted_region_is_never_shorter_than_a_beat() {
        let s = transit_model::Session::default();
        let (t, sr) = (&s.tempo, s.sample_rate);
        let bar = |n: i64| t.tick_to_samples(t.bar_start_tick(n), sr);
        let at = bar(2) + 5;
        assert_eq!(four_bars(t, sr, at, bar(2) + 10), Range::new(bar(2), bar(6)), "content 10 samples past the bar start: full four bars");
        let two = bar(2) + (bar(3) - bar(2)) * 2;
        assert_eq!(four_bars(t, sr, at, two), Range::new(bar(2), two), "a longer cut still applies");
        assert_eq!(four_bars(t, sr, at, 0), Range::new(bar(2), bar(6)), "no content ahead: no cut");
    }
}
