use super::*;

fn write_take(path: &Path, frames: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = transit_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
    bytes.extend((0..frames).flat_map(|_| 0.25_f32.to_le_bytes()));
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn save_as_moves_the_unplaced_reference_out_of_autosave() {
    let dir = transit_test_support::TestDir::created("unplaced-save-as");
    let autosave = dir.join("Autosave");
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    e.session_mut().track_mut(t).unwrap().mixer.record_arm = true;
    let takes = plan_takes(&e, &autosave, 1);
    let path = takes[0].2.clone();
    write_take(&path, 480);
    assert!(place_recording(&mut e, &Recording::new(-960), vec![(t, path)]).is_empty());
    let saved = dir.join("Saved/Song.transit");
    save_session(&mut e, saved.to_str().unwrap()).unwrap();
    let source = &e.session().sources[0];
    let kept = saved.parent().unwrap().join(&source.path);
    assert!(kept.is_file());
    std::fs::remove_dir_all(&autosave).unwrap();
    let entry = e.session().unplaced_takes[0].to_string();
    assert_eq!(entry, format!("{} (recorded before bar 1)", source.path));
    assert!(kept.is_file(), "the listed file survives removal of autosave");
    let mut reopened = Engine::default();
    open_session(&mut reopened, saved.to_str().unwrap()).unwrap();
    assert_eq!(reopened.session().unplaced_takes, e.session().unplaced_takes);
    assert_eq!(reopened.session().pool.get(source.id).unwrap().frames(), 480);
}

#[test]
fn undo_after_save_as_keeps_the_unplaced_file_reachable() {
    let dir = transit_test_support::TestDir::created("unplaced-save-history");
    let path = dir.join("Autosave/Audio Files/Vox.wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    place_recording(&mut e, &Recording::new(-960), vec![(t, path.clone())]);
    e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
    save_session(&mut e, dir.join("Saved/Song.transit").to_str().unwrap()).unwrap();
    std::fs::remove_dir_all(dir.join("Autosave")).unwrap();
    assert!(e.undo());
    let transit_model::UnplacedTake::File { path, source, .. } = &e.session().unplaced_takes[0] else { panic!("structured entry required") };
    assert!(dir.join("Saved").join(path).is_file(), "Undo must not restore the deleted autosave path");
    assert_eq!(&e.session().source(source.unwrap()).unwrap().path, path);
    assert!(e.undo());
    assert!(e.session().sources.is_empty() && e.session().unplaced_takes.is_empty());
    assert!(e.redo());
    assert!(e.redo());
    assert!(dir.join("Saved").join(&e.session().sources[0].path).is_file());
}

#[test]
fn save_as_follows_every_entry_kind_and_reads_legacy_sessions() {
    use transit_model::UnplacedTake;
    let dir = transit_test_support::TestDir::created("unplaced-kinds");
    let path = dir.join("Autosave/Audio Files/Vox (first)_01.wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    place_recording(&mut e, &Recording::new(-960), vec![(t, path.clone())]);
    let id = e.session().sources[0].id;
    for reason in ["last lap too short", "track deleted", "playlist could not be copied", "could not decode (damaged header)"] {
        e.session_mut().unplaced_takes.push(UnplacedTake::new(path.display().to_string(), reason.into(), Some(id), false));
    }
    e.session_mut().unplaced_takes.push(UnplacedTake::new(path.display().to_string(), "refused".into(), None, true));
    e.session_mut().unplaced_takes.push(format!("{} (recorded before bar 1)", path.display()).into());
    let text = e.session().to_json().unwrap();
    let loaded = Session::from_json(&text).unwrap();
    assert_eq!(loaded.unplaced_takes, e.session().unplaced_takes);
    save_session(&mut e, dir.join("Saved/Song.transit").to_str().unwrap()).unwrap();
    for entry in &e.session().unplaced_takes {
        let UnplacedTake::File { path, source, .. } = entry else { panic!("legacy entry was not migrated") };
        assert_eq!(path, &e.session().sources[0].path);
        assert_eq!(*source, Some(id));
        assert!(dir.join("Saved").join(path).is_file());
    }
}

#[test]
fn an_empty_take_has_no_source_listing_or_record_step() {
    let dir = transit_test_support::TestDir::created("empty-take");
    let path = dir.join("Empty.wav");
    write_take(&path, 0);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let before = e.session_arc();
    assert!(place_recording(&mut e, &Recording::new(0), vec![(t, path)]).is_empty());
    assert_eq!(e.undo_label(), None, "empty audio is not a Record step");
    assert_eq!(e.session(), before.as_ref());
    assert!(e.messages.iter().any(|m| m.contains("empty")));
}

fn assert_unplaced_undo(e: &mut Engine, before: &Session, path: &Path, reason: &str) {
    let listed = e.session().unplaced_takes.clone();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].to_string().contains(reason), "{listed:?}");
    assert!(e.messages.iter().any(|m| m.contains(reason) && m.contains(path.to_str().unwrap())), "{:?}", e.messages);
    assert_eq!(e.session().sources.len(), before.sources.len() + 1);
    assert_eq!(e.undo_label(), Some("Record"));
    let file = std::fs::read(path).unwrap();
    assert!(e.undo());
    assert_eq!(e.session(), before);
    assert!(e.redo());
    assert_eq!(e.session().unplaced_takes, listed);
    assert_eq!(e.session().sources.len(), before.sources.len() + 1);
    assert_eq!(std::fs::read(path).unwrap(), file);
}

#[test]
fn a_deleted_track_leaves_its_take_listed_with_its_source() {
    let dir = transit_test_support::TestDir::created("deleted-track-take");
    let path = dir.join("Vox.wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let plan = Recording::new(0);
    e.session_mut().tracks.clear();
    let before = e.session_arc();
    assert!(place_recording(&mut e, &plan, vec![(t, path.clone())]).is_empty());
    assert_unplaced_undo(&mut e, &before, &path, "track was deleted");
}

#[test]
fn a_failed_playlist_copy_lists_the_take_and_undoes_its_source() {
    let dir = transit_test_support::TestDir::created("playlist-unplaced");
    let path = dir.join("New.wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let source = add_source(e.session_mut(), "Original", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 480]] }, None, FileFormat::Wav);
    let clip = e.session_mut().new_clip_id();
    crate::edit::place_clip(e.session_mut(), t, Clip::audio(clip, "Original", source, 0, 0, 480));
    let track = e.session_mut().track_mut(t).unwrap();
    track.freeze = Some(transit_model::Frozen { source, start: 0, length: 480, hash: track.freeze_hash(), revision: 1, latency: 0 });
    let before = e.session_arc();
    assert!(place_recording_take(&mut e, &Recording::new(0), vec![(t, path.clone())], true).is_empty());
    assert_eq!(e.session().tracks, before.tracks);
    assert_unplaced_undo(&mut e, &before, &path, "playlist could not be copied");
}

#[test]
fn an_undecodable_nonempty_take_stays_listed_across_history() {
    let dir = transit_test_support::TestDir::created("damaged-take");
    let path = dir.join("Broken.wav");
    write_take(&path, 480);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..4].copy_from_slice(b"oops");
    std::fs::write(&path, &bytes).unwrap();
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
    assert!(place_recording(&mut e, &Recording::new(0), vec![(t, path.clone())]).is_empty());
    let listed = e.session().unplaced_takes.clone();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].to_string().contains("could not be decoded"));
    assert!(e.session().sources.is_empty());
    assert!(e.undo());
    assert_eq!(e.session().unplaced_takes, listed);
    assert!(e.redo());
    assert_eq!(e.session().unplaced_takes, listed);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_short_first_lap_is_kept_and_a_short_last_lap_is_reported() {
    let dir = transit_test_support::TestDir::created("short-only-take");
    for (start, frames, clips, listed) in [(0, 10, 1, 0), (-900, 810, 0, 1)] {
        let path = dir.join(format!("Vox_{frames}.wav"));
        write_take(&path, frames);
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert_eq!(add_recording(&mut e, start, vec![(t, path)], Some(800)).len(), clips);
        assert_eq!(e.session().unplaced_takes.len(), listed);
        assert_eq!(e.undo_label(), Some("Record"));
        if frames > 800 {
            assert_eq!(e.short_laps(), 1);
        }
        assert!(e.undo());
        assert!(e.session().sources.is_empty() && e.session().unplaced_takes.is_empty());
    }
}

#[test]
fn an_unreadable_take_keeps_a_reference_without_inventing_a_source() {
    let dir = transit_test_support::TestDir::created("unreadable-take");
    let path = dir.join("Take.wav");
    write_take(&path, 480);
    let allowed = dir.join("Allowed");
    std::fs::create_dir(&allowed).unwrap();
    let roots = crate::file_access::AutomationRoots::new(Some(&allowed), None).unwrap().unwrap();
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    crate::file_access::confine(Some(&roots), || {
        assert!(place_recording(&mut e, &Recording::new(0), vec![(t, path.clone())]).is_empty());
    });
    assert!(path.is_file());
    assert_eq!(e.session().unplaced_takes.len(), 1);
    assert!(e.session().unplaced_takes[0].to_string().contains("could not be read"));
    assert!(e.session().sources.is_empty());
    assert_eq!(e.undo_label(), None);
    assert!(e.messages.iter().any(|m| m.contains(path.to_str().unwrap())));
}

#[test]
fn an_early_take_and_a_later_edit_follow_two_undos_and_redos() {
    let dir = transit_test_support::TestDir::created("early-history");
    let path = dir.join("Early.wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let before = e.session_arc();
    place_recording(&mut e, &Recording::new(-960), vec![(t, path.clone())]);
    let recorded = e.session_arc();
    e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
    let edited = e.session_arc();
    assert!(e.undo());
    assert_eq!(e.session(), recorded.as_ref());
    assert!(e.undo());
    assert_eq!(e.session(), before.as_ref());
    assert!(e.redo());
    assert_eq!(e.session(), recorded.as_ref());
    assert!(e.redo());
    assert_eq!(e.session(), edited.as_ref());
    assert!(path.is_file());
}

#[test]
fn a_refused_take_after_an_early_take_survives_its_undo_and_redo() {
    let dir = transit_test_support::TestDir::created("early-then-refused");
    let path = dir.join("Early.wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    place_recording(&mut e, &Recording::new(-960), vec![(t, path)]);
    let early = e.session().unplaced_takes[0].clone();
    let long = dir.join("Long.wav");
    let frames = transit_audio_io::MAX_DECODE_SAMPLES + 1_000;
    let bytes = transit_audio_io::wav_float_header(1, 44_100, frames).unwrap();
    std::fs::write(&long, &bytes).unwrap();
    let _guard = super::record_tests::FakeLenGuard;
    super::super::disk::FAKE_LEN.with(|l| l.set(Some(bytes.len() as u64 + frames * 4)));
    place_recording(&mut e, &Recording::new(0), vec![(t, long.clone())]);
    let refused = e.session().unplaced_takes[1].clone();
    assert!(e.undo());
    assert_eq!(e.session().unplaced_takes, std::slice::from_ref(&refused));
    assert!(e.session().sources.is_empty());
    assert!(e.redo());
    assert_eq!(e.session().unplaced_takes, [early, refused]);
    assert_eq!(e.session().sources.len(), 1);
    assert!(long.is_file());
}

#[test]
fn crash_recovery_keeps_a_finished_or_interrupted_early_take_listed() {
    use crate::autosave::{Autosaver, leftovers, recover};
    let dir = transit_test_support::TestDir::created("early-recovery");
    for finished in [false, true] {
        let root = dir.join(if finished { "finished" } else { "interrupted" });
        let path = root.join("Untitled/Audio Files/Vox_01.wav");
        write_take(&path, 480);
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut plan = Recording::new(-960);
        plan.rate = 48_000;
        plan.takes.push(Take { track: t, path: path.clone(), channels: 1 });
        if finished {
            place_recording(&mut e, &plan, vec![(t, path.clone())]);
        }
        let writer = Autosaver::new(root.clone());
        writer.write(e.session_arc(), None, (!finished).then_some(plan));
        writer.flush().unwrap();
        drop(writer);
        let mut recovered = Engine::default();
        assert!(recover(&mut recovered, &leftovers(&root)[0]).unwrap().is_empty());
        let s = recovered.session();
        assert_eq!(s.unplaced_takes.len(), 1);
        assert_eq!(s.unplaced_takes[0].to_string(), format!("{} (recorded before bar 1)", path.display()));
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.pool.get(s.sources[0].id).unwrap().frames(), 480);
        assert_eq!(s.all_clips().count(), 0);
        assert!(path.is_file());
    }
}

#[test]
fn one_early_lap_after_the_first_uses_the_lap_note() {
    let dir = transit_test_support::TestDir::created("one-early-lap");
    let path = dir.join("Vox.wav");
    write_take(&path, 30_000);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let plan = Recording { cycle: Some(Range::new(-12_000, 12_000)), ..Recording::new(6_000) };
    assert_eq!(place_recording(&mut e, &plan, vec![(t, path.clone())]).len(), 2);
    assert_eq!(
        e.messages.last(),
        Some(&format!("Audio recorded before bar 1 is not in the clip (audio clips start at bar 1): 0.25 s from 1 lap stay in {}.", path.display()))
    );
}

#[test]
fn one_early_lap_at_the_start_uses_the_lap_note() {
    let dir = transit_test_support::TestDir::created("first-early-lap");
    let path = dir.join("Vox.wav");
    write_take(&path, 36_000);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let plan = Recording { cycle: Some(Range::new(0, 12_000)), ..Recording::new(-12_000) };
    assert_eq!(place_recording(&mut e, &plan, vec![(t, path.clone())]).len(), 2);
    assert_eq!(
        e.messages.last(),
        Some(&format!("Audio recorded before bar 1 is not in the clip (audio clips start at bar 1): 0.25 s from 1 lap stay in {}.", path.display()))
    );
}

#[test]
fn save_as_matches_a_legacy_absolute_entry_to_a_relative_source() {
    let dir = transit_test_support::TestDir::created("legacy-relative-source");
    let original = dir.join("Original");
    let path = original.join("Audio Files").join("Vox (first).wav");
    write_take(&path, 480);
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    place_recording(&mut e, &Recording::new(-960), vec![(t, path.clone())]);
    // The previous format saved relative sources but left absolute text entries unchanged.
    e.session_mut().sources[0].path = "Audio Files/Vox (first).wav".into();
    e.session_mut().unplaced_takes = vec![format!("{} (recorded before bar 1)", path.display()).into()];
    std::fs::write(original.join("Old.transit"), e.session().to_json().unwrap()).unwrap();
    let mut reopened = Engine::default();
    open_session(&mut reopened, original.join("Old.transit").to_str().unwrap()).unwrap();
    assert_eq!(reopened.session().unplaced_takes.len(), 1);
    assert_eq!(reopened.session().all_clips().count(), 0);
    reopened.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
    save_session(&mut reopened, dir.join("Saved/New.transit").to_str().unwrap()).unwrap();
    std::fs::remove_dir_all(&original).unwrap();
    let s = reopened.session();
    assert_eq!(s.unplaced_takes[0].to_string(), format!("{} (recorded before bar 1)", s.sources[0].path));
    assert!(dir.join("Saved").join(&s.sources[0].path).is_file());
    assert!(reopened.undo());
    let transit_model::UnplacedTake::File { path, source, .. } = &reopened.session().unplaced_takes[0] else { panic!("legacy entry not migrated") };
    assert!(Path::new(path).is_file());
    assert_eq!(&reopened.session().source(source.unwrap()).unwrap().path, path);
}

#[test]
fn a_legacy_entry_does_not_follow_a_filename_prefix() {
    let text = "Audio Files/Vox.wav (copy).wav (recorded before bar 1)";
    let mut entry = transit_model::UnplacedTake::from(text);
    assert!(!entry.follow_source(SourceId(1), "Audio Files/Vox.wav", "Audio Files/Wrong.wav"));
    assert_eq!(entry.to_string(), text);
    assert!(entry.follow_source(SourceId(2), "Audio Files/Vox.wav (copy).wav", "Audio Files/Right.wav"));
    assert_eq!(entry.to_string(), "Audio Files/Right.wav (recorded before bar 1)");
    let mut nested = transit_model::UnplacedTake::from("Audio Files/Vox.wav (could not decode (bad header))");
    assert!(nested.follow_source(SourceId(1), "Audio Files/Vox.wav", "Audio Files/Kept.wav"));
    assert_eq!(nested.to_string(), "Audio Files/Kept.wav (could not decode (bad header))");
}
