//! ADR 0056 slice 1 (#554): split, trim, heal and the file bounds of an audio clip go through its
//! mapping (`offset` read at clip time `anchor`), so a clip with a stretch other than 1 keeps its
//! audio through every edit; the inherited processing that reads from `offset` refuses a
//! re-anchored clip; AudioSuite resets the stretch it bakes; an edit whose result leaves the numeric
//! bounds is refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use crate::edit::{TimeCtx, split_clip};
use serde_json::json;
use transit_audio_io::AudioBuffer;
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, ClipLoop, Session, TrackId, TrackKind};
use transit_time::{Range, Samples};

/// The stretches the record names for slice 1 (decision 5's evidence), and 1.
const STRETCHES: [f64; 5] = [1.0, 0.5, 2.0, 1.3, 0.77];

/// A mono ramp source (`x[i] = i`, exact in `f32`) of `frames` frames on one audio track, and one
/// clip on it at timeline 1000: `offset`, `length`, `stretch`, `looping`.
fn ramp(frames: usize, offset: Samples, length: Samples, stretch: f64, looping: Option<ClipLoop>) -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
    let data: Vec<f32> = (0..frames).map(|i| i as f32).collect();
    let src =
        crate::io::add_source(&mut s, "ramp", AudioBuffer { sample_rate: 48_000, channels: vec![data] }, None, transit_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "c", src, offset, 1_000, length);
    c.stretch = stretch;
    c.set_loop(looping);
    crate::edit::place_clip(&mut s, t, c);
    s.edit.selected_tracks = vec![t];
    (Engine::new(s), t, id)
}

fn render(e: &Engine, t: TrackId, from: Samples, to: Samples) -> Vec<f32> {
    transit_mix::render_clips(e.session(), t, Range::new(from, to)).remove(0)
}

fn clips(e: &Engine, t: TrackId) -> Vec<Clip> {
    e.session().track(t).unwrap().clips().to_vec()
}

fn the_clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).unwrap().1.clone()
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// The position the whole clip reads at timeline sample `at`, and the one the piece covering it
/// reads there.
fn assert_pieces_read_like(whole: &Clip, pieces: &[Clip], what: &str) {
    for piece in pieces {
        for p in 0..piece.length {
            let at = piece.start + p;
            assert_eq!(
                piece.source_pos_at(p).to_bits(),
                whole.source_pos_at(at - whole.start).to_bits(),
                "{what}: timeline {at} (piece at {}, clip time {p})",
                piece.start
            );
        }
    }
}

/// `a_split_clip_reads_the_same_positions`: a clip split up to three times reads, at every sample,
/// exactly the position the unsplit clip reads, at rate 1 and at stretch 0.5, 2, 1.3 and 0.77,
/// looped and unlooped; #554's case reads 150 after the split at 100, not 200. Negative control: the
/// base's `offset += phase` (the right piece at 200).
#[test]
fn a_split_clip_reads_the_same_positions() {
    let s = Session::default();
    let tc = TimeCtx::of(&s);
    let cuts: [&[Samples]; 4] = [&[100], &[37, 250], &[1, 199, 398], &[333, 334, 600]];
    for stretch in STRETCHES {
        for looping in [None, Some(ClipLoop { start: 50, end: 250 }), Some(ClipLoop { start: -30, end: 120 })] {
            for at in cuts {
                let mut whole = Clip::audio(ClipId(1), "w", transit_model::SourceId(1), 100, 1_000, 700);
                whole.stretch = stretch;
                whole.set_loop(looping);
                let mut pieces = Vec::new();
                let mut rest = whole.clone();
                for (i, &cut) in at.iter().enumerate() {
                    let (l, r) = split_clip(&rest, 1_000 + cut, ClipId(10 + i as u64), tc).unwrap();
                    pieces.push(l);
                    rest = r;
                }
                pieces.push(rest);
                assert_pieces_read_like(&whole, &pieces, &format!("stretch {stretch}, loop {looping:?}, cuts {at:?}"));
            }
        }
    }
    let mut whole = Clip::audio(ClipId(1), "w", transit_model::SourceId(1), 100, 0, 400);
    whole.stretch = 2.0;
    let (_, right) = split_clip(&whole, 100, ClipId(2), tc).unwrap();
    assert_eq!(right.source_pos_at(0), 150.0, "#554: the piece after the split continues at 150");
}

/// `a_split_clip_nulls_against_the_unsplit_clip` (#554's split case): with 0 dB, no envelope and no
/// fades, the pieces of `edit.separate` render the unsplit clip bit for bit, at every rate, looped
/// and unlooped. Negative control: the base's split (the stretch-2 right piece starts 50 source
/// frames late).
#[test]
fn a_split_clip_nulls_against_the_unsplit_clip() {
    for stretch in STRETCHES {
        for looping in [None, Some(ClipLoop { start: 64, end: 300 })] {
            let (mut e, t, _) = ramp(2_000, 100, 400, stretch, looping);
            let before = render(&e, t, 900, 1_500);
            for at in [1_100, 1_257, 1_399] {
                e.execute("edit.separate", &json!({"tracks": [t.0], "at": at})).unwrap();
            }
            assert_eq!(clips(&e, t).len(), 4);
            assert_eq!(bits(&render(&e, t, 900, 1_500)), bits(&before), "stretch {stretch}, loop {looping:?}");
        }
    }
}

/// #554's trim case: a start trim (inward and back outward) and an end trim of a stretched clip
/// render, over what remains, exactly what the untrimmed clip rendered there. Negative control: the
/// base's start trim reads `phase` frames late at any stretch other than 1.
#[test]
fn a_trimmed_stretched_clip_keeps_its_audio() {
    for stretch in STRETCHES {
        let (mut e, t, id) = ramp(4_000, 100, 400, stretch, None);
        let before = render(&e, t, 1_000, 1_400);
        e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 1_100})).unwrap();
        assert_eq!(bits(&render(&e, t, 1_100, 1_400)), bits(&before[100..]), "start trim, stretch {stretch}");
        e.execute("edit.trim_clip", &json!({"clip": id.0, "end": 1_300})).unwrap();
        assert_eq!(bits(&render(&e, t, 1_100, 1_300)), bits(&before[100..300]), "end trim, stretch {stretch}");
        e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 1_000})).unwrap();
        assert_eq!(bits(&render(&e, t, 1_000, 1_300)), bits(&before[..300]), "trimmed back out, stretch {stretch}");
        assert_eq!(the_clip(&e, id).canonical().content, ClipContent::Audio { source: the_clip(&e, id).source().unwrap(), offset: 100, anchor: 0 });
    }
}

/// `heal_merges_only_what_splits_back` (#554's heal case): split then heal gives the clip back at
/// every rate, and heal then split gives the pieces back; at rate 1 the round's `(100, 0)` split at
/// 20 heals with `(120, 0)`; at stretch 2 the two pieces the base's split made (offsets 100 and 200,
/// no anchor) do not play continuously and are not healed; two pieces at q = 2 with equivalent but
/// different fields are not healed either. Negative control: the base's heal (`offset + length`).
#[test]
fn heal_merges_only_what_splits_back() {
    for stretch in STRETCHES {
        let (mut e, t, id) = ramp(4_000, 100, 400, stretch, None);
        let whole = the_clip(&e, id);
        e.execute("edit.separate", &json!({"tracks": [t.0], "at": 1_120})).unwrap();
        let pieces = clips(&e, t);
        assert_eq!(e.execute("edit.heal", &json!({"tracks": [t.0], "start": 1_000, "end": 1_400})).unwrap()["healed"], 1, "stretch {stretch}");
        assert_eq!(clips(&e, t), vec![whole.clone()], "split then heal, stretch {stretch}");
        e.execute("edit.separate", &json!({"tracks": [t.0], "at": 1_120})).unwrap();
        let again = clips(&e, t);
        assert_eq!(
            again.iter().map(|c| (c.start, c.length, c.content.clone())).collect::<Vec<_>>(),
            pieces.iter().map(|c| (c.start, c.length, c.content.clone())).collect::<Vec<_>>(),
            "heal then split, stretch {stretch}"
        );
        if stretch == 1.0 {
            assert_eq!((again[1].source_offset(), again[1].anchor()), (220, 0), "rate 1: the normal form, offset 100 + 120");
        } else {
            assert_eq!((again[1].source_offset(), again[1].anchor()), (100, -120), "the offset stays, the anchor moves");
        }
    }
    // The round's example: (offset 100, anchor 0) at rate 1 split at 20 stores the right piece as
    // (120, 0), and the two heal.
    let (mut e, t, _) = ramp(4_000, 100, 100, 1.0, None);
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 1_020})).unwrap();
    assert_eq!((clips(&e, t)[1].source_offset(), clips(&e, t)[1].anchor()), (120, 0));
    assert_eq!(e.execute("edit.heal", &json!({"tracks": [t.0], "start": 1_000, "end": 1_100})).unwrap()["healed"], 1);
    // The base's split of a stretch-2 clip: the right piece at offset 200, which is not where the
    // left piece continues (150), so heal leaves both.
    let (mut e, t, id) = ramp(4_000, 100, 400, 2.0, None);
    let src = the_clip(&e, id).source().unwrap();
    let s = e.session_mut();
    let pl = s.track_mut(t).unwrap().playlist_mut().unwrap();
    pl.clips[0].length = 100;
    let mut right = pl.clips[0].clone();
    right.id = ClipId(77);
    right.start = 1_100;
    right.length = 300;
    right.content = ClipContent::Audio { source: src, offset: 200, anchor: 0 };
    pl.clips.push(right);
    let r = e.execute("edit.heal", &json!({"tracks": [t.0], "start": 1_000, "end": 1_400}));
    assert!(r.map_or(true, |v| v["healed"] == 0), "pieces that jump are not healed");
    assert_eq!(clips(&e, t).len(), 2);
    // q = 2 (stretch 0.5): (100, 0) and, touching at clip time 1, (102, 1) read the same positions,
    // but heal compares canonical fields and leaves them.
    let (mut e, t, id) = ramp(4_000, 100, 400, 0.5, None);
    let s = e.session_mut();
    let pl = s.track_mut(t).unwrap().playlist_mut().unwrap();
    pl.clips[0].length = 1;
    let mut right = pl.clips[0].clone();
    right.id = ClipId(78);
    right.start = 1_001;
    right.length = 399;
    right.content = ClipContent::Audio { source: the_clip(&e, id).source().unwrap(), offset: 102, anchor: 1 };
    e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips.push(right);
    let r = e.execute("edit.heal", &json!({"tracks": [t.0], "start": 1_000, "end": 1_400}));
    assert!(r.map_or(true, |v| v["healed"] == 0), "the conservative rule at q != 1");
    assert_eq!(clips(&e, t).len(), 2);
}

/// `trims_and_fade_handles_respect_the_mapped_file_bounds` (q = 0.5 and 2; forward, the only
/// direction before slice 6a): a 1,000-frame file read from offset 100 lets a start trim reach the
/// clip time where the mapping reads frame 0 and an end trim the one where it reads frame 999; Trim
/// to File and the crossfade handles use the same bounds. Negative control: the base's bounds count
/// clip samples as file frames (offset 100 earlier, file length − offset later).
#[test]
fn trims_and_fade_handles_respect_the_mapped_file_bounds() {
    // stretch 2 (q = 1/2): frames 0..=999 at clip times -200..=1798; stretch 0.5 (q = 2): -50..=449.
    for (stretch, first, end) in [(2.0, -200, 1_799), (0.5, -50, 450)] {
        let (mut e, _, id) = ramp(1_000, 100, 40, stretch, None);
        e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 0})).unwrap();
        let c = the_clip(&e, id);
        assert_eq!(c.start, 1_000 + first, "start bound, stretch {stretch}");
        assert_eq!(c.source_pos_at(0), 0.0);
        e.execute("edit.trim_clip", &json!({"clip": id.0, "end": 100_000})).unwrap();
        let c = the_clip(&e, id);
        assert_eq!(c.end(), 1_000 + end, "end bound, stretch {stretch}");
        assert!(c.source_pos_at(c.length - 1) <= 999.0 && c.source_pos(c.length) > 999.0);

        let (mut e, _, id) = ramp(1_000, 100, 40, stretch, None);
        e.execute("edit.trim_to_file_boundaries", &json!({"clips": [id.0]})).unwrap();
        assert_eq!(the_clip(&e, id).range(), Range::new(1_000 + first, 1_000 + end), "trim to file, stretch {stretch}");
    }
    // Crossfade handles: two touching stretch-2 pieces (from a split, so the right one is anchored)
    // extend into each other by what their mappings have left in the file.
    let (mut e, t, _) = ramp(1_000, 100, 400, 2.0, None);
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 1_200})).unwrap();
    let before = clips(&e, t);
    e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": 1_100, "end": 1_300, "shape": "linear"})).unwrap();
    let after = clips(&e, t);
    assert_eq!((after[0].end(), after[1].start), (1_300, 1_100), "both handles reach across the range");
    assert_pieces_read_like(&before[0], &after[..1], "left handle");
    let mut whole = before[0].clone();
    whole.length = 400;
    assert_pieces_read_like(&whole, &after, "both pieces still read the whole clip's positions");
    // At the file's start: a clip reading from frame 10 at q = 1/2 has 20 clip samples before it.
    let (mut e, t, _) = ramp(1_000, 10, 400, 2.0, None);
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 1_200})).unwrap();
    let left = clips(&e, t)[0].clone();
    let s = e.session_mut();
    let pl = s.track_mut(t).unwrap().playlist_mut().unwrap();
    pl.clips.retain(|c| c.start == 1_200);
    let mut l = left.clone();
    l.start = 1_100;
    l.length = 100;
    l.content = ClipContent::Audio { source: left.source().unwrap(), offset: 500, anchor: 0 };
    pl.clips.insert(0, l);
    e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": 1_150, "end": 1_250, "shape": "linear"})).unwrap();
    let r = &clips(&e, t)[1];
    assert_eq!(r.start, 1_150, "the right piece extends 50");
    let (mut e, t, id) = ramp(1_000, 10, 400, 2.0, None);
    e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 0})).unwrap();
    assert_eq!(clips(&e, t)[0].start, 980, "frame 10 at q = 1/2 leaves 20 clip samples before the start");
}

/// `the_domain_is_closed` through commands: the round-3 clip (loop `[−2^40, 64)`, length 100,
/// q = 1/32, offset 2^35) is valid, and splitting it at 63 would put its loop start below
/// −MAX_POSITION, so the split is refused and the clip is unchanged (checked refusal, never a
/// clamp). The round's clip (offset 0, anchor −2^40, length 2^40, q = 1/32, 2^36 + 10 frames) is
/// start-trimmed by one sample and split, each result inside +-2^46, and saves and loads unchanged.
#[test]
fn the_domain_is_closed() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
    let src = s.alloc();
    s.sources.push(transit_model::Source {
        id: transit_model::SourceId(src),
        name: "far".into(),
        path: "far.wav".into(),
        channels: 1,
        frames: (1 << 36) + 10,
        sample_rate: 48_000,
        format: transit_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: false,
    });
    let src = transit_model::SourceId(src);
    let mut c = Clip::audio(ClipId(500), "r3", src, 1 << 35, 0, 100);
    c.stretch = 32.0;
    c.looping = Some(ClipLoop { start: -(1 << 40), end: 64 });
    let mut big = Clip::audio(ClipId(501), "round", src, 0, 1_000, 1 << 40);
    big.stretch = 32.0;
    big.content = ClipContent::Audio { source: src, offset: 0, anchor: -(1 << 40) };
    let tr = s.track_mut(t).unwrap().playlist_mut().unwrap();
    tr.clips = vec![c.clone(), big.clone()];
    s.edit.selected_tracks = vec![t];
    let mut e = Engine::new(s);
    let err = e.execute("edit.separate", &json!({"tracks": [t.0], "at": 63})).unwrap_err();
    assert!(err.to_string().contains("its loop range would be outside"), "{err}");
    assert_eq!(the_clip(&e, ClipId(500)), c, "refused, not clamped");
    e.execute("edit.trim_clip", &json!({"clip": 501, "start": 1_001})).unwrap();
    assert_eq!(the_clip(&e, ClipId(501)).anchor(), -(1 << 40) - 1);
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 1_000i64 + (1i64 << 39)})).unwrap();
    let pieces: Vec<Clip> = clips(&e, t).into_iter().filter(|c| c.start >= 1_000).collect();
    assert_eq!(pieces.len(), 2);
    let mut trimmed = big.clone();
    trimmed.trim_start_to(1_001);
    for p in &pieces {
        assert!(p.anchor().abs() <= transit_model::MAX_ANCHOR && p.domain_problem().is_none());
        for k in [0, 1, p.length - 1] {
            assert_eq!(p.source_pos_at(k).to_bits(), trimmed.source_pos_at(p.start - trimmed.start + k).to_bits());
        }
    }
    let back = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert!(back.load_notes.is_empty(), "{:?}", back.load_notes);
    assert_eq!(back.tracks[0].clips(), e.session().track(t).unwrap().clips());
}

/// `inherited_processing_refuses_an_anchored_clip`: Elastic properties (on an Elastic track and on a
/// plain one), TCE to Timeline, Remove Pitch Shift, Conform to Tempo and Remove Warp refuse a split
/// piece of a stretched clip and change nothing. Negative control: the base's `render_stretched`
/// reads that piece from `offset`, its clip start.
#[test]
fn inherited_processing_refuses_an_anchored_clip() {
    let (mut e, t, _) = ramp(48_000, 100, 4_000, 2.0, None);
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 2_000})).unwrap();
    let piece = clips(&e, t)[1].clone();
    assert_ne!(piece.anchor(), 0);
    let before = e.session().to_json().unwrap();
    let calls = [
        ("clip.elastic_properties", json!({"clips": [piece.id.0], "ratio": 1.5})),
        ("edit.tce_to_timeline", json!({"clips": [piece.id.0], "start": 2_000, "end": 9_000})),
        ("clip.remove_pitch_shift", json!({"clips": [piece.id.0]})),
        ("clip.conform_to_tempo", json!({"clips": [piece.id.0], "source_bpm": 100.0})),
        ("clip.remove_warp", json!({"clips": [piece.id.0]})),
    ];
    for track_elastic in [false, true] {
        if track_elastic {
            e.execute("track.elastic", &json!({"track": t.0, "algorithm": "polyphonic"})).unwrap();
        }
        let before = e.session().to_json().unwrap();
        for (cmd, p) in &calls {
            let err = e.execute(cmd, p).unwrap_err();
            assert!(err.to_string().contains("split or trimmed at its stretch"), "{cmd}: {err}");
            assert_eq!(e.session().to_json().unwrap(), before, "{cmd} changed nothing");
        }
    }
    let _ = before;
    // An unanchored stretched clip is not refused.
    let (mut e, _, id) = ramp(48_000, 100, 4_000, 2.0, None);
    e.execute("clip.remove_warp", &json!({"clips": [id.0]})).unwrap();
    assert_eq!(the_clip(&e, id).stretch, 1.0);
}

/// #554's AudioSuite case: processing a stretched clip bakes the stretch into the new file, so the
/// replacement clip plays at stretch 1 and sounds as the clip did. Negative control: the base keeps
/// the stretch and plays the render stretched a second time.
#[test]
fn audiosuite_resets_the_stretch_it_bakes() {
    for stretch in [2.0, 0.5, 1.3] {
        let (mut e, t, id) = ramp(4_000, 100, 400, stretch, None);
        let before = render(&e, t, 1_000, 1_400);
        e.execute("audiosuite.process", &json!({"clips": [id.0], "process": "gain", "params": {"db": 0.0}})).unwrap();
        let c = &clips(&e, t)[0];
        assert_eq!((c.stretch, c.anchor(), c.length, c.start), (1.0, 0, 400, 1_000), "stretch {stretch}");
        assert_eq!(bits(&render(&e, t, 1_000, 1_400)), bits(&before), "stretch {stretch}: the processed clip sounds as it did");
    }
}

/// `fades_and_the_envelope_keep_their_clip_time_place`, slice 1's part (decision 9.3: start trims,
/// end trims and splits; slip, reverse and transpose come with their slices), on a stretched clip:
/// each piece keeps the fade on its outer edge, a split inside a fade clamps it to its piece, the
/// later piece and a start trim drop the envelope points before the cut and insert one at the cut
/// with today's `env_value` there, an end trim keeps points past the end. Negative control: a start
/// trim that does not insert the cut point.
#[test]
fn fades_and_the_envelope_keep_their_clip_time_place() {
    use transit_model::{Fade, FadeShape};
    let s = Session::default();
    let tc = TimeCtx::of(&s);
    let mut whole = Clip::audio(ClipId(1), "w", transit_model::SourceId(1), 100, 0, 1_000);
    whole.stretch = 2.0;
    whole.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
    whole.fade_out = Fade { len: 200, shape: FadeShape::SCurve, curvature: 0.3 };
    whole.gain_env = vec![(0, 0.0), (500, -12.0), (1_000, -6.0)];
    let fades = |c: &Clip| (c.fade_in.len, c.fade_out.len);
    // Outside both fades: unchanged on the outer edges, none on the inner ones.
    let (l, r) = split_clip(&whole, 600, ClipId(2), tc).unwrap();
    assert_eq!((fades(&l), fades(&r)), ((100, 0), (0, 200)));
    assert_eq!(r.fade_out, whole.fade_out);
    assert_eq!(l.gain_env, whole.gain_env, "the earlier piece keeps every point");
    assert_eq!(r.gain_env.len(), 2);
    assert_eq!(r.gain_env[0], (0, -12.0 + 6.0 * (100f32 / 500f32)), "a point at the cut, today's env_value there");
    assert_eq!(r.gain_env[1], (400, -6.0), "the rest moved earlier by the cut");
    // Inside the fade-in and inside the fade-out: clamped to the piece.
    let (l, r) = split_clip(&whole, 50, ClipId(2), tc).unwrap();
    assert_eq!((fades(&l), fades(&r)), ((50, 0), (0, 200)));
    let (l, r) = split_clip(&whole, 900, ClipId(2), tc).unwrap();
    assert_eq!((fades(&l), fades(&r)), ((100, 0), (0, 100)));
    // A start trim: the fade-in keeps its length from the new start; an end trim keeps the fade-out
    // at the end and the points past it.
    let mut t = whole.clone();
    t.trim_start_to(300);
    assert_eq!(fades(&t), (100, 200));
    assert_eq!(t.gain_env.iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, 200, 700]);
    t.trim_end_to(500);
    assert_eq!((fades(&t), t.gain_env.len()), ((100, 200), 3));
    // Breakpoints sharing a time keep their order through the edits.
    let mut d = whole.clone();
    d.gain_env = vec![(0, 0.0), (700, -3.0), (700, -9.0), (1_000, 0.0)];
    let (_, r) = split_clip(&d, 600, ClipId(2), tc).unwrap();
    assert_eq!(r.gain_env[1..], [(100, -3.0), (100, -9.0), (400, 0.0)]);
}
