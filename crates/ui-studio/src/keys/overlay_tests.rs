//! A modal dialog takes the wheel, the zoom and the pointer (#244): the piano roll and the
//! arrangement behind it neither scroll nor zoom while it is up, over the dialog or over its
//! backdrop, and take the wheel again once it closes.

use super::*;

fn view(t: &T) -> (f32, f64, f32, f32, (f64, f64)) {
    (t.app.keys.layout.top_pitch, t.app.keys.layout.scroll_ticks, t.app.ui.keys.key_h, t.app.ui.keys.px_per_quarter, arrange_view(t))
}

impl T {
    fn wheel(&mut self, at: (f32, f32), p: Value) {
        self.notch(at, p);
    }

    fn pinch(&mut self, at: (f32, f32)) {
        self.input("ui.zoom", json!({"x": at.0, "y": at.1, "factor": 1.5}));
    }

    fn dialog(&mut self, open: bool) {
        self.app.about.open = open;
        self.frames(4);
    }
}

/// Points over the piano roll's grid and the arrangement's tracks, outside the dialog.
fn behind(t: &T) -> [(f32, f32); 2] {
    let (g, c) = (t.app.keys.layout.grid, t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect);
    [(g[0] + 12.0, g[1] + 12.0), (c[0] + 6.0, c[1] + 6.0)]
}

#[test]
fn the_wheel_and_the_pinch_over_a_dialog_or_its_backdrop_leave_the_views_behind_alone() {
    let mut t = T::open();
    let [roll, arr] = behind(&t);
    // The harness scrolls both views with no dialog up (so the checks below can fail).
    let start = view(&t);
    t.wheel(roll, json!({"dy": -1, "shift": true}));
    t.wheel(roll, json!({"dy": -1}));
    t.wheel(arr, json!({"dy": -1, "shift": true}));
    assert!(view(&t).1 != start.1 && view(&t).0 != start.0 && view(&t).4 != start.4, "the views take the wheel: {start:?} -> {:?}", view(&t));

    t.dialog(true);
    let w = t.app.about.window;
    assert!(w[2] > w[0], "the dialog is up");
    let inside = |p: (f32, f32)| p.0 >= w[0] && p.0 <= w[2] && p.1 >= w[1] && p.1 <= w[3];
    assert!(!inside(roll) && !inside(arr), "backdrop points: {roll:?} {arr:?} vs {w:?}");
    let before = view(&t);
    for at in [roll, arr, mid(w)] {
        t.wheel(at, json!({"dy": -1}));
        t.wheel(at, json!({"dy": 1, "shift": true}));
        t.wheel(at, json!({"dx": 1}));
        t.pinch(at);
        t.input("ui.scroll", json!({"x": at.0, "y": at.1, "dy": -1, "unit": "line", "steps": 2, "ctrl": true}));
        assert_eq!(view(&t), before, "a wheel at {at:?} moved a view behind the dialog");
    }

    t.dialog(false);
    t.wheel(roll, json!({"dy": -1, "shift": true}));
    t.wheel(arr, json!({"dy": -1, "shift": true}));
    let after = view(&t);
    assert!(after.1 != before.1 && after.4 != before.4, "the wheel reaches the views again: {before:?} -> {after:?}");
}
