//! The port against the oracle over the fixed sweep (ADR 0033 gate, #241): every point of every
//! quality, bit for bit (the floor is exact: `transit-macro-osc-oracle`'s
//! `tests/texture_self_floor.rs`). A failure names the quality and the point; it is a defect to
//! hunt, never a run to repeat.

#![forbid(unsafe_code)]

mod common;

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::texture::Media;
use transit_macro_osc_oracle::texture_sweep::{Gate, Point, Quality, input_signal, longest_point_frames, points, render_point};

/// The points that render silence by design: DENSITY 0.5 (between 0.48 and 0.52 only taps seed
/// grains, and neither the latched button nor freeze taps) in the granular sweep, in the free and
/// the freeze patterns, at every quality.
fn silent_by_design(point: &Point) -> bool {
    #[allow(clippy::float_cmp)]
    let noon = point.base.density == 0.5;
    !point.delay() && noon && point.gate != Gate::Seed
}

fn null_quality(media: Media, mono: bool) {
    let quality = Quality { media, mono };
    let input = input_signal(longest_point_frames());
    let mut failures = Vec::new();
    let mut silent_mismatch = Vec::new();
    for point in points() {
        let expected = {
            let session = Session::lock();
            render_point(&session, quality, &point, &input).unwrap()
        };
        let got = common::render_point(quality, &point, &input);
        let r = common::residual(&expected, &got);
        if !r.is_exact() {
            failures.push(format!("{} {}: {r}", quality.label(), point.label()));
        }
        // From the second block on: the first block still carries the dry/wet ramp from 0.
        if common::sounds(&expected, 2 * media.block_size()) == silent_by_design(&point) {
            silent_mismatch.push(format!("{} {}", quality.label(), point.label()));
        }
    }
    assert!(failures.is_empty(), "{} of {} points differ:\n{}", failures.len(), points().len(), failures.join("\n"));
    assert!(silent_mismatch.is_empty(), "points that sound against the silent-by-design list (both directions):\n{}", silent_mismatch.join("\n"));
}

#[test]
fn digital_stereo() {
    null_quality(Media::Digital, false);
}

#[test]
fn digital_mono() {
    null_quality(Media::Digital, true);
}

#[test]
fn vintage_digital_stereo() {
    null_quality(Media::VintageDigital, false);
}

#[test]
fn vintage_digital_mono() {
    null_quality(Media::VintageDigital, true);
}

#[test]
fn clean_tape_stereo() {
    null_quality(Media::CleanTape, false);
}

#[test]
fn clean_tape_mono() {
    null_quality(Media::CleanTape, true);
}

#[test]
fn dirty_tape_stereo() {
    null_quality(Media::DirtyTape, false);
}

#[test]
fn dirty_tape_mono() {
    null_quality(Media::DirtyTape, true);
}
