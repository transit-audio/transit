//! One drop router (#368): a file dropped from the file manager goes to exactly one claimant, the
//! editor window on top at the drop's position or else the arrangement. The drop's position is the
//! pointer's first report after the drop, never the stale position egui held during the OS drag.

use super::test_rig::{T, mid};
use transit_model::TrackId;

/// A point on the menu bar: no track, nor the space below the tracks, so a drop there lands at the
/// cursor.
const MENU_BAR: (f32, f32) = (300.0, 12.0);

/// A point on the arrangement's timeline that no editor window covers, below the tracks (a drop
/// there lands on a new track; the Sampler's own instrument track takes no audio).
fn arrangement_point(t: &T, tr: TrackId) -> (f32, f32) {
    let tl = t.app.arrange.layout.timeline;
    let w = t.window(tr).rect;
    let inside = |p: (f32, f32)| p.0 >= w[0] && p.0 <= w[2] && p.1 >= w[1] && p.1 <= w[3];
    let candidates = [(tl[0] + 40.0, tl[3] - 12.0), (tl[2] - 40.0, tl[3] - 12.0)];
    *candidates.iter().find(|p| !inside(**p) && tl[3] - tl[1] > 24.0).expect("a visible stretch of timeline outside the editor window")
}

#[test]
fn a_file_dropped_on_a_pad_binds_the_pad_and_imports_no_clip() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    t.drop_file(&snare, t.pad_at(tr, 3));
    assert_eq!(t.patch(tr).pad(3).zone, Some(0), "the pad is bound");
    assert_eq!(t.clip_count(), clips, "no arrangement clip");
    // Negative control: the binary's old path (every drop to the arrangement import) adds a clip.
    crate::drops::open_path(&mut t.app, &snare.to_string_lossy());
    t.frames(3);
    assert_eq!(t.clip_count(), clips + 1, "the arrangement import is what the router kept out");
}

#[test]
fn a_file_dropped_on_the_arrangement_imports_a_clip_and_binds_no_pad() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    let at = arrangement_point(&t, tr);
    t.drop_file(&snare, at);
    assert_eq!(t.clip_count(), clips + 1, "one clip imported");
    assert!(t.patch(tr).zones.is_empty(), "no pad bound");
}

#[test]
fn a_file_dropped_on_the_title_bar_or_the_strip_imports_nothing() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    let before = t.undo_len();
    let w = t.window(tr).rect;
    t.drop_file(&snare, ((w[0] + w[2]) / 2.0, w[1] + 10.0));
    t.drop_file(&snare, mid(t.panel(tr).strip.panel));
    assert_eq!(t.clip_count(), clips, "the window refused: no arrangement clip");
    assert!(t.patch(tr).zones.is_empty(), "and no pad");
    assert_eq!(t.undo_len(), before);
    // Negative control: the same file on a pad is taken.
    t.drop_file(&snare, t.pad_at(tr, 0));
    assert_eq!(t.patch(tr).pad(0).zone, Some(0));
}

#[test]
fn a_drop_with_a_stale_pointer_waits_for_the_pointer_and_lands_on_the_pad_under_it() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    // egui's pointer was last over the arrangement; the OS drag reports no motion.
    let arrangement = arrangement_point(&t, tr);
    t.point(arrangement);
    t.drop_unplaced(&snare);
    // Negative control: routing at the stale position would have imported a clip. The drop is held.
    assert_eq!(t.clip_count(), clips, "nothing routed at the stale position");
    assert!(t.patch(tr).zones.is_empty());
    // The pointer's first report after the drop places it: on pad A2.
    t.point(t.pad_at(tr, 1));
    assert_eq!(t.patch(tr).pad(1).zone, Some(0), "the pad under the pointer is bound");
    assert_eq!(t.clip_count(), clips, "and no clip was imported");
    // Taken once: more motion routes nothing again.
    t.point(arrangement);
    assert_eq!(t.clip_count(), clips);
}

#[test]
fn a_kits_window_loads_nothing_from_a_file_dropped_off_the_grid() {
    // Kept from the retired one-sample panel's tests (#455): on the pads, a file dropped off the
    // grid is not loaded.
    let (mut t, tr) = T::open();
    let hit = t.sample("Hit.wav", 2400);
    let footer = t.panel(tr).footer;
    t.drop_file(&hit, mid(footer));
    let ins = t.app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    assert!(ins.sample.is_none());
    assert!(t.patch(tr).zones.is_empty());
}

#[test]
fn a_drop_with_the_pointer_gone_is_held_and_the_next_position_binds_the_pad() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    t.drop_gone(&snare);
    t.frames(2);
    assert_eq!(t.clip_count(), clips, "no blind import on PointerGone");
    assert!(t.patch(tr).zones.is_empty(), "and no pad yet");
    t.point(t.pad_at(tr, 2));
    assert_eq!(t.patch(tr).pad(2).zone, Some(0), "the pad under the pointer's next report is bound");
    assert_eq!(t.clip_count(), clips, "and no clip was imported");
}

#[test]
fn a_drop_with_the_pointer_gone_then_a_position_on_the_arrangement_imports_a_clip() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    t.drop_gone(&snare);
    assert!(t.clip_count() == clips && !t.app.imports.active(), "held");
    t.point(arrangement_point(&t, tr));
    t.settle_imports();
    assert_eq!(t.clip_count(), clips + 1, "one clip imported");
    assert!(t.patch(tr).zones.is_empty(), "no pad bound");
}

#[test]
fn a_drop_held_after_the_pointer_left_goes_to_the_arrangement_after_the_grace_frames() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    // The drop frame counts as the first held frame; the last held frame is the grace count.
    t.drop_gone(&snare);
    t.frames(crate::drops::GONE_GRACE_FRAMES as usize - 1);
    assert!(t.clip_count() == clips && !t.app.imports.active(), "still held on the last grace frame");
    t.frames(1);
    assert!(t.app.imports.active() || t.clip_count() == clips + 1, "routed on the next frame");
    t.settle_imports();
    assert_eq!(t.clip_count(), clips + 1, "then imported to the arrangement: the drop never vanishes");
    assert!(t.patch(tr).zones.is_empty());
}

#[test]
fn the_gone_rule_holds_where_the_old_rule_imported() {
    use crate::drops::{GONE_GRACE_FRAMES, Placement, placement};
    // The #368 rule, kept here as the negative control: `(None, true)` routed blind at once.
    let old = |moved: Option<egui::Pos2>, gone: bool| match (moved, gone) {
        (Some(p), _) => Placement::Route(Some(p)),
        (None, true) => Placement::Route(None),
        (None, false) => Placement::Hold,
    };
    assert_eq!(old(None, true), Placement::Route(None), "the old rule imports on PointerGone");
    let mut count = None;
    assert_eq!(placement(None, true, &mut count), Placement::Hold, "the new rule holds");
    for _ in 1..GONE_GRACE_FRAMES {
        assert_eq!(placement(None, false, &mut count), Placement::Hold);
    }
    assert_eq!(placement(None, false, &mut count), Placement::Route(None), "released after the grace frames");
    assert_eq!(count, None, "and the count is reset");
    // A position wins at any point and resets the count; no gone and no position holds forever.
    let p = egui::pos2(10.0, 20.0);
    let mut count = None;
    let _ = placement(None, true, &mut count);
    assert_eq!(placement(Some(p), true, &mut count), Placement::Route(Some(p)));
    assert_eq!(count, None);
    for _ in 0..(GONE_GRACE_FRAMES * 2) {
        assert_eq!(placement(None, false, &mut count), Placement::Hold);
    }
}

/// A file-manager drop lands where it is let go, as a browser drag does: on the track under the
/// pointer, at the pointer's time on its lane, rather than at the cursor (on a new track when the
/// cursor has none). A stereo file keeps its channels there: a split is asked for, never done on
/// import, even with a free mono track below.
#[test]
fn a_stereo_file_dropped_on_a_mono_lane_lands_whole_on_that_track_at_the_pointer() {
    let mut t = T::new();
    let mono = |t: &mut T| t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    let (a, b) = (mono(&mut t), mono(&mut t));
    t.frames(3);
    let mut wav = Vec::new();
    for v in [0x4646_4952u32, 36 + 4_800 * 4, 0x4556_4157, 0x2074_6d66, 16, 0x0002_0001, 48_000, 48_000 * 4, 0x0010_0004, 0x6174_6164, 4_800 * 4] {
        wav.extend(v.to_le_bytes());
    }
    for _ in 0..4_800 {
        wav.extend([0x00, 0x40, 0x00, 0xe0]); // 0.5 left, -0.25 right
    }
    let path = t.dir.join("Mics.wav");
    std::fs::write(&path, wav).unwrap();
    let lane = t.app.arrange.layout.tracks.iter().find(|r| r.id == a).unwrap().lane;
    let at = (lane[0] + (lane[2] - lane[0]) * 0.75, (lane[1] + lane[3]) / 2.0);
    let Some((crate::browser::Target::Track { at: Some(want), .. }, _)) = t.app.drop_target(&t.ctx, egui::pos2(at.0, at.1), true, true) else {
        panic!("the lane is a drop target")
    };
    assert!(want > 0 && t.app.cursor() == 0, "the pointer's time ({want}) is not the cursor's");
    t.drop_file(&path, at);
    let s = t.app.engine.session();
    let on = |id: u64| s.track(TrackId(id)).unwrap().clips().iter().map(|c| (c.name.clone(), c.start)).collect::<Vec<_>>();
    assert_eq!(s.tracks.len(), 2, "no new track");
    assert_eq!((on(a), on(b)), (vec![("Mics".to_string(), want)], vec![]), "whole, on A, at the pointer");
    assert_eq!(s.sources.iter().map(|x| x.channels).collect::<Vec<_>>(), [2], "one stereo source");
}

/// Several files dropped on a lane follow each other on its track, as a browser drag's do: each
/// lands where the one before ends, so none erases another.
#[test]
fn files_dropped_together_on_a_lane_follow_each_other_on_its_track() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.frames(3);
    let (one, two) = (t.sample("One.wav", 4_800), t.sample("Two.wav", 4_800));
    let lane = t.app.arrange.layout.tracks.iter().find(|r| r.id == a).unwrap().lane;
    t.drop_files(vec![one, two], mid(lane));
    let clips: Vec<(String, i64)> = t.app.engine.session().track(TrackId(a)).unwrap().clips().iter().map(|c| (c.name.clone(), c.start)).collect();
    assert_eq!(clips.len(), 2, "both files are on the track: {clips:?}");
    assert_eq!(clips[1].1 - clips[0].1, 4_800, "Two starts where One ends: {clips:?}");
}

/// A drop holding a session opens it and imports nothing else: its other files would land by a
/// pointer or a cursor that still names the closing session's tracks (here the cursor is on the
/// track with the id of the new session's MIDI track).
#[test]
fn a_drop_that_opens_a_session_opens_it_and_imports_nothing_else() {
    let mut t = T::new();
    let mut other = transit_engine::Engine::default();
    other.execute("track.new", &serde_json::json!({"kind": "audio", "name": "Drums"})).unwrap();
    let synth = other.execute("track.new", &serde_json::json!({"kind": "midi", "name": "Synth"})).unwrap()["tracks"][0].as_u64().unwrap();
    let b = t.dir.join("B.transit");
    other.execute("session.save", &serde_json::json!({"path": b.to_string_lossy()})).unwrap();
    for _ in 0..2 {
        t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap();
    }
    t.app.run("session.save", serde_json::json!({"path": t.dir.join("A.transit").to_string_lossy()})).unwrap();
    t.app.set_cursor(96_000, Some(TrackId(synth)));
    t.frames(3);
    let lane = t.app.arrange.layout.tracks.iter().find(|r| r.id == synth).expect("A has a track of Synth's id").lane;
    let (one, two) = (t.sample("One.wav", 2_400), t.sample("Two.wav", 2_400));
    t.drop_files(vec![b, one, two], mid(lane));
    let s = t.app.engine.session();
    assert!(s.tracks.iter().any(|tr| tr.name == "Synth"), "B is open");
    assert!(s.tracks.iter().all(|tr| tr.clips().is_empty()), "nothing else is imported");
    assert!(t.app.status.contains("2 files dropped with B.transit were not imported"), "{}", t.app.status);
}

/// Files dropped where no track or the space below them is (here the menu bar) land at the cursor
/// on the cursor's track, and follow each other there, rather than each erasing the last.
#[test]
fn files_dropped_at_the_cursor_follow_each_other_on_its_track() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.app.run("track.new", serde_json::json!({"kind": "midi"})).unwrap();
    t.app.set_cursor(48_000, Some(TrackId(a)));
    t.frames(3);
    let (one, two) = (t.sample("One.wav", 4_800), t.sample("Two.wav", 2_400));
    t.drop_files(vec![one, two], MENU_BAR);
    let clips: Vec<(String, i64)> = t.app.engine.session().track(TrackId(a)).unwrap().clips().iter().map(|c| (c.name.clone(), c.start)).collect();
    assert_eq!(clips, [("One".to_string(), 48_000), ("Two".to_string(), 52_800)]);
}

/// Audio dropped on a track that takes none (a MIDI track's lane) is refused with the reason on
/// the status line, as a browser drag's is, rather than landing somewhere else unannounced.
#[test]
fn audio_dropped_on_a_midi_lane_is_refused_with_the_reason() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    let m = t.app.run("track.new", serde_json::json!({"kind": "midi", "name": "Synth"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.app.set_cursor(0, Some(TrackId(a)));
    t.frames(3);
    let lane = t.app.arrange.layout.tracks.iter().find(|r| r.id == m).unwrap().lane;
    let one = t.sample("One.wav", 2_400);
    t.drop_file(&one, mid(lane));
    assert!(t.app.status.starts_with("Synth is not an audio track"), "{}", t.app.status);
    assert!(t.app.engine.session().tracks.iter().all(|tr| tr.clips().is_empty()), "nothing is imported");
}

/// A dropped session goes through the save question File > Open asks: with unsaved changes it
/// opens only once the question is answered, and nothing else of the drop is imported.
#[test]
fn a_dropped_session_asks_to_save_unsaved_changes_first() {
    let mut t = T::new();
    let b = t.dir.join("B.transit");
    let mut other = transit_engine::Engine::default();
    other.execute("track.new", &serde_json::json!({"kind": "midi", "name": "Synth"})).unwrap();
    other.execute("session.save", &serde_json::json!({"path": b.to_string_lossy()})).unwrap();
    t.app.run("track.new", serde_json::json!({"kind": "audio", "name": "Unsaved"})).unwrap();
    t.frames(3);
    assert!(t.app.unsaved_changes());
    let one = t.sample("One.wav", 2_400);
    t.drop_files(vec![b, one], MENU_BAR);
    assert!(matches!(t.app.dialog, Some(crate::Dialog::SaveChanges { .. })), "the save question is up");
    let s = t.app.engine.session();
    assert!(s.tracks.iter().any(|tr| tr.name == "Unsaved") && s.tracks.iter().all(|tr| tr.clips().is_empty()), "nothing opened or imported yet");
}

/// With the cursor before bar 1 (a count-in session's pre-count zone), files dropped at the cursor
/// still follow each other: the first lands at 0, where the engine puts it, and the next after it.
#[test]
fn files_dropped_at_a_cursor_before_bar_one_follow_each_other_from_zero() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    let m = t.app.run("track.new", serde_json::json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap();
    // A pickup clip at -96 000 opens the pre-count zone (ADR 0052), so the cursor can sit before 0.
    let s = t.app.engine.session_mut();
    let id = s.new_clip_id();
    let pickup = transit_model::Clip::midi(id, "Pickup", -96_000, 48_000, Default::default());
    s.track_mut(TrackId(m)).unwrap().playlist_mut().unwrap().clips.push(pickup);
    t.frames(3);
    t.app.ui.cursor = -48_000;
    t.app.ui.cursor_track = Some(TrackId(a));
    assert_eq!(t.app.cursor(), -48_000, "the cursor is before bar 1");
    let (one, two) = (t.sample("One.wav", 4_800), t.sample("Two.wav", 4_800));
    t.drop_files(vec![one, two], MENU_BAR);
    let clips: Vec<(String, i64)> = t.app.engine.session().track(TrackId(a)).unwrap().clips().iter().map(|c| (c.name.clone(), c.start)).collect();
    assert_eq!(clips, [("One".to_string(), 0), ("Two".to_string(), 4_800)]);
}

/// A dropped session that fails to open keeps its own reason on the status line: the line about
/// the files left out comes first, so the open's refusal replaces it.
#[test]
fn a_dropped_session_that_fails_to_open_shows_why() {
    let mut t = T::new();
    let bad = t.dir.join("B.transit");
    std::fs::write(&bad, b"not a session").unwrap();
    let (one, two) = (t.sample("One.wav", 2_400), t.sample("Two.wav", 2_400));
    let tl = t.app.arrange.layout.timeline;
    t.drop_files(vec![bad, one, two], (tl[0] + 40.0, tl[1] + 12.0));
    assert!(t.app.status.starts_with("Could not open"), "{}", t.app.status);
    assert!(t.app.engine.session().tracks.iter().all(|tr| tr.clips().is_empty()), "nothing is imported");
}

/// Whether a dropped file is audio is the engine's call, as it was for every file-manager drop:
/// WAV data named `.rf64`, or with no extension at all, imports, though the browser lists neither.
#[test]
fn audio_the_engine_reads_imports_whatever_its_file_is_named() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.frames(3);
    let (rf64, bare) = (t.sample("Kick.rf64", 2_400), t.sample("Snare", 2_400));
    let lane = t.app.arrange.layout.tracks.iter().find(|r| r.id == a).unwrap().lane;
    t.drop_files(vec![rf64, bare], mid(lane));
    let names: Vec<String> = t.app.engine.session().track(TrackId(a)).unwrap().clips().iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["Kick", "Snare"], "{}", t.app.status);
}

/// A file that is not audio, dropped with one that is, is named on the drop's last line with the
/// engine's reason (never as a Maschine/Kontakt file), though the audio after it commits later.
#[test]
fn a_file_that_is_not_audio_in_a_drop_is_named_with_its_reason() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.frames(3);
    let notes = t.dir.join("notes.txt");
    std::fs::write(&notes, b"not audio").unwrap();
    let kick = t.sample("Kick.wav", 2_400);
    let lane = t.app.arrange.layout.tracks.iter().find(|r| r.id == a).unwrap().lane;
    t.drop_files(vec![notes, kick], mid(lane));
    assert_eq!(t.app.engine.session().track(TrackId(a)).unwrap().clips().len(), 1, "Kick lands");
    let status = &t.app.status;
    assert!(status.contains("notes.txt (") && !status.contains("Maschine"), "{status}");
}

/// A drop at the cursor lands on the cursor's track, as it did before drops followed the pointer,
/// even when an earlier import left the focus on another audio track.
#[test]
fn a_drop_at_the_cursor_lands_on_the_cursors_track_not_the_focused_one() {
    let mut t = T::new();
    let new = |t: &mut T| t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    let (a, b) = (new(&mut t), new(&mut t));
    t.app.set_cursor(48_000, Some(TrackId(a)));
    t.app.ui.focus_track = Some(TrackId(b));
    t.frames(3);
    let one = t.sample("One.wav", 2_400);
    t.drop_file(&one, MENU_BAR);
    let on = |id: u64| t.app.engine.session().track(TrackId(id)).unwrap().clips().iter().map(|c| c.start).collect::<Vec<_>>();
    assert_eq!((on(a), on(b)), (vec![48_000], vec![]), "{}", t.app.status);
}

/// When the last file of a drop fails too, the drop's last line still names every file that
/// failed, and says where the others were imported, as it does when the failure comes first; a
/// lone failure keeps its own line.
#[test]
fn a_drop_whose_last_file_fails_names_every_failure() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.frames(3);
    let lane = mid(t.app.arrange.layout.tracks.iter().find(|r| r.id == a).unwrap().lane);
    let text = |t: &T, name: &str| {
        let p = t.dir.join(name);
        std::fs::write(&p, b"not audio").unwrap();
        p
    };
    let (x, y) = (text(&t, "a.txt"), text(&t, "b.txt"));
    t.drop_files(vec![x, y], lane);
    assert!(t.app.status.starts_with("2 of 2 could not be prepared: a.txt (") && t.app.status.contains("b.txt"), "{}", t.app.status);
    let (kick, notes) = (t.sample("Kick.wav", 2_400), text(&t, "notes.txt"));
    t.drop_files(vec![kick, notes], lane);
    assert!(
        t.app.status.starts_with("Imported 1 of 2 files to Audio 1 at ") && t.app.status.contains("; 1 of 2 could not be prepared: notes.txt ("),
        "{}",
        t.app.status
    );
    let lone = text(&t, "c.txt");
    t.drop_files(vec![lone], lane);
    assert!(t.app.status.starts_with("Couldn't import c.txt: "), "{}", t.app.status);
}

/// A drop on a track deleted since the frame it was seen in says so, rather than importing nothing
/// unannounced.
#[test]
fn a_drop_on_a_deleted_track_says_so() {
    let mut t = T::new();
    let one = t.sample("One.wav", 2_400);
    let r = t.app.import_files(&[one], Some(TrackId(9_999)), 0);
    assert!(r.is_err() && t.app.status == "The track was deleted: drop the files again", "{}", t.app.status);
    assert!(!t.app.imports.active());
}

/// The mixer's Main strip with no Master track is the main bus, no track: a file dropped there
/// lands at the cursor, as anywhere else no track is, never refused as a deleted track.
#[test]
fn a_drop_on_the_main_strip_with_no_master_track_lands_at_the_cursor() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.app.set_cursor(48_000, Some(TrackId(a)));
    t.app.ui.show_mixer = true;
    t.frames(3);
    assert!(t.app.engine.session().tracks.iter().all(|tr| tr.kind != transit_model::TrackKind::Master));
    let main = t.app.mixer.layout.master.clone().expect("the Main strip is drawn");
    let one = t.sample("One.wav", 2_400);
    t.drop_file(&one, mid(main.rect));
    let starts: Vec<i64> = t.app.engine.session().track(TrackId(a)).unwrap().clips().iter().map(|c| c.start).collect();
    assert_eq!(starts, [48_000], "{}", t.app.status);
}

/// A real Master track's strip is still a drop target, refused for audio with the reason (only the
/// main bus with no Master track is no target).
#[test]
fn audio_dropped_on_a_master_tracks_strip_is_refused_with_the_reason() {
    let mut t = T::new();
    let a = t.app.run("track.new", serde_json::json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.app.run("track.new", serde_json::json!({"kind": "master"})).unwrap();
    t.app.set_cursor(0, Some(TrackId(a)));
    t.app.ui.show_mixer = true;
    t.frames(3);
    let master = t.app.mixer.layout.master.clone().expect("the Master strip is drawn");
    assert_ne!(master.track, 0, "a Master track's own strip");
    let one = t.sample("One.wav", 2_400);
    t.drop_file(&one, mid(master.rect));
    assert!(t.app.status.contains("is not an audio track"), "{}", t.app.status);
    assert!(t.app.engine.session().tracks.iter().all(|tr| tr.clips().is_empty()), "nothing is imported");
}
