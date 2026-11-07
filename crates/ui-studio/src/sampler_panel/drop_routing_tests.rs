//! One drop router (#368): a file dropped from the file manager goes to exactly one claimant, the
//! editor window on top at the drop's position or else the arrangement. The drop's position is the
//! pointer's first report after the drop, never the stale position egui held during the OS drag.

use super::test_rig::{T, mid};
use transit_model::TrackId;

/// A point on the arrangement's timeline that no editor window covers.
fn arrangement_point(t: &T, tr: TrackId) -> (f32, f32) {
    let tl = t.app.arrange.layout.timeline;
    let w = t.window(tr).rect;
    let inside = |p: (f32, f32)| p.0 >= w[0] && p.0 <= w[2] && p.1 >= w[1] && p.1 <= w[3];
    let candidates = [(tl[0] + 40.0, tl[1] + 12.0), (tl[2] - 40.0, tl[1] + 12.0), (tl[0] + 40.0, tl[3] - 12.0), (tl[2] - 40.0, tl[3] - 12.0)];
    *candidates.iter().find(|p| !inside(**p) && tl[3] - tl[1] > 24.0).expect("a visible stretch of timeline outside the editor window")
}

#[test]
fn a_file_dropped_on_a_pad_binds_the_pad_and_imports_no_clip() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    t.drop_file(&snare, t.pad_at(tr, 3));
    assert_eq!(t.patch(tr).pad(3).zone, Some(0), "the pad is bound");
    assert_eq!(t.clip_count(), clips, "no arrangement clip");
    // Negative control: the binary's old path (every drop to the arrangement import) adds a clip.
    crate::drops::open_path(&mut t.app, &snare.to_string_lossy());
    t.frames(3);
    assert_eq!(t.clip_count(), clips + 1, "the arrangement import is what the router kept out");
}

#[test]
fn a_file_dropped_on_the_arrangement_imports_a_clip_and_binds_no_pad() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    let at = arrangement_point(&t, tr);
    t.drop_file(&snare, at);
    assert_eq!(t.clip_count(), clips + 1, "one clip imported");
    assert!(t.patch(tr).zones.is_empty(), "no pad bound");
}

#[test]
fn a_file_dropped_on_the_title_bar_or_the_strip_imports_nothing() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    let before = t.undo_len();
    let w = t.window(tr).rect;
    t.drop_file(&snare, ((w[0] + w[2]) / 2.0, w[1] + 10.0));
    t.drop_file(&snare, mid(t.panel(tr).strip.panel));
    assert_eq!(t.clip_count(), clips, "the window refused: no arrangement clip");
    assert!(t.patch(tr).zones.is_empty(), "and no pad");
    assert_eq!(t.undo_len(), before);
    // Negative control: the same file on a pad is taken.
    t.drop_file(&snare, t.pad_at(tr, 0));
    assert_eq!(t.patch(tr).pad(0).zone, Some(0));
}

#[test]
fn a_drop_with_a_stale_pointer_waits_for_the_pointer_and_lands_on_the_pad_under_it() {
    let (mut t, tr) = T::open();
    let snare = t.sample("Snare.wav", 2400);
    let clips = t.clip_count();
    // egui's pointer was last over the arrangement; the OS drag reports no motion.
    let arrangement = arrangement_point(&t, tr);
    t.point(arrangement);
    t.drop_unplaced(&snare);
    // Negative control: routing at the stale position would have imported a clip. The drop is held.
    assert_eq!(t.clip_count(), clips, "nothing routed at the stale position");
    assert!(t.patch(tr).zones.is_empty());
    // The pointer's first report after the drop places it: on pad A2.
    t.point(t.pad_at(tr, 1));
    assert_eq!(t.patch(tr).pad(1).zone, Some(0), "the pad under the pointer is bound");
    assert_eq!(t.clip_count(), clips, "and no clip was imported");
    // Taken once: more motion routes nothing again.
    t.point(arrangement);
    assert_eq!(t.clip_count(), clips);
}

#[test]
fn the_legacy_window_loads_its_one_sample_without_an_import() {
    let mut t = T::new();
    let tr = t.sampler_track();
    t.open_card(0);
    let clips = t.clip_count();
    let hit = t.sample("Hit.wav", 2400);
    t.drop_file(&hit, mid(t.panel(tr).legacy.unwrap().panel));
    assert!(t.window(tr).sample.starts_with("Hit"), "{}", t.window(tr).sample);
    assert_eq!(t.clip_count(), clips, "no arrangement clip");
    // Negative control: the same file beside the window imports and leaves the sample alone.
    let other = t.sample("Other.wav", 2400);
    let at = arrangement_point(&t, tr);
    t.drop_file(&other, at);
    assert_eq!(t.clip_count(), clips + 1);
    assert!(t.window(tr).sample.starts_with("Hit"));
}
