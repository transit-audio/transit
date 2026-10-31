//! Take stacks as edits and commands (#25): choose the take that plays, flatten, delete, undo,
//! the session round trip and what the mix renders.

use super::*;
use crate::Engine;
use serde_json::json;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_model::{ChannelFormat, Clip, Playlist, SourceId, TrackKind};

/// An audio track with three one-second takes over 1 s..2 s, on three playlists, each a
/// different constant level (take n plays n / 10): Take 3 plays. Also a clip at 5 s that only
/// the active playlist holds. (engine, track, [take 1, take 2, take 3] clip ids, the 5 s clip).
fn three_takes() -> (Engine, TrackId, [ClipId; 3], ClipId) {
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let mut ids = [ClipId(0); 3];
    let mut srcs: Vec<SourceId> = Vec::new();
    for n in 1..=3u32 {
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![n as f32 / 10.0; 48_000]] };
        srcs.push(crate::io::add_source(s, &format!("Vox_{n:02}"), buf, None, FileFormat::Wav));
    }
    let other_src = crate::io::add_source(s, "Other", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] }, None, FileFormat::Wav);
    let other = s.new_clip_id();
    let mut lists: Vec<Vec<Clip>> = vec![Vec::new(), Vec::new(), Vec::new()];
    for (k, n) in [(1usize, 1u32), (2, 2), (0, 3)] {
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, format!("Vox_{n:02}"), srcs[n as usize - 1], 0, 48_000, 48_000);
        c.take = Some(n);
        lists[k].push(c);
        ids[n as usize - 1] = id;
    }
    lists[0].push(Clip::audio(other, "Other", other_src, 0, 240_000, 48_000));
    let tr = s.track_mut(t).unwrap();
    tr.playlists = lists.into_iter().enumerate().map(|(i, clips)| Playlist { name: format!("Vox.{:02}", i + 1), clips }).collect();
    tr.playlists.iter_mut().for_each(Playlist::sort);
    tr.active_playlist = 0;
    (e, t, ids, other)
}

/// The level the track renders at `secs` (pre-fader, left channel).
fn raw_level(e: &Engine, t: TrackId, secs: f64) -> f32 {
    let at = e.session().sample_rate.samples(secs);
    let out = transit_mix::render_track_pre_fader(e.session(), t, Range::new(at, at + 64));
    out[0][32]
}

/// The source level playing at `secs`, scaled by the 0.5 clip at 5 s (whatever the pan law does
/// to a mono track, it does to both).
fn level_at(e: &Engine, t: TrackId, secs: f64) -> f32 {
    raw_level(e, t, secs) / raw_level(e, t, 5.5) * 0.5
}

fn active_take(e: &Engine, t: TrackId) -> Option<u32> {
    e.session().track(t).unwrap().take_stacks()[0].active()
}

#[test]
fn the_mix_renders_only_the_active_take_and_choosing_a_take_changes_what_renders() {
    let (mut e, t, ids, _) = three_takes();
    assert!((level_at(&e, t, 1.5) - 0.3).abs() < 1e-4, "take 3 plays, takes 1 and 2 are silent");
    assert!(raw_level(&e, t, 5.5) > 0.1);
    e.execute("takes.set_active", &json!({"track": t.0, "clip": ids[1].0})).unwrap();
    assert_eq!(active_take(&e, t), Some(2));
    assert!((level_at(&e, t, 1.5) - 0.2).abs() < 1e-4, "take 2 renders now");
    assert!(raw_level(&e, t, 5.5) > 0.1, "the rest of the track keeps playing");
}

#[test]
fn choosing_a_take_is_one_undo_step_and_keeps_every_clip_and_number() {
    let (mut e, t, ids, other) = three_takes();
    let before = e.session().clone();
    let undo = e.undo_history().len();
    let r = e.execute("takes.set_active", &json!({"track": t.0, "take": 1})).unwrap();
    assert_eq!(r["moved"], 1);
    assert_eq!(e.undo_history().len(), undo + 1);
    assert_eq!(e.undo_history().last().map(String::as_str), Some("Choose Take"));
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.active_playlist, 0, "the active playlist stays; its content changes");
    let playing: Vec<ClipId> = tr.clips().iter().map(|c| c.id).collect();
    assert_eq!(playing, vec![ids[0], other]);
    let all: usize = tr.playlists.iter().map(|p| p.clips.len()).sum();
    assert_eq!(all, 4, "nothing copied, nothing lost");
    assert_eq!(tr.take_numbers(), vec![3, 2, 1], "the lanes stay where they were");
    // Already playing: no change, no step.
    e.execute("takes.set_active", &json!({"track": t.0, "take": 1})).unwrap();
    assert_eq!(e.undo_history().len(), undo + 1);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session(), &before);
}

#[test]
fn a_shorter_take_plays_over_its_range_and_the_longer_one_around_it() {
    let (mut e, t, ids, _) = three_takes();
    // Take 1 becomes a quarter second at 1.25 s.
    let s = e.session_mut();
    let c = s.track_mut(t).unwrap().playlists[1].clip_mut(ids[0]).unwrap();
    c.start = 60_000;
    c.length = 12_000;
    e.execute("takes.set_active", &json!({"track": t.0, "clip": ids[0].0})).unwrap();
    assert!((level_at(&e, t, 1.1) - 0.3).abs() < 1e-4, "take 3 before it");
    assert!((level_at(&e, t, 1.3) - 0.1).abs() < 1e-4, "take 1 over its range");
    assert!((level_at(&e, t, 1.8) - 0.3).abs() < 1e-4, "take 3 after it");
    let st = &e.session().track(t).unwrap().take_stacks()[0];
    assert_eq!(st.active(), Some(3), "take 3 plays most of the stack");
    assert!(st.take(1).unwrap().is_active());
}

#[test]
fn flatten_keeps_what_plays_removes_the_other_takes_and_undo_restores_them() {
    let (mut e, t, ids, other) = three_takes();
    let before = e.session().clone();
    e.execute("takes.set_active", &json!({"track": t.0, "clip": ids[1].0})).unwrap();
    let chosen = e.session().clone();
    let r = e.execute("takes.flatten", &json!({"track": t.0})).unwrap();
    assert_eq!(r["removed"], 2);
    assert_eq!(e.undo_history().last().map(String::as_str), Some("Flatten Takes"));
    let tr = e.session().track(t).unwrap();
    assert_eq!((tr.playlists.len(), tr.active_playlist), (1, 0), "the emptied take playlists go");
    let clips: Vec<(ClipId, Option<u32>)> = tr.clips().iter().map(|c| (c.id, c.take)).collect();
    assert_eq!(clips, vec![(ids[1], None), (other, None)], "take 2 stays, as a plain clip");
    assert!(tr.take_stacks().is_empty());
    assert!((level_at(&e, t, 1.5) - 0.2).abs() < 1e-4);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session(), &chosen, "one undo step brings every take back");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session(), &before);
    // Nothing to flatten is refused and changes nothing.
    let mut plain = Engine::default();
    let p = plain.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    assert!(plain.execute("takes.flatten", &json!({"track": p.0})).is_err());
    assert!(plain.undo_history().is_empty());
}

#[test]
fn flatten_of_one_stack_leaves_the_others_and_a_playlist_that_still_holds_clips() {
    let (mut e, t, ids, _) = three_takes();
    // A second stack at 8 s: take 1 (playlist 1) and take 2 (playlist 0, playing).
    let s = e.session_mut();
    let src = s.track(t).unwrap().playlists[0].clip(ids[2]).and_then(Clip::source).unwrap();
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    let tr = s.track_mut(t).unwrap();
    let mut x = Clip::audio(a, "x", src, 0, 384_000, 1_000);
    x.take = Some(1);
    let mut y = Clip::audio(b, "y", src, 0, 384_000, 1_000);
    y.take = Some(2);
    tr.playlists[1].clips.push(x);
    tr.playlists[0].clips.push(y);
    assert_eq!(e.session().track(t).unwrap().take_stacks().len(), 2);
    e.execute("takes.flatten", &json!({"track": t.0, "stack": 0})).unwrap();
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.take_stacks().len(), 1, "the second stack keeps its takes");
    assert_eq!(tr.playlists.len(), 2, "playlist 1 still holds a take of the second stack");
    assert!(e.execute("takes.flatten", &json!({"track": t.0, "stack": 4})).is_err());
}

#[test]
fn delete_removes_a_take_that_does_not_play_and_refuses_the_one_that_does() {
    let (mut e, t, ids, _) = three_takes();
    let err = e.execute("takes.delete", &json!({"track": t.0, "take": 3})).unwrap_err().to_string();
    assert!(err.contains("take 3 is playing"), "{err}");
    let r = e.execute("takes.delete", &json!({"track": t.0, "take": 1})).unwrap();
    assert_eq!(r["removed"], 1);
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.playlists.len(), 2);
    assert!(tr.playlists.iter().all(|p| p.clip(ids[0]).is_none()));
    assert_eq!(tr.take_numbers(), vec![3, 2]);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session().track(t).unwrap().take_numbers(), vec![3, 2, 1]);
}

#[test]
fn takes_list_reports_the_stacks_and_untagged_takes_get_numbers_when_chosen() {
    let (mut e, t, ids, _) = three_takes();
    // An older session's takes carry no numbers: playlist order numbers them.
    for p in &mut e.session_mut().track_mut(t).unwrap().playlists {
        p.clips.iter_mut().for_each(|c| c.take = None);
    }
    let l = e.execute("takes.list", &json!({"track": t.0})).unwrap();
    let stack = &l["stacks"][0];
    assert_eq!((stack["start"].as_i64(), stack["end"].as_i64(), stack["active"].as_u64()), (Some(48_000), Some(96_000), Some(1)));
    let numbers: Vec<(u64, bool)> =
        stack["takes"].as_array().unwrap().iter().map(|k| (k["number"].as_u64().unwrap(), k["explicit"].as_bool().unwrap())).collect();
    assert_eq!(numbers, vec![(1, false), (2, false), (3, false)]);
    assert!(e.undo_history().is_empty(), "listing is no edit");
    // Choosing writes the numbers first, so each take keeps its lane when its clip moves.
    e.execute("takes.set_active", &json!({"track": t.0, "clip": ids[0].0})).unwrap();
    let st = &e.session().track(t).unwrap().take_stacks()[0];
    assert!(st.explicit());
    assert_eq!(st.take_of(ids[0]).map(|k| (k.number, k.is_active())), Some((2, true)), "take 2 (playlist 1) plays");
    assert_eq!(st.take_of(ids[2]).map(|k| (k.number, k.is_active())), Some((1, false)));
}

#[test]
fn the_active_take_round_trips_through_the_session_file() {
    let (mut e, t, ids, _) = three_takes();
    e.execute("takes.set_active", &json!({"track": t.0, "clip": ids[0].0})).unwrap();
    let text = e.session().to_json().unwrap();
    let back = transit_model::Session::from_json(&text).unwrap();
    let (a, b) = (e.session().track(t).unwrap(), back.track(t).unwrap());
    assert_eq!(a.take_stacks(), b.take_stacks());
    assert_eq!(b.take_stacks()[0].active(), Some(1));
    assert_eq!(a.playlists, b.playlists);
    // A session without take stacks writes no take field.
    assert!(!Engine::default().session().to_json().unwrap().contains("\"take\""));
}

#[test]
fn a_frozen_track_refuses_a_take_change() {
    let (mut e, t, ids, _) = three_takes();
    let tr = e.session_mut().track_mut(t).unwrap();
    tr.freeze = Some(transit_model::Frozen { source: SourceId(1), start: 0, length: 10, hash: tr.freeze_hash(), revision: 1, latency: 0 });
    let before = e.session().clone();
    assert!(e.execute("takes.set_active", &json!({"track": t.0, "clip": ids[0].0})).is_err());
    assert_eq!(e.session(), &before);
    // Deleting a take that does not play would leave the freeze valid; it is refused all the same.
    let err = e.execute("takes.delete", &json!({"track": t.0, "take": 1})).unwrap_err();
    assert!(matches!(err, crate::EngineError::TrackFrozen), "{err}");
    assert_eq!(e.session(), &before);
    assert!(e.undo_history().is_empty());
}

#[test]
fn a_frozen_track_refuses_flatten_of_an_unnumbered_stack() {
    let (mut e, t, _, _) = three_takes();
    let tr = e.session_mut().track_mut(t).unwrap();
    // An older session's takes: no numbers, so flatten would only remove other playlists' clips.
    for p in &mut tr.playlists {
        p.clips.iter_mut().for_each(|c| c.take = None);
    }
    tr.freeze = Some(transit_model::Frozen { source: SourceId(1), start: 0, length: 10, hash: tr.freeze_hash(), revision: 1, latency: 0 });
    let before = e.session().clone();
    let err = e.execute("takes.flatten", &json!({"track": t.0})).unwrap_err();
    assert!(matches!(err, crate::EngineError::TrackFrozen), "{err}");
    assert_eq!(e.session(), &before);
    assert!(e.undo_history().is_empty());
}

#[test]
fn bad_parameters_are_refused_without_a_change() {
    let (mut e, t, _, other) = three_takes();
    let before = e.session().clone();
    for p in [
        json!({"track": t.0}),
        json!({"track": t.0, "clip": other.0}),
        json!({"track": t.0, "clip": 9_999}),
        json!({"track": t.0, "take": 7}),
        json!({"track": t.0, "take": 1, "stack": 3}),
        json!({"take": 1}),
    ] {
        assert!(e.execute("takes.set_active", &p).is_err(), "{p}");
    }
    assert!(e.execute("takes.delete", &json!({"track": t.0})).is_err());
    assert_eq!(e.session(), &before);
    assert!(e.undo_history().is_empty());
}
