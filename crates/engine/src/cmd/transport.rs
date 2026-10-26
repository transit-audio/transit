//! Transport commands. They update the session's stopped playhead and queue requests for the
//! host's audio engine (which may be absent when headless).

use super::*;
use crate::{TransportRequest, cmd};
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "transport.play", "Play", [], Some("Space"), "{from?: position}", always, |e, p| {
            if let Some(at) = position_param(e, "transport.play", p, "from")? {
                e.transport_requests.push(TransportRequest::Locate(at.max(0)));
            }
            e.transport_requests.push(TransportRequest::Play);
            Ok(json!({}))
        }),
        cmd!(noundo "transport.stop", "Stop", [], Some("Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::Stop); Ok(json!({})) }),
        cmd!(noundo "transport.toggle", "Play/Stop", [], Some("Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::TogglePlay); Ok(json!({})) }),
        cmd!(noundo "transport.record", "Record", [], Some("Cmd+Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::Record); Ok(json!({})) }),
        cmd!(noundo "transport.pause", "Pause", [], None, "{}", always, |e, _| { e.transport_requests.push(TransportRequest::Pause); Ok(json!({})) }),
        cmd!(noundo "transport.half_speed", "Half-Speed Playback", [], Some("Shift+Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::HalfSpeed); Ok(json!({})) }),
        cmd!(noundo "transport.play_selection", "Play Edit", ["Edit", "Selection"], Some("Alt+["), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::PlaySelection); Ok(json!({})) }),
        cmd!(noundo "transport.rtz", "Return to Zero", [], Some("Home"), "{}", always, |e, _| locate(e, 0)),
        cmd!(noundo "transport.go_to_end", "Go to End", [], Some("End"), "{}", always, |e, _| { let end = e.session().content_end(); locate(e, end) }),
        cmd!(noundo "transport.rewind", "Rewind", [], None, "{seconds?: 1}", always, |e, p| {
            let s = e.session();
            let d = s.sample_rate.samples(f64_or(p, "seconds", 1.0).clamp(0.0, 86_400.0));
            let at = (cur(e) - d).max(0);
            locate(e, at)
        }),
        cmd!(noundo "transport.fast_forward", "Fast Forward", [], None, "{seconds?: 1}", always, |e, p| {
            let s = e.session();
            let d = s.sample_rate.samples(f64_or(p, "seconds", 1.0).clamp(0.0, 86_400.0));
            let at = cur(e) + d;
            locate(e, at)
        }),
        cmd!(noundo "transport.locate", "Locate", [], None, "{at: position}", always, |e, p| {
            let at = position_param(e, "transport.locate", p, "at")?.ok_or_else(|| bad("transport.locate", "`at` required"))?;
            locate(e, at.max(0))
        }),
        cmd!(
            "transport.loop",
            "Set Loop Region",
            [],
            None,
            "{start?: position, end?: position (both: the region, an empty or reversed pair is ordered, an empty one clears it), enabled?: bool}",
            always,
            |e, p| {
                let st = position_param(e, "transport.loop", p, "start")?;
                let en = position_param(e, "transport.loop", p, "end")?;
                if st.is_some() != en.is_some() {
                    return Err(bad("transport.loop", "`start` and `end` go together"));
                }
                let enabled = p.get("enabled").and_then(Value::as_bool);
                let edit = &e.session().edit;
                let range = st
                    .zip(en)
                    .map(|(a, b)| soundcraft_time::Range::new(a.max(0), b.max(0)))
                    .map(|r| if r.is_empty() { soundcraft_time::Range::default() } else { r });
                // Touch the document only when something changes, so a no-op leaves no undo step.
                if range.is_some_and(|r| r != edit.loop_range) || enabled.is_some_and(|v| v != edit.loop_playback) {
                    let ed = &mut e.session_mut().edit;
                    if let Some(r) = range {
                        ed.loop_range = r;
                    }
                    if let Some(v) = enabled {
                        ed.loop_playback = v;
                    }
                }
                let ed = &e.session().edit;
                Ok(json!({"start": ed.loop_range.start, "end": ed.loop_range.end, "enabled": ed.loop_playback}))
            }
        ),
        cmd!(
            "transport.cycle_to_selection",
            "Set Cycle to Selection",
            [],
            None,
            "{} — the loop region becomes the span of the selected clips, else the time selection; Cycle stays as it is",
            always,
            |e, _| {
                let r = super::options::selection_span(e.session())
                    .ok_or_else(|| bad("transport.cycle_to_selection", "nothing is selected: select clips or a time range first"))?;
                // Touch the document only when the region changes, so a no-op leaves no undo step.
                if e.session().edit.loop_range != r {
                    e.session_mut().edit.loop_range = r;
                }
                Ok(json!({"start": r.start, "end": r.end}))
            }
        ),
        cmd!(noundo "transport.status", "Transport Status", [], None, "{}", always, |e, _| Ok(json!(e.transport))),
    ]
}

fn cur(e: &Engine) -> i64 {
    if e.transport.playing { e.transport.position } else { e.session().edit.selection.start }
}

fn locate(e: &mut Engine, at: i64) -> Result<Value> {
    let s = e.session_mut();
    s.edit.playhead = at;
    if s.edit.selection.is_empty() || !s.edit.selection.contains(at) {
        s.edit.selection = soundcraft_time::Range::point(at);
    }
    e.transport_requests.push(TransportRequest::Locate(at));
    Ok(json!({"at": at}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    fn undo_len(e: &mut Engine) -> usize {
        e.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
    }

    #[test]
    fn cycle_on_makes_a_region_in_the_toggles_undo_step_and_off_keeps_it() {
        use soundcraft_time::Range;
        let mut e = Engine::default();
        let (steps, tempo, sr) = (undo_len(&mut e), e.session().tempo.clone(), e.session().sample_rate);
        let bar = |n: i64| tempo.tick_to_samples(tempo.bar_start_tick(n), sr);
        // A selection is promoted.
        e.session_mut().edit.selection = Range::new(500, 9000);
        e.execute("options.loop_playback", &json!({"value": true})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(500, 9000));
        assert_eq!(undo_len(&mut e), steps + 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().edit.loop_range.is_empty() && !e.session().edit.loop_playback);
        // No selection: four bars from the bar holding `at`.
        e.session_mut().edit.selection = Range::default();
        e.execute("options.loop_playback", &json!({"value": true, "at": bar(2) + 10})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(bar(2), bar(6)));
        // Off keeps it (no undo step), on again reuses it.
        let n = undo_len(&mut e);
        e.execute("options.loop_playback", &json!({"value": false})).unwrap();
        assert_eq!(undo_len(&mut e), n);
        e.execute("options.loop_playback", &json!({"value": true, "at": bar(9)})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(bar(2), bar(6)));
        assert_eq!(undo_len(&mut e), n);
    }

    /// Two MIDI clips on one track, (1000..3000) and (5000..9000), both selected by id.
    fn two_selected_clips(e: &mut Engine) -> (u64, u64) {
        let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = r["tracks"][0].as_u64().unwrap();
        let a = e.execute("midi.new_clip", &json!({"tracks": [t], "start": 1000, "end": 3000})).unwrap()["clip"].as_u64().unwrap();
        let b = e.execute("midi.new_clip", &json!({"tracks": [t], "start": 5000, "end": 9000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("edit.select", &json!({"clips": [a, b]})).unwrap();
        (a, b)
    }

    #[test]
    fn cycle_on_with_clips_selected_takes_their_span_even_when_a_region_exists() {
        use soundcraft_time::Range;
        let mut e = Engine::default();
        two_selected_clips(&mut e);
        e.execute("transport.loop", &json!({"start": 100, "end": 200})).unwrap();
        let steps = undo_len(&mut e);
        e.execute("options.loop_playback", &json!({"value": true})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(1000, 9000), "earliest start to latest end");
        assert!(e.session().edit.loop_playback);
        assert_eq!(undo_len(&mut e), steps + 1);
        // Off keeps the region, with no undo step.
        e.execute("options.loop_playback", &json!({"value": false})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(1000, 9000));
        assert_eq!(undo_len(&mut e), steps + 1);
        // The same selection again changes nothing about the region and adds no step.
        e.execute("options.loop_playback", &json!({"value": true})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(1000, 9000));
        assert_eq!(undo_len(&mut e), steps + 1);
        // Undo restores the earlier region and the switch.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(100, 200));
        assert!(!e.session().edit.loop_playback);
    }

    #[test]
    fn cycle_on_with_a_time_selection_takes_it_even_when_a_region_exists() {
        use soundcraft_time::Range;
        let mut e = Engine::default();
        e.execute("transport.loop", &json!({"start": 100, "end": 200})).unwrap();
        e.session_mut().edit.selection = Range::new(500, 9000);
        let steps = undo_len(&mut e);
        e.execute("options.loop_playback", &json!({"value": true})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(500, 9000));
        assert_eq!(undo_len(&mut e), steps + 1);
    }

    #[test]
    fn cycle_on_with_nothing_selected_reuses_the_region_or_makes_four_bars() {
        use soundcraft_time::Range;
        let mut e = Engine::default();
        let (tempo, sr) = (e.session().tempo.clone(), e.session().sample_rate);
        let bar = |n: i64| tempo.tick_to_samples(tempo.bar_start_tick(n), sr);
        // No region: four bars from the bar holding `at`.
        e.execute("options.loop_playback", &json!({"value": true, "at": bar(2) + 10})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(bar(2), bar(6)));
        // A region and nothing selected: reused, no undo step.
        e.execute("options.loop_playback", &json!({"value": false})).unwrap();
        e.execute("transport.loop", &json!({"start": 300, "end": 700})).unwrap();
        let steps = undo_len(&mut e);
        e.execute("options.loop_playback", &json!({"value": true, "at": bar(9)})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(300, 700));
        assert_eq!(undo_len(&mut e), steps);
    }

    #[test]
    fn cycle_to_selection_sets_the_region_without_toggling_cycle() {
        use soundcraft_time::Range;
        let mut e = Engine::default();
        // Nothing selected: an error, no change, no undo step.
        let steps = undo_len(&mut e);
        assert!(e.execute("transport.cycle_to_selection", &json!({})).is_err());
        assert!(e.session().edit.loop_range.is_empty() && !e.session().edit.loop_playback);
        assert_eq!(undo_len(&mut e), steps);
        // Clips: the region is their span; Cycle stays off.
        two_selected_clips(&mut e);
        let steps = undo_len(&mut e);
        e.execute("transport.cycle_to_selection", &json!({})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(1000, 9000));
        assert!(!e.session().edit.loop_playback);
        assert_eq!(undo_len(&mut e), steps + 1);
        // Again with the same selection: no change, no step. With Cycle on it stays on.
        e.execute("transport.cycle_to_selection", &json!({})).unwrap();
        assert_eq!(undo_len(&mut e), steps + 1);
        e.execute("options.loop_playback", &json!({"value": true})).unwrap();
        e.execute("edit.select", &json!({"clips": []})).unwrap();
        e.session_mut().edit.selection = Range::new(20, 40);
        e.execute("transport.cycle_to_selection", &json!({})).unwrap();
        assert_eq!(e.session().edit.loop_range, Range::new(20, 40));
        assert!(e.session().edit.loop_playback);
    }

    #[test]
    fn transport_loop_sets_the_region_in_one_undo_step() {
        let mut e = Engine::default();
        let steps = undo_len(&mut e);
        let v = e.execute("transport.loop", &json!({"start": 4800, "end": 1200, "enabled": true})).unwrap();
        assert_eq!((v["start"].as_i64(), v["end"].as_i64(), v["enabled"].as_bool()), (Some(1200), Some(4800), Some(true)), "ordered");
        assert_eq!(e.session().edit.loop_range, soundcraft_time::Range::new(1200, 4800));
        assert!(e.session().edit.loop_playback);
        assert_eq!(undo_len(&mut e), steps + 1);
        // Setting it again is no change, so no undo step.
        e.execute("transport.loop", &json!({"start": 1200, "end": 4800})).unwrap();
        assert_eq!(undo_len(&mut e), steps + 1);
        // Enabled alone leaves the region.
        e.execute("transport.loop", &json!({"enabled": false})).unwrap();
        assert_eq!(e.session().edit.loop_range, soundcraft_time::Range::new(1200, 4800));
        assert!(!e.session().edit.loop_playback);
        // An empty pair clears; a lone bound is refused; negatives clamp to 0.
        e.execute("transport.loop", &json!({"start": 7, "end": 7})).unwrap();
        assert!(e.session().edit.loop_range.is_empty());
        assert!(e.execute("transport.loop", &json!({"start": 7})).is_err());
        e.execute("transport.loop", &json!({"start": -50, "end": 100})).unwrap();
        assert_eq!(e.session().edit.loop_range, soundcraft_time::Range::new(0, 100));
        // Undo restores the previous region.
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().edit.loop_range.is_empty());
    }
}
