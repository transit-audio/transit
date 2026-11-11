//! The bench's viewport: source frames at the source's own rate, at every zoom (ADR 0041).

use super::state::{Loop, Mode};
use super::viewport::{Viewport, ms_from_start};

/// A loop of 44,100 frames at 44.1 kHz starting at frame 1,000 (one second; S at 1,000).
fn lp() -> Loop {
    Loop { source: None, start: 1_000, end: 45_100, rate: 44_100, mode: Mode::Auto, snap: true, auto: Vec::new(), manual: Vec::new() }
}

#[test]
fn zoom_keeps_the_selected_slice_centered_and_stays_inside_the_loop() {
    let l = lp();
    let mut v = Viewport::default();
    assert_eq!(v.range(&l), (1_000.0, 45_100.0), "1× shows the loop");
    v.select(20_000, 22_000);
    v.zoom_to(2);
    let (a, b) = v.range(&l);
    assert_eq!(b - a, 44_100.0 / 4.0);
    assert_eq!((a + b) / 2.0, 21_000.0, "4× centers the selected slice");
    v.zoom_to(3);
    let (a, b) = v.range(&l);
    assert_eq!((a + b) / 2.0, 21_000.0, "8× keeps the same center");
    // A slice at the loop's start: the view stops at S, never before it.
    v.select(1_000, 2_000);
    assert_eq!(v.range(&l).0, 1_000.0);
    // Back to 1×: the whole loop, whatever the center.
    v.zoom_to(0);
    assert_eq!(v.range(&l), (1_000.0, 45_100.0));
}

#[test]
fn scrolling_keeps_the_view_inside_the_loop_and_the_next_zoom_keeps_its_center() {
    let l = lp();
    let mut v = Viewport { zoom: 1, center: None };
    v.scroll_to(&l, 100_000.0);
    assert_eq!(v.range(&l), (45_100.0 - 22_050.0, 45_100.0));
    v.scroll_to(&l, 10_000.0);
    let center = 10_000.0 + 22_050.0 / 2.0;
    v.zoom_to(2);
    let (a, b) = v.range(&l);
    assert_eq!((a + b) / 2.0, center, "the center source position is preserved");
}

#[test]
fn the_pointer_reads_the_same_frame_and_milliseconds_at_1x_and_8x() {
    let l = lp();
    let (left, width) = (10.0, 900.0);
    let frame = 23_050u64;
    let one = Viewport::default();
    let mut eight = Viewport { zoom: 3, center: None };
    eight.select(frame, frame);
    for v in [one, eight] {
        let x = v.x_of(&l, frame as f64, left, width);
        assert_eq!(v.frame_at(&l, x, left, width), frame, "{v:?}");
    }
    // Milliseconds from S at the source's rate (22,050 frames at 44.1 kHz), not the session's.
    assert_eq!(ms_from_start(&l, frame), 500.0);
    assert_eq!(ms_from_start(&l, l.start), 0.0);
}

#[test]
fn the_waveform_waits_for_the_sources_peaks() {
    assert!(super::wave::columns(None, 0.0, 100.0, 10).is_none(), "no audio yet: nothing drawn, Analyzing…");
    let audio =
        transit_model::SourceAudio::new(transit_audio_io::AudioBuffer { sample_rate: 44_100, channels: vec![vec![0.5; 100], vec![-0.25; 100]] });
    let cols = super::wave::columns(Some(&audio), 0.0, 100.0, 10).unwrap();
    assert_eq!(cols.len(), 10);
    assert_eq!(cols[3], Some((-0.25, 0.5)), "channels overlaid, never summed");
}
