//! The strips lane's floating scroll bar (#414).

use crate::mixer::{STRIP_W, StripLayout};
use crate::tests::*;
use crate::tokens::chrome;

/// The scrolling lane, from the layout: the area less its 4 pt margin, ending 8 pt before the
/// pinned main bus.
fn lane(app: &StudioApp) -> [f32; 4] {
    let (a, m) = (app.mixer.layout.area, app.mixer.layout.master.clone().unwrap().rect);
    [a[0] + 4.0, a[1] + 4.0, m[0] - 8.0, a[3] - 4.0]
}

fn overflowing(n: usize) -> (StudioApp, egui::Context, f64) {
    let (mut app, ctx, mut time, _) = mixer_app();
    for _ in 1..n {
        app.run("track.new", json!({"kind": "audio"})).unwrap();
    }
    app.run("track.new", json!({"kind": "master"})).unwrap();
    frames(&mut app, &ctx, 4, &mut time);
    (app, ctx, time)
}

fn check_bar(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, n: usize) {
    let l = lane(app);
    let area = app.mixer.layout.area;
    let max = {
        let strips = n as f32 * (STRIP_W + 4.0) + 110.0;
        strips - (l[2] - l[0])
    };
    assert!(max > 0.0, "the strips overflow the lane");
    // The band under the strips the maintainer points at: a channel strip's panel ends 19 pt
    // above the lane's bottom (the main bus footer's room, 6 + 13, which only the main bus
    // fills), then the pane's own 4 pt margin. The floating bar needs no room of its own: it
    // sits in that empty band, 8 pt at most, clear of every strip panel.
    let first: &StripLayout = &app.mixer.layout.strips[0];
    assert_eq!(l[3] - first.rect[3], 19.0);
    assert_eq!(area[3] - l[3], 4.0);
    let bar = app.mixer.layout.lane_bar.clone().expect("overflowing strips paint a scroll bar");
    let track = l[2] - l[0] - 2.0 * chrome::SCROLL_INSET;
    let content = max + (l[2] - l[0]);
    let handle = track * (l[2] - l[0]) / content;
    let t = bar.thumb;
    assert!((t[2] - t[0] - handle).abs() < 0.01, "handle {} vs {handle}", t[2] - t[0]);
    assert_eq!(t[0], l[0] + chrome::SCROLL_INSET, "at rest the handle starts at the lane's left inset");
    assert!(t[0] >= l[0] && t[2] <= l[2], "inside the lane, never over the main bus");
    assert_eq!((t[1], t[3]), (l[3] - chrome::SCROLL_REST, l[3]));
    assert!(t[1] >= first.rect[3], "the handle lies below the strip panels, over nothing");
    assert_eq!(bar.thumb_width, chrome::SCROLL_REST);
    assert_eq!(app.mixer.hscroll, 0.0);
    // Dragging the handle by 40 points scrolls by the proportional amount and the handle follows.
    let from = mid(t);
    hold_drag(app, ctx, time, from, (from.0 + 40.0, from.1), egui::Modifiers::NONE);
    let held = app.mixer.layout.lane_bar.clone().unwrap();
    assert_eq!(held.thumb_width, chrome::SCROLL_HOVER, "dragging widens the handle");
    release_at(app, ctx, time, (from.0 + 40.0, from.1), egui::Modifiers::NONE);
    let want = 40.0 * max / (track - handle);
    assert!((app.mixer.hscroll - want).abs() < 0.5, "scrolled {} wanted {want}", app.mixer.hscroll);
    let moved = app.mixer.layout.lane_bar.clone().unwrap().thumb;
    assert!((moved[0] - t[0] - 40.0).abs() < 0.5, "the handle followed: {} to {}", t[0], moved[0]);
    // Scrolled to the end, the handle ends at the lane's right inset.
    app.mixer.hscroll = f32::MAX;
    frames(app, ctx, 3, time);
    let end = app.mixer.layout.lane_bar.clone().unwrap().thumb;
    assert!((end[2] - (l[2] - chrome::SCROLL_INSET)).abs() < 0.01, "{end:?}");
}

#[test]
fn docked_mixer_paints_a_horizontal_scroll_bar_when_the_strips_overflow() {
    let (mut app, ctx, mut time) = overflowing(22);
    check_bar(&mut app, &ctx, &mut time, 22);
}

#[test]
fn the_full_mixer_window_scrolls_the_same_way_and_paints_the_same_bar() {
    let (mut app, ctx, mut time) = overflowing(22);
    full_mixer(&mut app, &ctx, &mut time);
    check_bar(&mut app, &ctx, &mut time, 22);
}

#[test]
fn strips_that_fit_paint_no_scroll_bar() {
    let (mut app, ctx, mut time, _) = mixer_app();
    assert!(app.mixer.layout.lane_bar.is_none());
    for _ in 0..3 {
        app.run("track.new", json!({"kind": "audio"})).unwrap();
    }
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.mixer.layout.lane_bar.is_none());
}
