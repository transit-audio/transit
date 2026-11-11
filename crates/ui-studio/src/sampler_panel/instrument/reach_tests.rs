//! The whole Instrument view is reachable on a short screen: a 1080p display at 150 percent
//! scaling leaves a window of at most 720 points, and the device window's body scrolls.

use super::super::test_rig::mid;
use super::rig::*;
use crate::tokens::layout as space;
use serde_json::json;

#[test]
fn the_last_control_is_reachable_at_720_points_tall_and_scale_1_5() {
    let (mut t, tr) = grouped();
    t.screen = egui::vec2(1400.0, 720.0);
    t.ctx.set_pixels_per_point(1.5);
    t.frames(4);
    let w = t.panel(tr).window;
    assert!(w[3] - w[1] <= 720.0 && w[1] >= 0.0, "the window fits the screen: {w:?}");
    let body = |t: &super::super::test_rig::T| {
        let p = t.panel(tr);
        (p.window[1] + space::TITLE_BAR_HEIGHT, p.footer[1])
    };
    let last = |t: &super::super::test_rig::T| layout(t, tr).bars.last().expect("the Modulation bar").bar;
    let (top, bottom) = body(&t);
    assert!(last(&t)[3] > bottom, "the view is taller than the window: the last bar starts below it");
    // Scroll with the wheel over the key map until the last bar shows (a bounded number of steps).
    let at = mid(layout(&t, tr).key_map.well);
    for _ in 0..20 {
        let r = last(&t);
        if r[1] >= top && r[3] <= bottom {
            break;
        }
        t.send("ui.scroll", json!({"x": at.0, "y": at.1.min(bottom - 10.0), "dy": -120.0}));
    }
    let r = last(&t);
    let (top, bottom) = body(&t);
    assert!(r[1] >= top && r[3] <= bottom, "the last bar {r:?} shows inside the body {top}..{bottom}");
}
