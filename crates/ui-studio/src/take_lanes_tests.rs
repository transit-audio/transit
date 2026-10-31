//! Take lanes (#25): cycle recording opens a track's lanes with the newest take playing; a lane's
//! clip, its label, the header toggle, the track and clip menus and the lane menu, driven like a
//! user through the control channel's input queue. A child of `slice6_tests` for its harness.

use super::*;
use crate::arrange::TAKE_H;

/// A 16-bit mono WAV of `laps` laps of `lap` frames at 48 kHz, lap k (from 1) a constant
/// `k * 1000`, so each take renders its own level.
fn write_laps(path: &std::path::Path, lap: u32, laps: u32) {
    let frames = lap * laps;
    let data = frames * 2;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(48_000u32.to_le_bytes());
    b.extend(96_000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for i in 0..frames {
        b.extend(((i / lap + 1) as i16 * 1000).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

impl T {
    /// Stop a cycle recording of `laps` one-second laps from `start` on `t`, as Stop places it.
    fn record_cycle(&mut self, dir: &Dir, t: TrackId, start: i64, laps: u32) {
        let path = dir.0.join(format!("cycle{}.wav", t.0));
        write_laps(&path, 48_000, laps);
        let plan = transit_engine::io::Recording {
            start,
            pass: Some(48_000),
            punch: None,
            rate: 48_000,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
        };
        self.app.place_takes(&plan, vec![(t, path)], start + 48_000, 1);
        self.frames(3);
    }

    fn rows_of(&self, t: TrackId) -> Vec<crate::finish::TakeRow> {
        self.app.finish.layout.takes.iter().filter(|r| r.track == t.0).cloned().collect()
    }

    fn lane_clip(&self, t: TrackId, take: u32) -> crate::finish::TakeClip {
        self.app
            .finish
            .layout
            .take_clips
            .iter()
            .find(|c| c.track == t.0 && c.take == take)
            .cloned()
            .unwrap_or_else(|| panic!("no clip of take {take}"))
    }

    fn active_take(&self, t: TrackId) -> Option<u32> {
        self.app.engine.session().track(t).unwrap().take_stacks().first().and_then(transit_model::TakeStack::active)
    }

    /// The level the track renders half a second into a lap from 0.
    fn level(&self, t: TrackId) -> f32 {
        transit_mix::render_track_pre_fader(self.app.engine.session(), t, transit_time::Range::new(24_000, 24_064))[0][32]
    }

    fn right_click(&mut self, at: (f32, f32)) {
        self.input("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    }

    fn menu_item(&mut self, label: &str) {
        let r = self
            .app
            .arrange
            .menu
            .iter()
            .find(|m| m.label == label)
            .unwrap_or_else(|| panic!("no menu item {label}: {:?}", self.app.arrange.menu))
            .rect;
        self.click(mid(r));
    }

    /// An audio track after three laps of cycle recording over 0 s..1 s, zoomed in.
    fn cycled(&mut self, dir: &Dir) -> TrackId {
        let a = self.track(json!({"kind": "audio", "name": "Audio 1"}));
        let _ = self.app.engine.execute("view.zoom_set", &json!({"samples_per_px": 500.0}));
        self.record_cycle(dir, a, 0, 3);
        a
    }
}

#[test]
fn cycle_recording_opens_three_lanes_newest_on_top_with_the_newest_take_playing() {
    let dir = Dir::new("lanes3");
    let mut t = T::new();
    let a = t.cycled(&dir);
    assert_eq!(t.app.ui.takes_open, vec![a], "cycle recording shows the lanes (Logic, Live)");
    let rows = t.rows_of(a);
    assert_eq!(rows.iter().map(|r| r.take).collect::<Vec<_>>(), vec![3, 2, 1], "newest on top");
    assert_eq!(rows.iter().map(|r| r.active).collect::<Vec<_>>(), vec![true, false, false]);
    assert!(rows[1].lane[1] - rows[0].lane[1] >= TAKE_H - 0.5, "one lane per take, stacked");
    let h = t.row(a).header;
    assert!(h[3] - h[1] >= t.app.ui.track_height + 3.0 * TAKE_H - 1.0, "the row grew by the lanes: {h:?}");
    assert!(t.lane_clip(a, 3).active && !t.lane_clip(a, 2).active && !t.lane_clip(a, 1).active);
    assert_eq!(t.active_take(a), Some(3));
    let tr = t.app.engine.session().track(a).unwrap();
    assert_eq!(tr.active_playlist, 0, "the track plays the playlist it played before");
    assert!(t.level(a) > 0.0, "take 3 renders");
}

#[test]
fn clicking_take_2_in_its_lane_plays_it_in_one_undo_step_and_flatten_keeps_it() {
    let dir = Dir::new("lanes-click");
    let mut t = T::new();
    let a = t.cycled(&dir);
    let three = t.level(a);
    let before = t.undo_len();
    t.click(mid(t.lane_clip(a, 2).rect));
    assert_eq!(t.active_take(a), Some(2));
    assert_eq!(t.undo_len(), before + 1, "one undo step");
    assert_eq!(t.app.engine.undo_label(), Some("Choose Take"));
    assert!(t.app.status.contains("Take 2 plays on Audio 1"), "{}", t.app.status);
    assert!((t.level(a) / three - 2.0 / 3.0).abs() < 1e-3, "the mix renders take 2 now");
    let rows = t.rows_of(a);
    assert_eq!(rows.iter().map(|r| (r.take, r.active)).collect::<Vec<_>>(), vec![(3, false), (2, true), (1, false)], "lanes stay put");
    // Flatten from the lane menu: take 2 stays, the lanes go; one undo brings all three back.
    t.click(mid(rows[0].menu));
    t.menu_item("Flatten Takes");
    let tr = t.app.engine.session().track(a).unwrap().clone();
    assert_eq!(tr.playlists.len(), 1);
    assert_eq!(tr.clips().len(), 1);
    assert!(tr.take_stacks().is_empty());
    assert!(t.app.ui.takes_open.is_empty() && t.rows_of(a).is_empty(), "nothing left to show");
    assert!((t.level(a) / three - 2.0 / 3.0).abs() < 1e-3);
    assert_eq!(t.undo_len(), before + 2);
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    let tr = t.app.engine.session().track(a).unwrap();
    assert_eq!((tr.playlists.len(), tr.take_numbers()), (3, vec![3, 2, 1]));
    assert_eq!(t.active_take(a), Some(2));
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.active_take(a), Some(3));
}

#[test]
fn the_lanes_follow_vertical_scroll() {
    let dir = Dir::new("lanes-scroll");
    let mut t = T::new();
    let a = t.cycled(&dir);
    for i in 0..14 {
        t.track(json!({"kind": "audio", "name": format!("Fill {i}")}));
    }
    t.frames(3);
    let lane0 = t.rows_of(a)[0].lane;
    let clip0 = t.lane_clip(a, 3).rect;
    t.app.arrange.vscroll = 30.0;
    t.frames(3);
    assert!((t.app.arrange.vscroll - 30.0).abs() < 0.1, "the content is tall enough to scroll");
    assert!((t.rows_of(a)[0].lane[1] - (lane0[1] - 30.0)).abs() < 0.5, "the lane moved with the rows");
    assert!((t.lane_clip(a, 3).rect[1] - (clip0[1] - 30.0)).abs() < 0.5, "its clips too");
    assert!((t.row(a).header[1] - (lane0[1] - 30.0 - t.app.ui.track_height)).abs() < 1.5, "under its track");
}

#[test]
fn the_header_toggle_and_the_track_menu_show_and_hide_the_lanes() {
    let dir = Dir::new("lanes-toggle");
    let mut t = T::new();
    let a = t.cycled(&dir);
    let row = t.row(a);
    assert!(row.takes_shown && row.takes != [0.0; 4]);
    let rev = t.app.engine.revision;
    t.click(mid(row.takes));
    assert!(t.app.ui.takes_open.is_empty() && t.rows_of(a).is_empty());
    assert_eq!(t.app.engine.revision, rev, "showing takes is view state");
    let h = t.row(a).header;
    t.right_click((h[0] + 60.0, h[1] + 10.0));
    t.menu_item("Show Take Lanes");
    assert_eq!(t.app.ui.takes_open, vec![a]);
    assert_eq!(t.rows_of(a).len(), 3);
    let h = t.row(a).header;
    t.right_click((h[0] + 60.0, h[1] + 10.0));
    assert!(t.app.arrange.menu.iter().any(|m| m.label == "Flatten Takes"), "the track menu flattens too");
    t.menu_item("Hide Take Lanes");
    assert!(t.app.ui.takes_open.is_empty());
    // A track without stacked takes has no toggle and no take items.
    let b = t.track(json!({"kind": "audio", "name": "Plain"}));
    assert_eq!(t.row(b).takes, [0.0; 4]);
    let h = t.row(b).header;
    t.right_click((h[0] + 60.0, h[1] + 10.0));
    assert!(!t.app.arrange.menu.iter().any(|m| m.label.contains("Take")));
}

#[test]
fn a_lane_label_plays_its_take_everywhere_and_delete_take_asks_first() {
    let dir = Dir::new("lanes-label");
    let mut t = T::new();
    let a = t.cycled(&dir);
    let rows = t.rows_of(a);
    t.click(mid(rows[2].label));
    assert_eq!(t.active_take(a), Some(1));
    let rows = t.rows_of(a);
    assert!(!rows[0].active && rows[0].delete_enabled, "take 3 no longer plays");
    assert!(rows[2].active && !rows[2].delete_enabled, "the playing take cannot be deleted");
    let before = t.undo_len();
    t.click(mid(rows[0].menu));
    t.menu_item("Delete Take 3");
    assert_eq!(t.inspect_dialog(), "delete_take", "the click only asks");
    assert_eq!(t.undo_len(), before);
    t.key("Enter", json!({}));
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.engine.session().track(a).unwrap().take_numbers(), vec![2, 1]);
    assert!(t.app.status.contains("Deleted take 3"), "{}", t.app.status);
    assert_eq!(t.undo_len(), before + 1);
}

#[test]
fn flatten_takes_from_the_clip_menu_flattens_that_stack() {
    let dir = Dir::new("lanes-clipmenu");
    let mut t = T::new();
    let a = t.cycled(&dir);
    let playing = t.app.engine.session().track(a).unwrap().clips()[0].id;
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == playing.0).map(|c| c.rect).expect("the playing clip is drawn");
    t.right_click(((r[0] + r[2]) / 2.0, r[1] + 30.0));
    t.menu_item("Flatten Takes");
    let tr = t.app.engine.session().track(a).unwrap();
    assert!(tr.take_stacks().is_empty());
    assert_eq!(tr.clips().iter().map(|c| c.id).collect::<Vec<_>>(), vec![playing], "the take that played stays");
}

/// An audio track with two takes over 0 s..4 s (a take recorded over a clip), lanes not yet shown.
fn two_takes(t: &mut T, dir: &Dir) -> TrackId {
    let a = t.track(json!({"kind": "audio", "name": "Audio 1"}));
    t.record(dir, a, 0, 4.0, 1);
    t.record(dir, a, 0, 4.0, 2);
    let _ = t.app.engine.execute("view.zoom_set", &json!({"samples_per_px": 500.0}));
    t.frames(3);
    a
}

#[test]
fn a_take_over_a_clip_gets_a_chip_that_shows_two_lanes_and_hides_them_again() {
    let dir = Dir::new("chip");
    let mut t = T::new();
    let a = two_takes(&mut t, &dir);
    let chip = t.app.finish.layout.chips.first().cloned().expect("a takes chip on the clip");
    assert_eq!((chip.track, chip.takes, chip.open), (a.0, 2, false));
    assert!(t.app.finish.layout.takes.is_empty(), "collapsed after a single take");
    let rev = t.app.engine.revision;
    t.click(mid(chip.rect));
    assert_eq!(t.app.ui.takes_open, vec![a]);
    assert_eq!(t.app.engine.revision, rev, "showing takes is view state");
    let rows = t.rows_of(a);
    assert_eq!(rows.iter().map(|r| (r.take, r.active)).collect::<Vec<_>>(), vec![(2, true), (1, false)]);
    let h = t.row(a).header;
    assert!(h[3] - h[1] >= t.app.ui.track_height + 2.0 * TAKE_H - 1.0, "{h:?}");
    t.click(mid(t.lane_clip(a, 1).rect));
    assert_eq!(t.active_take(a), Some(1));
    assert!(t.app.status.contains("Take 1 plays on Audio 1"), "{}", t.app.status);
    let chip = t.app.finish.layout.chips.first().cloned().unwrap();
    t.click(mid(chip.rect));
    assert!(t.app.ui.takes_open.is_empty() && t.app.finish.layout.takes.is_empty());
}

#[test]
fn the_take_glyph_keeps_the_lanes_reachable_at_a_narrow_zoom() {
    let dir = Dir::new("narrow");
    let mut t = T::new();
    let a = two_takes(&mut t, &dir);
    // 4 s is 192000 samples: at 4000 samples per pixel the clip is 48 px wide, too narrow for the chip.
    let _ = t.app.engine.execute("view.zoom_set", &json!({"samples_per_px": 4000.0}));
    t.frames(3);
    let chip = t.app.finish.layout.chips.first().cloned().expect("a glyph on the narrow clip");
    assert!(chip.rect[2] - chip.rect[0] <= 12.0, "{:?}", chip.rect);
    assert!(t.app.finish.layout.takes.is_empty());
    t.click(mid(chip.rect));
    assert_eq!(t.app.ui.takes_open, vec![a], "the glyph opens the lanes");
    assert_eq!(t.app.finish.layout.takes.len(), 2);
}

// ---- MIDI takes ----

mod midi {
    use super::*;
    use transit_mix::{Message, NoteId};
    use transit_playback::MidiTake;
    use transit_playback::midi_capture::CapturedEvent;
    use transit_playback::record::start::Point;
    use transit_playback::timeline_map::Position;

    const BEAT: i64 = 24_000;
    const LATCH: u64 = 10_000;

    fn on(frame: u64, track: TrackId, key: u8) -> CapturedEvent {
        CapturedEvent { frame, track, channel: 0, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE }, approximate: false }
    }

    fn off(frame: u64, track: TrackId, key: u8) -> CapturedEvent {
        CapturedEvent { message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE }, ..on(frame, track, key) }
    }

    #[test]
    fn midi_cycle_recording_shows_its_takes_as_lanes_and_a_lane_click_plays_take_1() {
        let mut t = T::new();
        let m = t.track(json!({"kind": "midi", "name": "Keys"}));
        t.app.run("mix.record_arm", json!({"tracks": [m.0], "value": true})).unwrap();
        let _ = t.app.engine.execute("view.zoom_set", &json!({"samples_per_px": 200.0}));
        let lap = 2 * BEAT as u64;
        let events = vec![on(LATCH, m, 60), off(LATCH + 1_000, m, 60), on(LATCH + lap, m, 62), off(LATCH + lap + 1_000, m, 62)];
        let start = Point { host_ns: 0, output_frame: LATCH, position: Position { sample: 0, fraction: 0, denominator: 1 } };
        t.app.record_start = 0;
        t.app.take_plan = Some(transit_engine::io::Recording {
            start: 0,
            pass: Some(2 * BEAT),
            punch: None,
            rate: 48_000,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
        });
        t.app.engine.transport.recording = true;
        t.app.start_play(4 * BEAT, None, None);
        t.app.finished_test_midi = Some(MidiTake { tracks: vec![m], start: Some(start), events, dropped: 0, device_rate: 48_000 });
        t.app.stop_play();
        t.frames(3);
        assert_eq!(t.app.ui.takes_open, vec![m], "MIDI cycle recording shows the lanes too");
        let rows = t.rows_of(m);
        assert_eq!(rows.iter().map(|r| (r.take, r.active)).collect::<Vec<_>>(), vec![(2, true), (1, false)]);
        let pitch = |t: &T| {
            let tr = t.app.engine.session().track(m).unwrap();
            let transit_model::ClipContent::Midi { sequence } = &tr.clips()[0].content else { panic!("not MIDI") };
            sequence.notes[0].pitch
        };
        assert_eq!(pitch(&t), 62, "the newest pass plays");
        t.click(mid(t.lane_clip(m, 1).rect));
        assert_eq!(t.active_take(m), Some(1));
        assert_eq!(pitch(&t), 60, "take 1's notes play now");
        t.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(pitch(&t), 62, "undo restores the newest pass");
    }
}
