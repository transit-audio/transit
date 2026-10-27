use super::*;
use crate::edit_window::test_support::*;

#[test]
fn a_file_time_stamp_at_the_end_of_the_range_draws() {
    let mut engine = transit_engine::demo::demo_engine();
    let s = engine.session_mut();
    s.edit.flags.insert("view.clip.time.original".into());
    s.sources.iter_mut().for_each(|src| src.time_reference = i64::MAX as u64);
    harness_with(engine).run_steps(2);
}

#[test]
fn fade_curve_sampling_keeps_steep_bends_below_a_quarter_pixel_error() {
    for (width, height) in [(420.0, 180.0), (75.0, 320.0), (10_000.0, 80.0)] {
        let body = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
        for scale in [1.0, 2.0, 4.0] {
            for shape in transit_model::FadeShape::ALL {
                for curvature in [-1.0, 1.0] {
                    for fade_in in [true, false] {
                        let fade = transit_model::Fade { len: 1000, shape, curvature };
                        let points = fade_curve_points(fade, fade_in, 0.0, width, body, scale);
                        assert!(points.len() > 17 && points.len() <= 4097);
                        assert_eq!((points.len() - 1) % 2, 0);
                        let segments = (points.len() - 1) as f32;
                        for (i, pair) in points.windows(2).enumerate() {
                            let line = pair[1] - pair[0];
                            for fraction in [0.25, 0.5, 0.75] {
                                let t = (i as f32 + fraction) / segments;
                                let actual = pos2(width * t, height - height * fade.gain(if fade_in { t } else { 1.0 - t }));
                                let along = ((actual - pair[0]).dot(line) / line.length_sq()).clamp(0.0, 1.0);
                                let distance = actual.distance(pair[0] + line * along) * scale;
                                assert!(
                                    distance < 0.25,
                                    "{width}x{height}, scale={scale}, {shape:?}, curve={curvature}, side={fade_in}, error={distance}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    let body = Rect::from_min_size(Pos2::ZERO, vec2(1e20, 1e20));
    for scale in [f32::NAN, f32::INFINITY, 1.0] {
        let points = fade_curve_points(transit_model::Fade::default(), true, 0.0, 1e20, body, scale);
        assert!(points.len() <= 4097);
    }
}
