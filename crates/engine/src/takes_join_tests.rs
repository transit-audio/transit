//! Choosing a take back after a shorter take was chosen inside it joins its pieces again, so it
//! renders exactly as before it was cut (#25, review minors 1 and 2). Frames and values only.

use crate::Engine;
use serde_json::json;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, Fade, FadeShape, Playlist, Session, TrackId, TrackKind};
use transit_time::{Range, TICKS_PER_QUARTER};

/// Put `a` (take 1) on playlist 0, which plays, and `b` (take 2) on playlist 1.
fn stack(s: &mut Session, t: TrackId, mut a: Clip, mut b: Clip) {
    a.take = Some(1);
    b.take = Some(2);
    let tr = s.track_mut(t).unwrap();
    tr.playlists = vec![Playlist { name: "A".into(), clips: vec![a] }, Playlist { name: "B".into(), clips: vec![b] }];
    tr.active_playlist = 0;
}

fn render(s: &Session, frames: i64) -> Vec<f32> {
    transit_mix::render_range(s, Range::new(0, frames), 512).swap_remove(0)
}

fn energy(x: &[f32], from: usize, to: usize) -> f32 {
    x[from..to].iter().map(|v| v * v).sum()
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// Choose take 2 (`b`), then take 1 back by its moved piece, as a lane click does.
fn choose_and_choose_back(e: &mut Engine, t: TrackId, b: ClipId) {
    e.execute("takes.set_active", &json!({"track": t.0, "clip": b.0})).unwrap();
    let piece = e.session().track(t).unwrap().playlists[1].clips.iter().find(|c| c.take == Some(1)).map(|c| c.id).expect("take 1's middle piece");
    e.execute("takes.set_active", &json!({"track": t.0, "clip": piece.0})).unwrap();
}

#[test]
fn a_midi_take_chosen_back_plays_its_note_across_the_cut_again() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    let q = TICKS_PER_QUARTER;
    // A: 2 s, one note from 0.5 s to 1.5 s (120 bpm: a quarter is 24,000 frames). B: a shorter
    // take at 40,000..56,000, inside that note.
    let mut long = Sequence::default();
    long.notes.push(Note { pitch: 69, velocity: 100, release_velocity: 64, channel: 0, start: q, length: 2 * q });
    let mut short = Sequence::default();
    short.notes.push(Note { pitch: 81, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: q / 4 });
    let s = e.session_mut();
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    stack(s, t, Clip::midi(a, "A", 0, 96_000, long), Clip::midi(b, "B", 40_000, 16_000, short));
    let original = e.session().track(t).unwrap().clips()[0].clone();
    let before = render(e.session(), 96_000);
    choose_and_choose_back(&mut e, t, b);
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.clips(), std::slice::from_ref(&original), "take 1 is one clip again, as it was");
    assert_eq!(tr.playlists[1].clips.iter().map(|c| c.id).collect::<Vec<_>>(), vec![b], "take 2 is back on its playlist");
    let after = render(e.session(), 96_000);
    let (x, y) = (energy(&before, 30_000, 50_000), energy(&after, 30_000, 50_000));
    assert!(x > 1.0, "the note sounds over the cut: {x}");
    assert_eq!(x, y, "energy over 30k..50k");
    assert_eq!(max_diff(&before, &after), 0.0);
    // One undo step per choice: two undos give the untouched session.
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session().track(t).unwrap().clips(), std::slice::from_ref(&original));
}

#[test]
fn an_audio_take_chosen_back_gets_its_whole_fades_again() {
    // A ramp source, so an offset into it shows in the render.
    let ramp: Vec<f32> = (0..96_000).map(|i| 0.2 + 0.6 * i as f32 / 96_000.0).collect();
    // B inside A's fade-in, then inside A's fade-out.
    for (b_start, b_len) in [(50_000, 10_000), (88_000, 4_000)] {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let src = crate::io::add_source(s, "ramp", AudioBuffer { sample_rate: 48_000, channels: vec![ramp.clone()] }, None, FileFormat::Wav);
        let (a, b) = (s.new_clip_id(), s.new_clip_id());
        let mut clip_a = Clip::audio(a, "A", src, 12_000, 48_000, 48_000);
        clip_a.fade_in = Fade { len: 20_000, shape: FadeShape::SCurve, curvature: 0.3 };
        clip_a.fade_out = Fade { len: 10_000, shape: FadeShape::Exponential, curvature: -0.2 };
        clip_a.gain_db = -3.0;
        clip_a.gain_env = vec![(0, 0.0), (30_000, -6.0)];
        stack(s, t, clip_a, Clip::audio(b, "B", src, 0, b_start, b_len));
        let original = e.session().track(t).unwrap().clips()[0].clone();
        let before = render(e.session(), 120_000);
        e.execute("takes.set_active", &json!({"track": t.0, "clip": b.0})).unwrap();
        let cut = e.session().track(t).unwrap().clips().to_vec();
        assert_eq!(cut.len(), 3, "A cut around B");
        assert!(cut[0].fade_in.len <= cut[0].length && cut[2].fade_out.len <= cut[2].length, "pieces keep fades within themselves");
        choose_and_choose_back(&mut e, t, b);
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.clips(), std::slice::from_ref(&original), "B at {b_start}: take 1 is one clip again, fades whole");
        let after = render(e.session(), 120_000);
        assert_eq!(max_diff(&before, &after), 0.0, "B at {b_start}");
        assert_eq!(before[1_055 + 48_000], after[1_055 + 48_000]);
    }
}

#[test]
fn pieces_edited_after_the_cut_are_not_joined() {
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let src = crate::io::add_source(s, "dc", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 96_000]] }, None, FileFormat::Wav);
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    stack(s, t, Clip::audio(a, "A", src, 0, 0, 96_000), Clip::audio(b, "B", src, 0, 40_000, 16_000));
    e.execute("takes.set_active", &json!({"track": t.0, "clip": b.0})).unwrap();
    // The right piece gets its own gain: it is no longer what the cut made.
    let s = e.session_mut();
    let right = s.track_mut(t).unwrap().playlist_mut().unwrap().clips.iter_mut().find(|c| c.start == 56_000).unwrap();
    right.gain_db = -6.0;
    choose_and_choose_back(&mut e, t, b);
    let starts: Vec<i64> = e.session().track(t).unwrap().clips().iter().map(|c| c.start).collect();
    assert_eq!(starts, vec![0, 56_000], "the left and middle pieces join; the edited one stays apart");
}
