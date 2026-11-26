//! A take recorded at an interface rate other than the session's: whether it stacks on the clips
//! under it is decided over its length in session samples, the length its clip has on the
//! timeline (#442).

use super::*;
use transit_time::SampleRate;

/// A mono take file of `frames` frames at `rate`, as a recorder on an interface at `rate` writes.
fn write_take(path: &Path, frames: usize, rate: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = transit_audio_io::wav_float_header(1, rate, frames as u64).unwrap();
    bytes.extend((0..frames).flat_map(|_| 0.25_f32.to_le_bytes()));
    std::fs::write(path, bytes).unwrap();
}

/// A session at `session` Hz with one audio track holding a clip of `len` samples at `at`.
fn track_with_clip(session: u32, at: Samples, len: Samples) -> (Engine, TrackId, transit_model::ClipId) {
    let mut e = Engine::default();
    let s = e.session_mut();
    s.sample_rate = SampleRate::new(session).unwrap();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let len_frames = usize::try_from(len).unwrap();
    let source = add_source(s, "Old", AudioBuffer { sample_rate: session, channels: vec![vec![0.1; len_frames]] }, None, FileFormat::Wav);
    let clip = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(clip, "Old", source, 0, at, len));
    (e, t, clip)
}

/// The issue's case: a 4 s take from an interface at 44.1 kHz in a 48 kHz session, latched at 0,
/// with a clip starting 3.75 s in. The take is 176,400 interface frames but 192,000 session
/// samples, so its clip covers the old clip's start: the old clip is kept whole as take 1 and the
/// new take stacks on it. Measured in interface frames the take seemed to end before the clip, no
/// take stack was started, and the new clip cut the old one away.
#[test]
fn a_take_at_44_1_khz_in_a_48_khz_session_stacks_on_a_clip_its_session_length_reaches() {
    let dir = transit_test_support::TestDir::new("converted-rate-stack");
    let path = dir.join("Audio Files/Vox_02.wav");
    write_take(&path, 176_400, 44_100);
    let (mut e, vox, old) = track_with_clip(48_000, 180_000, 4_800);
    let ids = place_recording_take(&mut e, &Recording { rate: 44_100, ..Recording::new(0) }, vec![(vox, path)], true);
    assert_eq!(ids.len(), 1);
    let t = e.session().track(vox).unwrap();
    let new = t.clips().iter().find(|c| c.id == ids[0]).map(|c| (c.start, c.length));
    assert_eq!(new, Some((0, 192_000)), "the take's clip, in session samples");
    assert_eq!(t.playlists.len(), 2, "a take stack was started");
    assert!(t.playlists.iter().any(|p| p.clip(old).is_some_and(|c| (c.start, c.length) == (180_000, 4_800))), "the old clip is kept whole");
    let stacks = t.take_stacks();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].takes.iter().map(|k| k.number).collect::<Vec<_>>(), vec![1, 2], "the old clip, then the new take");
}

/// Record a take of `frames` frames at `take_rate`, latched at 0, over a clip at `clip_at` in a
/// session at `session` Hz with new takes on, then Undo and Redo.
fn record_over_a_clip_then_undo_and_redo(tag: &str, session: u32, take_rate: u32, frames: usize, clip_at: Samples) {
    let dir = transit_test_support::TestDir::new(tag);
    let path = dir.join("Audio Files/Vox_02.wav");
    write_take(&path, frames, take_rate);
    let (mut e, vox, _) = track_with_clip(session, clip_at, 4_800);
    let before = e.session().clone();
    let ids = place_recording_take(&mut e, &Recording { rate: take_rate, ..Recording::new(0) }, vec![(vox, path)], true);
    assert_eq!(ids.len(), 1);
    assert_eq!(e.session().track(vox).unwrap().playlists.len(), 2, "the take stacks on the clip under it");
    let recorded = e.session().clone();
    assert!(e.undo());
    let s = e.session();
    let names = |s: &transit_model::Session| s.sources.iter().map(|x| (x.id, x.name.clone())).collect::<Vec<_>>();
    assert_eq!(names(s), names(&before), "Undo takes the take's source away");
    assert_eq!(s.pool.ids().collect::<Vec<_>>(), before.pool.ids().collect::<Vec<_>>(), "and its audio");
    assert_eq!(s, &before, "Undo gives back the session from before the recording");
    assert!(e.redo());
    assert_eq!(e.session(), &recorded, "Redo gives the recording back");
}

/// Undo of a take that stacked on a clip leaves no trace of the take, at the session's rate and
/// at a converted one; Redo brings it back.
#[test]
fn undo_of_a_take_stacked_on_a_clip_takes_its_source_away_and_redo_brings_it_back() {
    record_over_a_clip_then_undo_and_redo("undo-stack-equal", 48_000, 48_000, 9_600, 4_800);
    record_over_a_clip_then_undo_and_redo("undo-stack-converted", 48_000, 44_100, 176_400, 180_000);
}

/// The other way round: a 1 s take from an interface at 48 kHz in a 44.1 kHz session ends at
/// 44,100 session samples, before a clip at 46,000, so nothing is under it: no take stack and the
/// clip is untouched. Measured in interface frames (48,000) the take seemed to reach the clip.
#[test]
fn a_take_at_48_khz_in_a_44_1_khz_session_ending_before_a_clip_starts_no_take_stack() {
    let dir = transit_test_support::TestDir::new("converted-rate-clear");
    let path = dir.join("Audio Files/Vox_02.wav");
    write_take(&path, 48_000, 48_000);
    let (mut e, vox, old) = track_with_clip(44_100, 46_000, 4_410);
    let ids = place_recording_take(&mut e, &Recording { rate: 48_000, ..Recording::new(0) }, vec![(vox, path)], true);
    assert_eq!(ids.len(), 1);
    let t = e.session().track(vox).unwrap();
    let new = t.clips().iter().find(|c| c.id == ids[0]).map(|c| (c.start, c.length));
    assert_eq!(new, Some((0, 44_100)), "the take's clip, in session samples");
    assert_eq!(t.playlists.len(), 1, "no take stack");
    let kept = t.clips().iter().find(|c| c.id == old).map(|c| (c.start, c.length, c.take));
    assert_eq!(kept, Some((46_000, 4_410, None)), "the clip after the take is untouched");
}
