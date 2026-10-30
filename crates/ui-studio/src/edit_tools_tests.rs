//! Clip tools of #181 slice 1: observable state only (clips, undo depth, status line).

use crate::edit_tools::Tool;
use crate::tests::*;
use transit_time::Range;

/// Press `key` with `mods` ("cmd", "alt", "shift" set) and let the frame handle it.
fn press(r: &mut CopyRig, key: &str, mods: serde_json::Value) {
    let mut p = json!({"key": key});
    if let (Some(o), Some(m)) = (p.as_object_mut(), mods.as_object()) {
        o.extend(m.clone());
    }
    send_control(&mut r.app, "ui.key", p);
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
}

fn second_clip(r: &mut CopyRig, start: i64) -> ClipId {
    let c = r.app.run("midi.new_clip", json!({"tracks": [r.t.0], "start": start, "end": start})).unwrap();
    ClipId(c["clip"].as_u64().unwrap())
}

fn select(r: &mut CopyRig, clips: &[ClipId]) {
    r.app.run("edit.select", json!({"clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()})).unwrap();
}

fn first_clip(r: &CopyRig) -> ClipId {
    ClipId(clips_on(&r.app, r.t)[0].0)
}

fn set_time_selection(r: &mut CopyRig, a: i64, b: i64) {
    r.app.engine.session_mut().edit.selection = Range::new(a, b);
}

#[test]
fn ctrl_j_joins_two_adjacent_clips_into_one_in_one_undo_step() {
    let mut r = copy_rig();
    let bar = r.beat * 4;
    let a = first_clip(&r);
    let b = second_clip(&mut r, bar);
    assert_eq!(clips_on(&r.app, r.t).len(), 2);
    select(&mut r, &[a, b]);
    let before = undo_len(&mut r.app);
    press(&mut r, "J", json!({"cmd": true}));
    let clips = clips_on(&r.app, r.t);
    assert_eq!(clips.iter().map(|c| (c.1, c.2)).collect::<Vec<_>>(), vec![(0, 2 * bar)], "{clips:?}");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    assert_eq!(r.app.engine.session().edit.selected_clips.len(), 1, "the joined clip stays selected");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(clips_on(&r.app, r.t).len(), 2, "one undo restores both clips");
}

#[test]
fn consolidating_a_time_selection_with_a_gap_makes_one_clip_as_long_as_the_selection() {
    let mut r = copy_rig();
    let bar = r.beat * 4;
    second_clip(&mut r, 3 * bar);
    r.app.deselect_all();
    let (from, to) = (bar / 2, 3 * bar + bar / 2);
    r.app.run("edit.select", json!({"tracks": [r.t.0], "start": from, "end": to})).unwrap();
    assert!(r.app.engine.session().edit.selected_clips.is_empty());
    let before = undo_len(&mut r.app);
    r.app.join_selected().unwrap();
    let inside: Vec<_> = clips_on(&r.app, r.t).into_iter().filter(|c| c.1 == from).collect();
    assert_eq!(inside.len(), 1, "{:?}", clips_on(&r.app, r.t));
    assert_eq!(inside[0].2 - inside[0].1, to - from, "the clip is as long as the selection");
    assert_eq!(undo_len(&mut r.app), before + 1);
}

#[test]
fn crop_trims_both_edges_to_the_time_selection_on_the_grid() {
    let mut r = copy_rig();
    let (clip, beat) = (first_clip(&r), r.beat);
    select(&mut r, &[clip]);
    set_time_selection(&mut r, beat + 40, 3 * beat - 25);
    let before = undo_len(&mut r.app);
    r.app.crop_to_selection().unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(beat, 3 * beat)], "the edges snap to the beat grid");
    assert_eq!(undo_len(&mut r.app), before + 1);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(0, 4 * beat)]);
    // With the snap off the selection is used as drawn.
    r.app.arrange.grid.snap_enabled = false;
    select(&mut r, &[clip]);
    set_time_selection(&mut r, beat + 40, 3 * beat - 25);
    r.app.crop_to_selection().unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(beat + 40, 3 * beat - 25)]);
}

#[test]
fn crop_only_touches_the_selected_clips() {
    let mut r = copy_rig();
    let (a, beat) = (first_clip(&r), r.beat);
    let b = second_clip(&mut r, 4 * beat);
    select(&mut r, &[a]);
    set_time_selection(&mut r, beat, 6 * beat);
    r.app.crop_to_selection().unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(beat, 4 * beat), (4 * beat, 8 * beat)], "the unselected clip {b:?} keeps its length");
}

#[test]
fn trim_start_and_end_to_the_cursor_with_the_bracket_keys() {
    let mut r = copy_rig();
    let (clip, beat) = (first_clip(&r), r.beat);
    select(&mut r, &[clip]);
    r.app.set_cursor(2 * beat, Some(r.t));
    let before = undo_len(&mut r.app);
    press(&mut r, "[", json!({}));
    assert_eq!(spans(&r.app, r.t), vec![(2 * beat, 4 * beat)]);
    assert_eq!(undo_len(&mut r.app), before + 1);
    r.app.run("edit.undo", json!({})).unwrap();
    press(&mut r, "]", json!({}));
    assert_eq!(spans(&r.app, r.t), vec![(0, 2 * beat)]);
    assert_eq!(undo_len(&mut r.app), before + 1);
}

#[test]
fn trim_to_cursor_refuses_with_the_cursor_outside_the_clip() {
    let mut r = copy_rig();
    let (clip, beat) = (first_clip(&r), r.beat);
    select(&mut r, &[clip]);
    r.app.set_cursor(9 * beat, Some(r.t));
    assert!(r.app.trim_to_cursor(true).is_err());
    assert!(r.app.status.contains("cursor"), "{}", r.app.status);
    assert_eq!(spans(&r.app, r.t), vec![(0, 4 * beat)]);
}

#[test]
fn alt_arrows_nudge_by_the_grid_and_alt_shift_by_a_fine_step() {
    let mut r = copy_rig();
    let (clip, beat) = (first_clip(&r), r.beat);
    select(&mut r, &[clip]);
    let before = undo_len(&mut r.app);
    press(&mut r, "ArrowRight", json!({"alt": true}));
    assert_eq!(spans(&r.app, r.t), vec![(beat, 5 * beat)], "one grid step (a beat) later");
    assert_eq!(undo_len(&mut r.app), before + 1);
    press(&mut r, "ArrowRight", json!({"alt": true, "shift": true}));
    assert_eq!(spans(&r.app, r.t), vec![(beat + beat / 4, 5 * beat + beat / 4)], "a quarter of the grid step");
    assert_eq!(undo_len(&mut r.app), before + 2);
    press(&mut r, "ArrowLeft", json!({"alt": true}));
    assert_eq!(spans(&r.app, r.t), vec![(beat / 4, 4 * beat + beat / 4)]);
    assert_eq!(r.app.cursor(), 0, "the arrows alone, and the nudge, leave the cursor where it was");
    assert_eq!(r.app.engine.session().edit.selected_clips, vec![clip]);
}

#[test]
fn nudge_stops_at_the_start_of_the_timeline() {
    let mut r = copy_rig();
    let clip = first_clip(&r);
    select(&mut r, &[clip]);
    assert!(r.app.nudge_selected(-1, false).is_err());
    assert!(r.app.status.contains("Nudge"), "{}", r.app.status);
    assert_eq!(clips_on(&r.app, r.t)[0].1, 0);
}

#[test]
fn every_tool_is_disabled_with_nothing_selected_and_says_why() {
    let mut r = copy_rig();
    r.app.deselect_all();
    for tool in Tool::ALL {
        assert!(r.app.clip_tool_blocked(tool).is_some(), "{tool:?}");
        let rev = r.app.engine.revision;
        assert!(r.app.run_clip_tool(tool).is_err(), "{tool:?}");
        assert!(
            r.app.status.starts_with(match tool {
                Tool::Join => "Join",
                Tool::Crop => "Crop",
                Tool::TrimStart => "Trim start",
                Tool::TrimEnd => "Trim end",
                Tool::NudgeEarlier | Tool::NudgeLater => "Nudge",
            }),
            "{tool:?}: {}",
            r.app.status
        );
        assert_eq!(r.app.engine.revision, rev, "{tool:?} left the document alone");
    }
    let clip = first_clip(&r);
    select(&mut r, &[clip]);
    assert!(r.app.clip_tool_blocked(Tool::NudgeLater).is_none());
    assert!(r.app.clip_tool_blocked(Tool::Join).is_some(), "one clip alone has nothing to join");
}

#[test]
fn the_shortcut_rows_name_the_commands_they_follow() {
    use crate::shortcuts::{Action as A, label, rows};
    for (action, keys, cited) in [
        (A::Join, "Ctrl+J", &["Ableton's Consolidate", "Logic's Join"][..]),
        (A::Crop, "Ctrl+Shift+J", &["Ableton's Crop Selected Clips", "Logic has no default"]),
        (A::TrimStart, "[", &["Ours, Logic-style", "Set Region Start to Playhead Position"]),
        (A::TrimEnd, "]", &["Ours, Logic-style", "Set Region End to Playhead Position"]),
        (A::NudgeEarlier, "Alt+Left", &["Ableton nudges selected clips with the arrow keys", "Logic has no default", "Ours"]),
        (A::NudgeLater, "Alt+Right", &["Ableton nudges selected clips with the arrow keys", "Logic has no default", "Ours"]),
        (A::NudgeEarlierFine, "Alt+Shift+Left", &["Ours"]),
        (A::NudgeLaterFine, "Alt+Shift+Right", &["Ours"]),
    ] {
        assert_eq!(label(action), keys);
        for text in cited {
            assert!(rows(action).all(|s| s.note.contains(text)), "{action:?} should cite {text}");
        }
    }
    assert!(rows(A::NudgeLater).all(|s| s.label.contains("snap off")), "the row says what snap off does");
    assert_eq!(Tool::Crop.action(), Some(A::Crop));
}

#[test]
fn crop_and_trim_are_refused_in_shuffle_mode_and_change_nothing() {
    let mut r = copy_rig();
    let (a, beat) = (first_clip(&r), r.beat);
    let b = second_clip(&mut r, 4 * beat);
    r.app.run("edit.mode", json!({"mode": "shuffle"})).unwrap();
    select(&mut r, &[a, b]);
    set_time_selection(&mut r, beat, 7 * beat);
    r.app.set_cursor(2 * beat, Some(r.t));
    let (rev, before) = (r.app.engine.revision, spans(&r.app, r.t));
    for tool in [Tool::Crop, Tool::TrimStart, Tool::TrimEnd] {
        assert!(r.app.clip_tool_blocked(tool).is_some_and(|w| w.contains("Shuffle")), "{tool:?}");
        assert!(r.app.run_clip_tool(tool).is_err());
        assert!(r.app.status.contains("Shuffle"), "{}", r.app.status);
    }
    assert_eq!(spans(&r.app, r.t), before);
    assert_eq!(r.app.engine.revision, rev);
}

#[test]
fn a_time_range_join_refuses_where_no_clip_lies_in_the_range() {
    let mut r = copy_rig();
    let bar = r.beat * 4;
    r.app.deselect_all();
    r.app.run("edit.select", json!({"tracks": [r.t2.0], "start": 0, "end": bar})).unwrap();
    let rev = r.app.engine.revision;
    assert!(r.app.clip_tool_blocked(Tool::Join).is_some());
    assert!(r.app.join_selected().is_err());
    assert!(r.app.status.contains("nothing in the time range"), "{}", r.app.status);
    assert!(clips_on(&r.app, r.t2).is_empty(), "no empty clip made");
    assert_eq!(r.app.engine.revision, rev);
    // A selected track that is empty is skipped when another has material in the range.
    r.app.run("edit.select", json!({"tracks": [r.t.0, r.t2.0], "start": 0, "end": bar})).unwrap();
    r.app.join_selected().unwrap();
    assert!(clips_on(&r.app, r.t2).is_empty());
}

#[test]
fn color_sets_and_clears_the_selected_clips_color() {
    let mut r = copy_rig();
    let clip = first_clip(&r);
    assert!(r.app.color_selected(Some([1, 2, 3])).is_err(), "nothing selected");
    select(&mut r, &[clip]);
    r.app.color_selected(Some([1, 2, 3])).unwrap();
    assert_eq!(the_clip(&r.app, clip).color, Some([1, 2, 3]));
    r.app.color_selected(None).unwrap();
    assert_eq!(the_clip(&r.app, clip).color, None);
}
