//! The lap cut of cycle recording: an audio take recorded from inside the loop is cut per lap as
//! its MIDI is (#567), and no lap is ever dropped (#522). Engine level, no devices:
//! the take files are written here and placed as Stop places them.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::Engine;
use crate::io::{Recording, add_recording, place_recording, place_recording_take};
use crate::midi_take::{Cut, Msg, RecEvent, build_wrapping, place_midi_recording_wrapping};
use serde_json::json;
use std::path::Path;
use transit_model::TrackId;

/// A scratch folder (#497), removed on drop so a failed assertion leaks nothing.
struct TempDir(transit_test_support::TestDir);
impl TempDir {
    fn new(tag: &str) -> Self {
        TempDir(transit_test_support::TestDir::new(&format!("take-laps-{tag}")))
    }
}

/// A mono float take of `frames` frames at 48 kHz.
fn write_take(path: &Path, frames: usize) {
    let mut bytes = transit_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
    bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn audio_track(e: &mut Engine) -> TrackId {
    let id = e.execute("track.new", &json!({"name": "Gtr", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    TrackId(id)
}

fn plan(start: Samples, pass: Option<Samples>, cycle: Option<Range>) -> Recording {
    Recording { start, pass, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false, cycle, end: None }
}

/// Every clip of the track on every playlist as (source offset, start, length), by source offset.
fn clips_of(e: &Engine, t: TrackId) -> Vec<(i64, i64, i64)> {
    let mut out: Vec<_> =
        e.session().track(t).unwrap().playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
    out.sort_unstable();
    out
}

fn ev(offset: Samples, msg: Msg) -> RecEvent {
    RecEvent { track: TrackId(1), channel: 0, offset, msg }
}
fn on(offset: Samples) -> RecEvent {
    ev(offset, Msg::NoteOn { pitch: 60, velocity: 90 })
}
fn off(offset: Samples) -> RecEvent {
    ev(offset, Msg::NoteOff { pitch: 60, velocity: 0 })
}

const LOOP: Range = Range { start: 1_000, end: 1_400 };

#[test]
fn a_pass_from_inside_the_loop_is_a_first_stretch_then_whole_laps() {
    let cut = laps(1_200, None, Some(LOOP), 0, 999, 1_000).unwrap();
    let got: Vec<_> = cut.laps.iter().map(|l| (l.index, l.window, l.range)).collect();
    assert_eq!(got, vec![(0, Range::new(0, 200), Range::new(1_200, 1_400)), (1, Range::new(200, 600), LOOP), (2, Range::new(600, 1_000), LOOP)]);
    assert_eq!(cut.merged, 0);
    assert_eq!(laps(1_200, None, Some(LOOP), 0, 199, 200), None, "it never reached the loop end: one straight take");
    assert_eq!(laps(1_400, None, Some(LOOP), 0, 999, 1_000), None, "started at the loop end: the transport does not wrap");
    assert_eq!(laps(1_200, None, None, 0, 999, 1_000), None, "no loop: straight through");
    let ahead = laps(800, None, Some(LOOP), 0, 799, 800).unwrap();
    assert_eq!(ahead.laps[0].range, Range::new(800, 1_400), "started ahead of the loop: the first stretch reaches back");
}

/// #567: Record pressed inside a cycling loop, away from the loop start. The audio take was one
/// straight clip of the whole file running past the loop end; it is now cut as the MIDI is.
#[test]
fn an_audio_take_recorded_from_inside_a_cycling_loop_is_cut_per_lap_like_its_midi_issue_567() {
    let dir = TempDir::new("inside");
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    write_take(&path, 1_000);
    let mut e = Engine::default();
    let gtr = audio_track(&mut e);
    let ids = place_recording_take(&mut e, &plan(1_200, None, Some(LOOP)), vec![(gtr, path.clone())], true);
    assert_eq!(ids.len(), 3, "a take per lap");
    assert_eq!(clips_of(&e, gtr), vec![(0, 1_200, 200), (200, 1_000, 400), (600, 1_000, 400)]);
    let t = e.session().track(gtr).unwrap();
    assert_eq!(t.playlists.len(), 3, "each later lap on its own playlist");
    assert_eq!(t.active_playlist, 0, "the track plays the playlist it played before");
    assert!(t.clips().iter().any(|c| c.id == ids[2]), "the newest lap plays");
    assert_eq!(t.take_stacks()[0].active(), Some(3));
    assert_eq!(e.undo_history().last().map(String::as_str), Some("Record"));

    // The MIDI of the same pass: the same clip ranges.
    let events = [on(50), off(60), on(300), off(310), on(700), off(710)];
    let (tempo, sr) = (e.session().tempo.clone(), e.session().sample_rate);
    let midi = build_wrapping(&tempo, sr, &events, &Cut { start: 1_200, pass: None, punch: None, stop: 1_000 }, Some(LOOP));
    let midi: Vec<Range> = midi.iter().map(|b| b.range).collect();
    let t = e.session().track(gtr).unwrap();
    // An earlier lap's take is on a playlist the track does not play.
    let audio: Vec<Range> = ids.iter().map(|id| t.playlists.iter().find_map(|p| p.clip(*id)).unwrap().range()).collect();
    assert_eq!(midi, audio);

    // Recovery places the plan it read back from its file the same way.
    let read: Recording = serde_json::from_str(&serde_json::to_string(&plan(1_200, None, Some(LOOP))).unwrap()).unwrap();
    assert_eq!(read.cycle, Some(LOOP));
    let mut r = Engine::default();
    let gtr = audio_track(&mut r);
    assert_eq!(place_recording(&mut r, &read, vec![(gtr, path)]).len(), 3);
    assert_eq!(clips_of(&r, gtr), vec![(0, 1_200, 200), (200, 1_000, 400), (600, 1_000, 400)]);
}

#[test]
fn a_plan_written_before_the_loop_was_kept_reads_back_as_straight() {
    let old = r#"{"start":1200,"pass":null,"punch":null,"rate":48000,"takes":[]}"#;
    let read: Recording = serde_json::from_str(old).unwrap();
    assert_eq!(read, plan(1_200, None, None));
    assert!(!serde_json::to_string(&read).unwrap().contains("cycle"), "nothing new written for a straight take");
}

/// #522: the 1,000-pass cap left the frames after pass 1,000 without a clip.
#[test]
fn an_audio_take_past_lap_1000_is_placed_to_its_last_frame_issue_522() {
    let dir = TempDir::new("past-1000");
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    write_take(&path, 100_150);
    let mut e = Engine::default();
    let gtr = audio_track(&mut e);
    let ids = add_recording(&mut e, 0, vec![(gtr, path)], Some(100));
    assert_eq!(ids.len(), 1_002);
    let clips = clips_of(&e, gtr);
    assert_eq!(clips.iter().map(|c| c.2).sum::<i64>(), 100_150, "every frame is in a take");
    assert_eq!(clips.last(), Some(&(100_100, 0, 50)));
    assert!(!e.messages.iter().any(|m| m.starts_with(OVER_LIMIT_NOTE)), "{:?}", e.messages);
}

#[test]
fn a_midi_note_after_lap_1000_is_kept_issue_522() {
    let s = transit_model::Session::default();
    let events = [on(1_000 * 48_000 + 10), off(1_000 * 48_000 + 500)];
    let built = crate::midi_take::build(&s.tempo, s.sample_rate, &events, &Cut { start: 96_000, pass: Some(48_000), punch: None, stop: 0 });
    assert_eq!(built.iter().map(|b| (b.pass, b.range)).collect::<Vec<_>>(), vec![(1_000, Range::new(96_000, 144_000))]);
    assert_eq!(built[0].sequence.notes.len(), 1);
}

fn on_at(track: TrackId, offset: Samples, pitch: u8) -> RecEvent {
    RecEvent { track, channel: 0, offset, msg: Msg::NoteOn { pitch, velocity: 90 } }
}
fn off_at(track: TrackId, offset: Samples, pitch: u8) -> RecEvent {
    RecEvent { track, channel: 0, offset, msg: Msg::NoteOff { pitch, velocity: 0 } }
}
fn midi_track(e: &mut Engine) -> TrackId {
    TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap())
}

/// #634: a last lap shorter than an eighth of the loop is still no take (a product choice), but it
/// is said: how many frames and how long, and the file that keeps them.
#[test]
fn a_last_lap_too_short_to_be_a_take_is_said_issue_634() {
    let dir = TempDir::new("short-lap");
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    write_take(&path, 1_220);
    let mut e = Engine::default();
    let gtr = audio_track(&mut e);
    assert_eq!(add_recording(&mut e, 0, vec![(gtr, path.clone())], Some(400)).len(), 3);
    let note = e.messages.iter().find(|m| m.starts_with(SHORT_LAP_NOTE)).unwrap();
    assert_eq!(note, &short_lap_note(20, 20.0 / 48_000.0, &path.display().to_string()));
    assert!(note.contains("the last 20 frames (0.4 ms) stay in"), "{note}");
}

/// Review M1 (2026-11-13): MIDI played only before the first wrap. Loop [1000, 1400), Record at
/// 1200, one note at offsets 50 to 60, two wraps, Stop at offset 1000. The pass wrapped, so the
/// MIDI is cut as the audio of the same pass is: its one note is a clip over [1200, 1400), and the
/// empty later laps make no clip. It used to be one clip over [1200, 2200), past the loop end.
#[test]
fn midi_played_only_before_the_first_wrap_is_cut_at_the_loop_end_like_its_audio() {
    let dir = TempDir::new("before-wrap");
    let mut e = Engine::default();
    let t = midi_track(&mut e);
    let gtr = audio_track(&mut e);
    let p = plan(1_200, None, Some(LOOP));
    let ids = place_midi_recording_wrapping(&mut e, &p, &[on_at(t, 50, 60), off_at(t, 60, 60)], 1_000, Some(LOOP));
    let ranges: Vec<Range> = ids.iter().map(|id| e.session().track(t).unwrap().playlists.iter().find_map(|p| p.clip(*id)).unwrap().range()).collect();
    assert_eq!(ranges, vec![Range::new(1_200, 1_400)]);
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    write_take(&path, 1_000);
    let audio = place_recording_take(&mut e, &p, vec![(gtr, path)], true);
    let first = e.session().track(gtr).unwrap().playlists.iter().find_map(|p| p.clip(audio[0])).unwrap().range();
    assert_eq!(first, ranges[0], "the MIDI clip is the audio's first lap");
}

/// #522: a pass handed to the cut with more laps than the limit (the host could not stop it in
/// time, or crash recovery of a pass that ran on) loses nothing: its first `MAX_LAPS` laps are
/// takes, everything after them one longer final take, said once. Audio and MIDI of the same pass
/// get the same final take.
#[test]
fn past_the_lap_limit_the_last_laps_are_one_final_take_and_nothing_is_dropped_issue_522() {
    let dir = TempDir::new("over-limit");
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    let laps = MAX_LAPS + 3;
    write_take(&path, usize::try_from(laps * 8).unwrap());
    let mut e = Engine::default();
    let (gtr, t) = (audio_track(&mut e), midi_track(&mut e));
    let mut p = plan(0, Some(8), None);
    p.end = Some(laps * 8);
    let audio = place_recording_take(&mut e, &p, vec![(gtr, path)], true);
    assert_eq!(audio.len(), usize::try_from(MAX_LAPS + 1).unwrap());
    let clips = clips_of(&e, gtr);
    assert_eq!(clips.iter().map(|c| c.2).sum::<i64>(), laps * 8, "every frame is in a take");
    assert_eq!(clips.last(), Some(&(MAX_LAPS * 8, 0, 24)), "the last three laps are one final take");
    let said: Vec<&String> = e.messages.iter().filter(|m| m.starts_with(OVER_LIMIT_NOTE)).collect();
    assert_eq!(said, vec![&over_limit_note(3)]);
    assert_eq!(said[0], "Cycle recording ran past its limit of 10000 laps: its last 3 laps are one longer final take.");
    // The MIDI of the same pass: a note in the first lap and one in the very last.
    let events = [on_at(t, 1, 60), off_at(t, 2, 60), on_at(t, (laps - 1) * 8 + 1, 62), off_at(t, (laps - 1) * 8 + 2, 62)];
    let ids = place_midi_recording_wrapping(&mut e, &p, &events, 0, None);
    assert_eq!(ids.len(), 2, "empty laps make no clip");
    let tr = e.session().track(t).unwrap();
    let last = tr.playlists.iter().find_map(|pl| pl.clip(ids[1])).unwrap();
    let audio_last = e.session().track(gtr).unwrap().playlists.iter().find_map(|pl| pl.clip(*audio.last().unwrap())).unwrap();
    assert_eq!(last.range(), audio_last.range(), "the same final take as the audio");
    let transit_model::ClipContent::Midi { sequence } = &last.content else { panic!("not MIDI") };
    assert_eq!(sequence.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![62], "the last lap's note is kept");
}

/// Judging round 2, item 1: the first MIDI take of a pass is heard even when the pass's first lap
/// held nothing. It used to go on a new playlist the track does not play, alone and silent.
#[test]
fn the_first_midi_take_of_a_pass_whose_first_lap_is_empty_plays() {
    // Record pressed inside the loop, notes only after the first wrap.
    let mut e = Engine::default();
    let t = midi_track(&mut e);
    let ids = place_midi_recording_wrapping(&mut e, &plan(1_200, None, Some(LOOP)), &[on_at(t, 300, 60), off_at(t, 310, 60)], 1_000, Some(LOOP));
    assert_eq!(ids.len(), 1);
    let playing = e.session().track(t).unwrap().clips().iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(playing, ids, "on the playlist the track plays");
    // Loop recording from the loop start, notes only in the third lap.
    let mut e = Engine::default();
    let t = midi_track(&mut e);
    let ids = place_midi_recording_wrapping(&mut e, &plan(1_000, Some(400), None), &[on_at(t, 850, 60), off_at(t, 860, 60)], 1_000, None);
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.clips().iter().map(|c| (c.id, c.start, c.length)).collect::<Vec<_>>(), vec![(ids[0], 1_000, 400)]);
    let transit_model::ClipContent::Midi { sequence } = &tr.clips()[0].content else { panic!("not MIDI") };
    assert_eq!(sequence.notes.len(), 1, "the playing take holds the note");
}

/// #522: completed laps come from how far the pass has run, never from where the playhead is: the
/// judge's case of a 0.8 s loop at 48 kHz, a two-second stall just after 9,999 wraps, laps that
/// end on the same position, and a start halfway into the loop.
#[test]
fn completed_laps_count_every_wrap_from_the_pass_clock() {
    let lap = 38_400;
    let looped = Range::new(96_000, 96_000 + lap);
    // From the loop start, in laps of the loop.
    let after_9999 = 9_999 * lap;
    assert_eq!(completed(96_000, Some(lap), None, after_9999), 9_999);
    assert_eq!(completed(96_000, Some(lap), None, after_9999 + 96_000), 10_001, "the stall's two and a half laps all count");
    assert_eq!(completed(96_000, Some(lap), None, 5 * lap + 100) - completed(96_000, Some(lap), None, 2 * lap + 100), 3, "same position, three laps");
    // Halfway into the loop: the first lap is the half to the loop end.
    let start = 96_000 + lap / 2;
    assert_eq!(completed(start, None, Some(looped), lap / 2 - 1), 0);
    assert_eq!(completed(start, None, Some(looped), lap / 2), 1, "the first lap ends at the first wrap");
    assert_eq!(completed(start, None, Some(looped), lap / 2 + 9_999 * lap), MAX_LAPS);
    assert_eq!(completed(start, None, Some(looped), lap / 2 + 9_999 * lap - 1), MAX_LAPS - 1);
    // It agrees with the cut at every lap end.
    let cut = laps(start, None, Some(looped), 0, 5 * lap - 1, 5 * lap).unwrap();
    for l in &cut.laps {
        assert_eq!(completed(start, None, Some(looped), l.window.end), l.index + 1, "lap {} is complete at its window end", l.index);
    }
    assert_eq!(completed(1_200, None, None, 10_000), 0, "no loop: no laps");
}
