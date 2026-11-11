//! The bench's playheads (#470) from what the instrument publishes through its tap (a fake tap
//! here: no player, no clock): a line at the voice's source frame at 1× and zoomed and scrolled at
//! 1.5 pixels per point, in the `waveform/playhead` ink; none for a voice of another source, off
//! the view, of a bypassed Sampler or while nothing sounds, and no repaint asked for while idle.

use super::rig::{bench, sliced};
use crate::sampler_panel::test_rig::T;
use serde_json::json;
use transit_dsp::VoicePos;
use transit_model::TrackId;
use transit_model::sampler_loops::effective_region;

/// The zone slots that play the loop's source, in zone order.
fn loop_zones(t: &T, tr: TrackId) -> Vec<u16> {
    let p = t.patch(tr);
    let source = p.slicing.as_ref().unwrap().source;
    p.zones.iter().filter(|z| effective_region(z, &p).source == source).map(|z| z.id).collect()
}

/// Publishes `voices` (zone slot, source frame) as the render would, then draws a frame.
fn sound(t: &mut T, tap: &transit_mix::ScopeTap, voices: &[(u16, u64)]) {
    let words: Vec<u64> = voices.iter().map(|(z, f)| VoicePos::new(*z, *f as f64).0).collect();
    tap.push_positions(&words);
    t.frames(1);
}

/// The well's inside: (left, width).
fn inside(t: &T, tr: TrackId) -> (f32, f32) {
    let w = bench(t, tr).well;
    (w[0] + 1.0, w[2] - w[0] - 2.0)
}

/// A kit with a second source on pad 10 (zone 10), beside the sliced loop.
fn with_other_source() -> (T, TrackId, u16) {
    let (mut t, tr) = sliced();
    let other = t.sample_at("Other.wav", 44_100, 44_100);
    t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": 10, "pad": 10, "path": other.to_string_lossy()}));
    let zone = 9;
    assert!(!loop_zones(&t, tr).contains(&zone), "zone 10 plays another source");
    (t, tr, zone)
}

#[test]
fn a_voice_of_the_loop_draws_a_line_at_its_frame_and_a_voice_of_another_source_none() {
    let (mut t, tr, other) = with_other_source();
    let tap = t.tap(tr);
    let zones = loop_zones(&t, tr);
    assert_eq!(zones.len(), 4, "the four slices' zones");
    // Slice 2's voice a quarter of the way in, and the other source's voice at the middle.
    sound(&mut t, &tap, &[(zones[1], 11_025), (other, 22_050)]);
    let b = bench(&t, tr);
    let (left, width) = inside(&t, tr);
    assert_eq!(b.playheads.len(), 1, "only the loop's voice: {:?}", b.playheads);
    let line = b.playheads[0];
    assert!((line[0] - (left + 0.25 * width)).abs() < 0.01, "{line:?} at a quarter of {left} + {width}");
    assert!((line[2] - line[0] - 1.0).abs() < 0.01, "a 1-point line");
    assert!((line[1] - b.strip[3]).abs() < 0.01 && (line[3] - (b.well[3] - 1.0)).abs() < 0.01, "across the waveform under the strip: {line:?}");
    assert_eq!(b.playhead_ink, crate::theme::TOKENS.text, "waveform/playhead is text");
    assert!(b.playhead_repaint);
    // Two voices of the loop: two lines.
    sound(&mut t, &tap, &[(zones[0], 0), (zones[3], 33_075)]);
    let xs: Vec<f32> = bench(&t, tr).playheads.iter().map(|l| l[0]).collect();
    assert_eq!(xs.len(), 2);
    assert!((xs[0] - left).abs() < 0.01 && (xs[1] - (left + 0.75 * width)).abs() < 0.01, "{xs:?}");
}

#[test]
fn zoomed_and_scrolled_at_1_5_pixels_per_point_the_line_follows_the_view_and_a_voice_off_it_draws_none() {
    let (mut t, tr) = sliced();
    t.ctx.set_pixels_per_point(1.5);
    t.frames(2);
    let tap = t.tap(tr);
    let zones = loop_zones(&t, tr);
    // 4×: a quarter of the loop (11,025 frames) from frame 22,050.
    {
        let v = &mut t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().slices;
        v.viewport.zoom_to(2);
        let l = v.snapshot.data.clone().unwrap();
        v.viewport.scroll_to(&l, 22_050.0);
    }
    t.frames(1);
    assert_eq!(bench(&t, tr).view, (22_050.0, 33_075.0));
    // 2,205 frames into the view (a fifth of it); the other voice is before the view.
    sound(&mut t, &tap, &[(zones[2], 24_255), (zones[0], 11_025)]);
    let b = bench(&t, tr);
    let (left, width) = inside(&t, tr);
    assert_eq!(b.playheads.len(), 1, "the voice off the view draws nothing: {:?}", b.playheads);
    assert!((b.playheads[0][0] - (left + 0.2 * width)).abs() < 0.01, "{:?} at a fifth of {left} + {width}", b.playheads[0]);
    assert!(b.playhead_repaint, "a voice of the loop sounds, in view or not");
    assert_eq!(bench(&t, tr).view, (22_050.0, 33_075.0), "the view does not scroll to follow");
    // Only the voice off the view: nothing drawn, still repainting while it sounds.
    sound(&mut t, &tap, &[(zones[0], 11_100)]);
    let b = bench(&t, tr);
    assert!(b.playheads.is_empty() && b.playhead_repaint);
}

#[test]
fn a_bypassed_sampler_shows_no_playhead() {
    let (mut t, tr) = sliced();
    let tap = t.tap(tr);
    let zones = loop_zones(&t, tr);
    sound(&mut t, &tap, &[(zones[1], 11_025)]);
    assert_eq!(bench(&t, tr).playheads.len(), 1, "negative control: it shows while not bypassed");
    t.app.engine.session_mut().track_mut(tr).unwrap().instrument.as_mut().unwrap().bypass = true;
    sound(&mut t, &tap, &[(zones[1], 11_100)]);
    let b = bench(&t, tr);
    assert!(b.playheads.is_empty(), "{:?}", b.playheads);
    assert!(!b.playhead_repaint, "nothing moves, so no repaint");
}

#[test]
fn nothing_sounding_draws_nothing_and_asks_no_repaint() {
    let (mut t, tr) = sliced();
    let tap = t.tap(tr);
    t.frames(2);
    let b = bench(&t, tr);
    assert!(b.playheads.is_empty() && !b.playhead_repaint, "idle");
    let zones = loop_zones(&t, tr);
    sound(&mut t, &tap, &[(zones[0], 100)]);
    assert!(bench(&t, tr).playhead_repaint, "negative control: a sounding voice asks for frames");
    // The voice ended: its slot empties, the line goes and the repaints stop.
    sound(&mut t, &tap, &[]);
    let b = bench(&t, tr);
    assert!(b.playheads.is_empty() && !b.playhead_repaint);
}
