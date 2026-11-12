//! The lap cut of cycle recording: an audio take recorded from inside the loop is cut per lap as
//! its MIDI is (#567), and no lap past the thousandth is dropped (#522). Engine level, no devices:
//! the take files are written here and placed as Stop places them.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::Engine;
use crate::io::{Recording, add_recording, place_recording, place_recording_take};
use crate::midi_take::{Cut, Msg, RecEvent, build_wrapping, place_midi_recording};
use serde_json::json;
use std::path::{Path, PathBuf};
use transit_model::TrackId;

/// Removes the directory on drop, so a failed assertion leaks nothing.
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("transit-take-laps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
    Recording { start, pass, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false, cycle }
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
    let cut = laps(1_200, None, Some(LOOP), 0, 999).unwrap();
    let got: Vec<_> = cut.laps.iter().map(|l| (l.index, l.window, l.range)).collect();
    assert_eq!(got, vec![(0, Range::new(0, 200), Range::new(1_200, 1_400)), (1, Range::new(200, 600), LOOP), (2, Range::new(600, 1_000), LOOP)]);
    assert_eq!(cut.left_out, 0);
    assert_eq!(laps(1_200, None, Some(LOOP), 0, 199), None, "it never reached the loop end: one straight take");
    assert_eq!(laps(1_400, None, Some(LOOP), 0, 999), None, "started at the loop end: the transport does not wrap");
    assert_eq!(laps(1_200, None, None, 0, 999), None, "no loop: straight through");
    let ahead = laps(800, None, Some(LOOP), 0, 799).unwrap();
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
    assert!(!e.messages.iter().any(|m| m.starts_with(LEFT_OUT_NOTE)), "{:?}", e.messages);
}

/// #522: past the most laps a recording keeps, the newest are the takes and the status says how
/// many earlier laps were left out and that the file still holds them.
#[test]
fn past_the_most_laps_the_newest_are_kept_and_the_earlier_ones_are_said_issue_522() {
    let dir = TempDir::new("cap");
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    let laps = usize::try_from(MAX_LAPS).unwrap() + 2;
    write_take(&path, laps * 8);
    let mut e = Engine::default();
    let gtr = audio_track(&mut e);
    let ids = add_recording(&mut e, 0, vec![(gtr, path.clone())], Some(8));
    assert_eq!(ids.len(), usize::try_from(MAX_LAPS).unwrap());
    let clips = clips_of(&e, gtr);
    assert_eq!((clips.first().map(|c| c.0), clips.last().map(|c| c.0)), (Some(16), Some((MAX_LAPS + 1) * 8)), "the newest laps");
    let note = e.messages.iter().find(|m| m.starts_with(LEFT_OUT_NOTE)).unwrap();
    assert!(note.contains("2 earlier laps were left out") && note.contains(path.to_string_lossy().as_ref()), "{note}");
}

#[test]
fn a_midi_note_after_lap_1000_is_kept_issue_522() {
    let s = transit_model::Session::default();
    let events = [on(1_000 * 48_000 + 10), off(1_000 * 48_000 + 500)];
    let built = crate::midi_take::build(&s.tempo, s.sample_rate, &events, &Cut { start: 96_000, pass: Some(48_000), punch: None, stop: 0 });
    assert_eq!(built.iter().map(|b| (b.pass, b.range)).collect::<Vec<_>>(), vec![(1_000, Range::new(96_000, 144_000))]);
    assert_eq!(built[0].sequence.notes.len(), 1);
}

#[test]
fn past_the_most_laps_the_midi_of_the_earlier_laps_is_said_to_be_left_out_issue_522() {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    let last = (MAX_LAPS + 1) * 8;
    let mut events = vec![on(1), off(2), on(last + 1), off(last + 2)];
    for x in &mut events {
        x.track = t;
    }
    let ids = place_midi_recording(&mut e, &plan(0, Some(8), None), &events, 0);
    assert_eq!(ids.len(), 1, "the newest lap's note is a take; the first lap's is not");
    let note = e.messages.iter().find(|m| m.starts_with(LEFT_OUT_NOTE)).unwrap();
    assert!(note.contains("2 earlier laps were left out with the MIDI played in them"), "{note}");
}
