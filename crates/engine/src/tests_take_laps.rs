//! The lap cut of cycle recording: an audio take recorded from inside the loop is cut per lap as
//! its MIDI is (#567), and no lap past the thousandth is dropped (#522). Engine level, no devices:
//! the take files are written here and placed as Stop places them.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::Engine;
use crate::io::{Recording, add_recording, place_recording, place_recording_take};
use crate::midi_take::{Cut, LeftOutMidi, Msg, RecEvent, build_wrapping, place_midi_recording, place_midi_recording_reported};
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
    assert_eq!(cut.left_out, 0);
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

/// #522: the MIDI of the laps left out is never lost: it is kept in a MIDI file beside the
/// journal, named in the status note, and holds those laps' notes.
#[test]
fn past_the_most_laps_the_midi_of_the_earlier_laps_is_kept_in_a_file_and_said_issue_522() {
    let dir = TempDir::new("midi-cap");
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    let last = (MAX_LAPS + 1) * 8;
    // A note in each of the two laps left out, and one in the newest lap.
    let mut events = vec![on(1), off(2), on(9), off(10), on(last + 1), off(last + 2)];
    for x in &mut events {
        x.track = t;
    }
    let mut p = plan(0, Some(8), None);
    p.midi_journal = Some(dir.0.join("Audio Files/Song_1.midijournal"));
    let ids = place_midi_recording(&mut e, &p, &events, 0);
    assert_eq!(ids.len(), 1, "the newest lap's note is a take; the earlier laps' are not");
    let note = e.messages.iter().find(|m| m.starts_with(LEFT_OUT_NOTE)).unwrap().clone();
    let kept = dir.0.join("Audio Files").join(format!("{} left-out laps 01.mid", crate::io::sanitize_name(&e.session().name)));
    assert!(note.ends_with(&format!("2 earlier laps were left out; their MIDI is kept in {}.", kept.display())), "{note}");
    let smf = transit_midi::read_smf(&std::fs::read(&kept).unwrap()).unwrap();
    let notes: usize = smf.tracks.iter().map(|tr| tr.sequence.notes.len()).sum();
    assert_eq!(notes, 2, "the left-out laps' notes are in the file");
}

fn on_at(track: TrackId, offset: Samples, pitch: u8) -> RecEvent {
    RecEvent { track, channel: 0, offset, msg: Msg::NoteOn { pitch, velocity: 90 } }
}
fn off_at(track: TrackId, offset: Samples, pitch: u8) -> RecEvent {
    RecEvent { track, channel: 0, offset, msg: Msg::NoteOff { pitch, velocity: 0 } }
}
fn cc_at(track: TrackId, offset: Samples, cc: u8, value: u8) -> RecEvent {
    RecEvent { track, channel: 0, offset, msg: Msg::Cc { cc, value } }
}
fn midi_track(e: &mut Engine) -> TrackId {
    TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap())
}
/// The MIDI clips of a track on every playlist, by start: (start, notes as (pitch, start tick, length)).
fn midi_clips(e: &Engine, t: TrackId) -> Vec<(i64, Vec<(u8, i64, i64)>)> {
    let mut out: Vec<_> = e
        .session()
        .track(t)
        .unwrap()
        .playlists
        .iter()
        .flat_map(|p| p.clips.iter())
        .filter_map(|c| match &c.content {
            transit_model::ClipContent::Midi { sequence } => Some((c.start, sequence.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect())),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// Judge item 3 (2026-11-13): the window of laps kept past the limit is the pass's, not each
/// track's. Two MIDI tracks whose last events fall in different laps and an audio track of the same
/// pass all keep the laps from the same first lap on; the earlier laps of each go to the file.
#[test]
fn every_track_of_a_pass_over_the_limit_keeps_the_same_lap_window() {
    let dir = TempDir::new("one-window");
    let mut e = Engine::default();
    let (a, b, gtr) = (midi_track(&mut e), midi_track(&mut e), audio_track(&mut e));
    let laps = MAX_LAPS + 2;
    // A plays to the last lap; B's last note is in lap 2, the first lap the pass keeps.
    let events = vec![on_at(a, 1, 60), off_at(a, 2, 60), on_at(a, (laps - 1) * 8 + 1, 62), off_at(a, (laps - 1) * 8 + 2, 62)];
    let events = [events, vec![on_at(b, 1, 64), off_at(b, 2, 64), on_at(b, 9, 65), off_at(b, 10, 65), on_at(b, 17, 67), off_at(b, 18, 67)]].concat();
    let mut p = plan(0, Some(8), None);
    p.end = Some(laps * 8);
    p.midi_journal = Some(dir.0.join("Audio Files/Song_1.midijournal"));
    place_midi_recording(&mut e, &p, &events, 0);
    assert_eq!(midi_clips(&e, b).iter().map(|c| c.0).collect::<Vec<_>>(), vec![0], "B keeps only its lap-2 note");
    assert_eq!(midi_clips(&e, b)[0].1.len(), 1);
    assert_eq!(midi_clips(&e, a).len(), 1, "A keeps its last-lap note; its lap-0 note is left out like B's");
    let kept = dir.0.join("Audio Files").join(format!("{} left-out laps 01.mid", crate::io::sanitize_name(&e.session().name)));
    let smf = transit_midi::read_smf(&std::fs::read(&kept).unwrap()).unwrap();
    assert_eq!(smf.tracks.iter().map(|tr| tr.sequence.notes.len()).collect::<Vec<_>>(), vec![1, 2], "A's lap 0, B's laps 0 and 1");
    // The audio take of the same pass keeps the same laps.
    let path = dir.0.join("Audio Files/Gtr_01.wav");
    write_take(&path, usize::try_from(laps * 8).unwrap());
    place_recording_take(&mut e, &p, vec![(gtr, path)], true);
    assert_eq!(clips_of(&e, gtr).first().map(|c| c.0), Some(16), "audio's first kept lap is lap 2 too");
    // Empty MIDI laps make no clip: B made one, though the pass kept 10,000 laps.
}

/// Judge item 1 (2026-11-13): a note, a sustain pedal and a controller across the cutoff. The file
/// holds each note that starts before the cutoff whole (its real note-off) and a held pedal's real
/// release; the first kept lap resumes the held note from its start, as every lap after a wrap does,
/// and holds what changed at or after the cutoff.
#[test]
fn a_note_and_a_pedal_held_across_the_cutoff_lose_nothing() {
    let dir = TempDir::new("held");
    let mut e = Engine::default();
    let t = midi_track(&mut e);
    let lap = 4_800;
    let cutoff = 2 * lap;
    let events = vec![
        on_at(t, lap + 100, 60),
        cc_at(t, lap + 200, 64, 127),
        cc_at(t, lap + 300, 1, 10),
        off_at(t, cutoff + 2_400, 60),
        cc_at(t, cutoff + 2_500, 64, 0),
        cc_at(t, cutoff + 2_600, 1, 90),
    ];
    let mut p = plan(0, Some(lap), None);
    p.end = Some((MAX_LAPS + 2) * lap);
    p.midi_journal = Some(dir.0.join("Song_1.midijournal"));
    let placed = place_midi_recording_reported(&mut e, &p, &events, 0, None);
    let s = e.session();
    let ticks = |at: Samples| s.tempo.samples_to_ticks(at, s.sample_rate);
    // The kept first lap (lap 2): the note resumed from the lap start to its release.
    let clips = midi_clips(&e, t);
    assert_eq!(clips, vec![(0, vec![(60, 0, ticks(2_400))])]);
    let clip = e.session().track(t).unwrap().playlists.iter().flat_map(|p| p.clips.iter()).find(|c| c.id == placed.clips[0]).unwrap().clone();
    let transit_model::ClipContent::Midi { sequence } = &clip.content else { panic!("not MIDI") };
    let kept_ctrls: Vec<_> = sequence.ctrls.iter().map(|c| (c.kind, c.value)).collect();
    assert_eq!(kept_ctrls, vec![(transit_midi::CtrlKind::Cc(64), 0), (transit_midi::CtrlKind::Cc(1), 90)], "what changed at or after the cutoff");
    // The file: the whole note, the pedal down and its real release, the controller before the cutoff.
    let Some(LeftOutMidi::Kept(path)) = placed.left_out else { panic!("not kept: {:?}", placed.left_out) };
    let smf = transit_midi::read_smf(&std::fs::read(&path).unwrap()).unwrap();
    let seq = &smf.tracks[0].sequence;
    let n = seq.notes[0];
    assert_eq!((n.pitch, n.start, n.start + n.length), (60, ticks(lap + 100), ticks(cutoff + 2_400)), "the whole note, not cut at the cutoff");
    let file_ctrls: Vec<_> = seq.ctrls.iter().map(|c| (c.tick, c.kind, c.value)).collect();
    assert_eq!(
        file_ctrls,
        vec![
            (ticks(lap + 200), transit_midi::CtrlKind::Cc(64), 127),
            (ticks(lap + 300), transit_midi::CtrlKind::Cc(1), 10),
            (ticks(cutoff + 2_500), transit_midi::CtrlKind::Cc(64), 0),
        ]
    );
}

/// Judge item 4 (2026-11-13): when the file of the laps left out cannot be written, placement says
/// so with the reason (the host then keeps the journal, see `midi_record_tests`).
#[test]
fn a_file_that_cannot_be_written_is_said_with_the_reason() {
    let dir = TempDir::new("cannot-write");
    std::fs::create_dir_all(&dir.0).unwrap();
    // The journal's "folder" is a file, so no file can go beside it.
    let blocker = dir.0.join("not-a-folder");
    std::fs::write(&blocker, b"x").unwrap();
    let mut e = Engine::default();
    let t = midi_track(&mut e);
    let mut p = plan(0, Some(8), None);
    p.end = Some((MAX_LAPS + 2) * 8);
    p.midi_journal = Some(blocker.join("Song_1.midijournal"));
    let placed = place_midi_recording_reported(&mut e, &p, &[on_at(t, 1, 60), off_at(t, 2, 60)], 0, None);
    assert!(matches!(placed.left_out, Some(LeftOutMidi::Failed(_))), "{:?}", placed.left_out);
    let note = e.messages.iter().find(|m| m.starts_with(LEFT_OUT_NOTE)).unwrap();
    assert!(note.contains("2 earlier laps were left out; their MIDI could not be written to a file ("), "{note}");
}

/// Judge item 4 (2026-11-13): a journal kept because the file could not be written gives the file
/// later, placing no clip.
#[test]
fn a_kept_journal_gives_the_file_of_the_laps_left_out_later_without_placing_clips() {
    use crate::midi_journal::{Header, Latch, Record, Writer, export_left_out};
    let dir = TempDir::new("kept-journal");
    let mut e = Engine::default();
    let t = midi_track(&mut e);
    let path = dir.0.join("Song_1.midijournal");
    std::fs::create_dir_all(&dir.0).unwrap();
    let latch = Latch { output_frame: 0, position: 0, device_rate: 48_000, session_rate: 48_000 };
    let header = Header::new(latch, &plan(0, Some(8), None), None, vec![t]);
    let last = u64::try_from((MAX_LAPS + 1) * 8).unwrap();
    let records: Vec<Record> = [(1, true), (2, false), (last + 1, true), (last + 2, false)]
        .iter()
        .map(|&(frame, down)| Record {
            frame,
            track: t,
            channel: 0,
            approximate: false,
            msg: if down { Msg::NoteOn { pitch: 60, velocity: 90 } } else { Msg::NoteOff { pitch: 60, velocity: 0 } },
        })
        .collect();
    let mut writer = Writer::new(path.clone());
    writer.append(&header, &records, records.len()).unwrap();
    writer.flush().unwrap();
    let undo = e.undo_history().len();
    let got = export_left_out(&mut e, &path).unwrap();
    let Some(LeftOutMidi::Kept(file)) = got else { panic!("{got:?}") };
    assert_eq!(file.parent(), Some(dir.0.path()), "beside the journal");
    let smf = transit_midi::read_smf(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(smf.tracks.iter().map(|tr| tr.sequence.notes.len()).sum::<usize>(), 1);
    assert!(e.session().track(t).unwrap().clips().is_empty() && e.undo_history().len() == undo, "no clip placed");
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
