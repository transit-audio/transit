//! Tests of the clip-to-source mapping (ADR 0056 decision 5, slice 1: #554).

use super::*;
use crate::{ChannelFormat, ClipId, ClipLoop, Session, SourceId, TrackKind};

fn clip(offset: Samples, length: Samples, stretch: f64) -> Clip {
    let mut c = Clip::audio(ClipId(1), "c", SourceId(1), offset, 0, length);
    c.stretch = stretch;
    c
}

/// A small deterministic generator (no wall clock, no OS randomness).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo) as u64) as i64
    }
}

/// #554's case in the model: a stretch-2 clip, offset 100, start-trimmed by 100 clip samples, reads
/// at its new start the position the whole clip read there (150), not 200; the offset stays and
/// the anchor moves. Negative control: the base's `offset += phase` reads 200.
#[test]
fn a_start_trim_of_a_stretched_clip_keeps_its_positions() {
    let whole = clip(100, 400, 2.0);
    for cut in [1, 37, 100, 399] {
        let mut piece = whole.clone();
        piece.trim_start_to(cut);
        assert_eq!((piece.source_offset(), piece.anchor()), (100, -cut), "cut {cut}");
        for p in 0..piece.length {
            assert_eq!(piece.source_pos_at(p).to_bits(), whole.source_pos_at(p + cut).to_bits(), "cut {cut}, clip time {p}");
        }
    }
    let mut piece = whole.clone();
    piece.trim_start_to(100);
    assert_eq!(piece.source_pos_at(0), 150.0);
}

/// At rate 1 a start trim stores the normal form (anchor 0, the offset moved), exactly as before
/// this slice, and an outward trim too.
#[test]
fn a_unit_rate_trim_stores_the_normal_form() {
    let mut c = clip(100, 400, 1.0);
    c.trim_start_to(20);
    assert_eq!((c.source_offset(), c.anchor()), (120, 0));
    c.trim_start_to(-30);
    assert_eq!((c.source_offset(), c.anchor(), c.length), (70, 0, 430));
    let mut odd = clip(100, 400, 1.0);
    odd.content = ClipContent::Audio { source: SourceId(1), offset: 100, anchor: -20 };
    assert_eq!(odd.canonical().content, ClipContent::Audio { source: SourceId(1), offset: 120, anchor: 0 });
    assert_eq!(odd.source_pos(0), 120.0);
    let mut stretched = odd.clone();
    stretched.stretch = 2.0;
    assert_eq!(stretched.canonical(), stretched, "the canonical form keeps the fields at another rate");
}

/// The mapping arithmetic is decision 5.3's order: `(c − anchor)` exact in i64, one product, one sum.
#[test]
fn source_pos_is_the_declared_arithmetic() {
    let mut c = clip(1_000, 10_000, 1.3);
    c.content = ClipContent::Audio { source: SourceId(1), offset: 1_000, anchor: -77 };
    let q = 1.0 / 1.3;
    for p in [0i64, 1, 5_000, 9_999] {
        let want = 1_000f64 + (p + 77) as f64 * q;
        assert_eq!(c.source_pos(p).to_bits(), want.to_bits(), "{p}");
    }
    c.set_loop(Some(ClipLoop { start: 100, end: 300 }));
    assert_eq!(c.source_pos_at(300).to_bits(), c.source_pos(100).to_bits(), "the loop wraps before the mapping");
}

/// The bisection reference for [`Clip::clip_span_in_file`]: the clip times whose computed position
/// lies in `[0, Fa − 1]`, found by bisecting outward from a point inside.
fn span_by_bisection(c: &Clip, frames: u64) -> Range {
    let last = (frames.min(1 << 40) - 1) as f64;
    let inside = |t: Samples| (0.0..=last).contains(&c.source_pos(t));
    let s = c.rate();
    let mid = c.anchor() + ((last / 2.0 - c.source_offset() as f64) / s).round() as Samples;
    if !inside(mid) {
        return Range { start: 0, end: 0 };
    }
    let (mut lo, mut hi) = (mid - (1 << 50), mid);
    while hi - lo > 1 {
        let m = lo + (hi - lo) / 2;
        if inside(m) { hi = m } else { lo = m }
    }
    let start = hi;
    let (mut lo, mut hi) = (mid, mid + (1 << 50));
    while hi - lo > 1 {
        let m = lo + (hi - lo) / 2;
        if inside(m) { lo = m } else { hi = m }
    }
    Range { start, end: lo + 1 }
}

/// The span in the file at rate 1 is today's `[−offset, F − offset)`; at other rates it equals a
/// bisection over the computed predicate, on small and very large files.
#[test]
fn the_span_in_the_file_is_exact_at_any_rate() {
    let c = clip(100, 400, 1.0);
    assert_eq!(c.clip_span_in_file(1_000), Range { start: -100, end: 900 });
    assert!(c.clip_span_in_file(0).is_empty());
    // q = 1/2: positions 100 + c/2 in [0, 999] for c in [-200, 1798].
    assert_eq!(clip(100, 400, 2.0).clip_span_in_file(1_000), Range { start: -200, end: 1_799 });
    // q = 2: positions 100 + 2c in [0, 999] for c in [-50, 449].
    assert_eq!(clip(100, 400, 0.5).clip_span_in_file(1_000), Range { start: -50, end: 450 });
    let mut rng = Lcg(554);
    for i in 0..4_000 {
        let frames = if i % 4 == 0 { rng.next() % (1 << 41) + 1 } else { rng.next() % 5_000 + 1 };
        let stretch = [0.5, 2.0, 1.3, 0.77, 1.0, 1.0 / 32.0, 32.0, 1.059_463_094_359_295_3][i % 8];
        let offset = rng.range(0, frames.min(1 << 40) as i64);
        let mut c = clip(offset, 100, stretch);
        c.content = ClipContent::Audio { source: SourceId(1), offset, anchor: rng.range(-(1 << 20), 1 << 20) };
        assert_eq!(c.clip_span_in_file(frames), span_by_bisection(&c, frames), "case {i}: {c:?} over {frames} frames");
    }
}

/// The addressed clip times of decision 5.5.
#[test]
fn addressed_times_cover_the_loop() {
    let c = clip(0, 400, 1.0);
    assert_eq!(c.addressed(), (0, 399));
    let mut l = c.clone();
    l.set_loop(Some(ClipLoop { start: -50, end: 200 }));
    assert_eq!(l.addressed(), (-50, 199));
    l.set_loop(Some(ClipLoop { start: 20, end: 200 }));
    assert_eq!(l.addressed(), (0, 199));
}

/// The numeric bounds of decision 5.5 name the field outside them.
#[test]
fn the_numeric_bounds_name_what_is_outside() {
    let mut c = clip(0, 100, 2.0);
    assert_eq!(c.domain_problem(), None);
    c.content = ClipContent::Audio { source: SourceId(1), offset: 0, anchor: MAX_ANCHOR };
    assert_eq!(c.domain_problem(), None, "the bound is inclusive");
    c.content = ClipContent::Audio { source: SourceId(1), offset: 0, anchor: MAX_ANCHOR + 1 };
    assert!(c.domain_problem().unwrap().contains("its anchor"));
    c.content = ClipContent::Audio { source: SourceId(1), offset: MAX_POSITION + 1, anchor: 0 };
    assert!(c.domain_problem().unwrap().contains("its source offset"));
    c.content = ClipContent::Audio { source: SourceId(1), offset: 0, anchor: 0 };
    c.looping = Some(ClipLoop { start: -MAX_POSITION - 63, end: 64 });
    assert!(c.domain_problem().unwrap().contains("its loop range"));
}

fn one_track(clips: Vec<Clip>) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips = clips;
    s
}

/// `the_mapping_saves_and_loads`: the anchor round-trips; a clip at rate 1 and an unsplit stretched
/// clip write no `anchor` key, the content exactly as the base build wrote it; a start-trimmed
/// stretched clip writes its anchor and the `clip_anchor` need, which an inactive playlist counts
/// too, and a session without one lists no such need.
#[test]
fn the_mapping_saves_and_loads() {
    let plain = clip(100, 400, 1.0);
    let stretched = clip(30, 400, 2.0);
    let text = serde_json::to_string(&plain.content).unwrap();
    assert_eq!(text, r#"{"Audio":{"source":1,"offset":100}}"#, "the base build's bytes");
    assert_eq!(serde_json::to_string(&stretched.content).unwrap(), r#"{"Audio":{"source":1,"offset":30}}"#);
    let mut trimmed = clip(100, 400, 2.0);
    trimmed.trim_start_to(100);
    assert_eq!(serde_json::to_string(&trimmed.content).unwrap(), r#"{"Audio":{"source":1,"offset":100,"anchor":-100}}"#);

    let mut unit_trimmed = plain.clone();
    unit_trimmed.trim_start_to(50);
    let s = one_track(vec![unit_trimmed, stretched.clone()]);
    let json = s.to_json().unwrap();
    assert!(!json.contains("anchor"), "{json}");
    assert!(!s.needs().contains(&"clip_anchor"));

    let mut s = one_track(vec![trimmed.clone()]);
    assert_eq!(s.needs(), vec!["clip_anchor"]);
    let back = Session::from_json(&s.to_json().unwrap()).unwrap();
    assert!(back.load_notes.is_empty(), "{:?}", back.load_notes);
    assert_eq!(back.tracks[0].clips()[0], trimmed);
    // An inactive playlist counts.
    let t = s.tracks[0].id;
    let tr = s.track_mut(t).unwrap();
    tr.playlists.push(crate::Playlist { name: "alt".into(), clips: vec![clip(0, 10, 1.0)] });
    tr.active_playlist = 1;
    assert_eq!(s.needs(), vec!["clip_anchor"], "a stored take or alternate playlist counts");
    let v: serde_json::Value = serde_json::from_str(&s.to_json().unwrap()).unwrap();
    assert_eq!(v["needs"], serde_json::json!(["clip_anchor"]));
}

/// A hand-edited anchor outside +-2^46 is reset at load with a note naming the clip; one at rate 1
/// is stored in normal form; one just inside the bound loads as it is.
#[test]
fn a_loaded_anchor_outside_its_bound_is_reset_with_a_note() {
    let mut a = clip(10, 100, 2.0);
    a.name = "far".into();
    a.content = ClipContent::Audio { source: SourceId(1), offset: 10, anchor: MAX_ANCHOR + 1 };
    let mut b = clip(10, 100, 2.0);
    b.id = ClipId(2);
    b.start = 1_000;
    b.content = ClipContent::Audio { source: SourceId(1), offset: 10, anchor: -MAX_ANCHOR };
    let mut u = clip(10, 100, 1.0);
    u.id = ClipId(3);
    u.start = 2_000;
    u.content = ClipContent::Audio { source: SourceId(1), offset: 10, anchor: -5 };
    let text = one_track(vec![a, b.clone(), u]).to_json().unwrap();
    let back = Session::from_json(&text).unwrap();
    let clips = back.tracks[0].clips();
    assert_eq!(clips[0].anchor(), 0);
    assert_eq!(clips[1], b);
    assert_eq!((clips[2].source_offset(), clips[2].anchor()), (15, 0), "the normal form at rate 1");
    assert_eq!(back.load_notes.len(), 1, "{:?}", back.load_notes);
    assert!(back.load_notes[0].starts_with("track \"A\", clip \"far\": its anchor 70368744177665 was outside"), "{:?}", back.load_notes);
    assert!(crate::is_clip_load_note(&back.load_notes[0], "A"));
}

/// `the_domain_is_closed` in the model: the round's clip (offset 0, anchor −2^40, length 2^40,
/// q = 1/32 on a source of 2^36 + 10 frames) start-trimmed by one sample stays inside +-2^46, keeps
/// the material invariant, reads the positions it read, and saves and loads unchanged.
#[test]
fn the_rounds_clip_stays_in_the_domain_through_a_trim() {
    let frames: u64 = (1 << 36) + 10;
    let mut c = clip(0, 1 << 40, 32.0);
    c.content = ClipContent::Audio { source: SourceId(1), offset: 0, anchor: -(1 << 40) };
    let (first, last) = c.addressed();
    assert_eq!((c.source_pos(first), c.source_pos(last)), ((1u64 << 35) as f64, (1u64 << 36) as f64 - 1.0 / 32.0));
    let span = c.clip_span_in_file(frames);
    assert!(span.start <= first && last < span.end, "the invariant holds");
    let whole = c.clone();
    c.trim_start_to(1);
    assert_eq!(c.anchor(), -(1 << 40) - 1);
    assert_eq!(c.domain_problem(), None);
    for p in [0, 1, (1 << 40) - 2] {
        assert_eq!(c.source_pos_at(p).to_bits(), whole.source_pos_at(p + 1).to_bits());
    }
    let s = one_track(vec![c.clone()]);
    let back = Session::from_json(&s.to_json().unwrap()).unwrap();
    assert_eq!(back.tracks[0].clips()[0], c);
}

/// `StreamMap` carries the mapping (ADR 0056 decision 5.6): the map of a start-trimmed piece of a
/// stretched clip reads, through the clip reader, exactly what the whole clip's map reads at the
/// same timeline sample, its fingerprint differs from the whole clip's, and at rate 1 the map is the
/// canonical form (anchor 0). Negative control: a reader that ignores the anchor.
#[test]
fn the_stream_map_reads_a_trimmed_piece_as_the_whole_clip() {
    use crate::disk::{Memory, StreamMap};
    let data = vec![(0..2_000).map(|i| ((i * 7919) % 1_000) as f32 / 1_000.0).collect::<Vec<f32>>()];
    let read = |m: &StreamMap, k: Samples| {
        let mut out = [0.0f32];
        m.reader().read(k, &mut Memory(&data), 0, &mut out);
        out[0]
    };
    for stretch in [1.0, 0.5, 2.0, 1.3, 0.77] {
        let whole = clip(300, 600, stretch);
        let mut piece = whole.clone();
        piece.trim_start_to(170);
        let (w, p) = (StreamMap::of(&whole).unwrap(), StreamMap::of(&piece).unwrap());
        for k in 0..piece.length {
            assert_eq!(read(&p, k).to_bits(), read(&w, k + 170).to_bits(), "stretch {stretch}, clip time {k}");
        }
        if stretch == 1.0 {
            assert_eq!((p.offset, p.anchor), (470, 0), "the canonical form at rate 1");
        } else {
            assert_eq!((p.offset, p.anchor), (300, -170));
            assert_ne!(p.fingerprint(), StreamMap { length: p.length, ..w }.fingerprint(), "the anchor is in the fingerprint");
        }
    }
}
