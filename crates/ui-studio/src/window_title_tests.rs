//! Internal window title bars (#206): short, still closing and dragging, and no window built
//! outside `theme::window`.

use crate::theme;

const BODY_H: f32 = 40.0;

struct W {
    ctx: egui::Context,
    time: f64,
    open: bool,
    pos: egui::Pos2,
    rect: egui::Rect,
}

impl W {
    fn new(scale: f32) -> W {
        let ctx = egui::Context::default();
        theme::apply(&ctx, theme::Appearance::Dark);
        ctx.set_pixels_per_point(scale);
        W { ctx, time: 0.0, open: true, pos: egui::pos2(300.0, 200.0), rect: egui::Rect::NOTHING }
    }

    fn frame(&mut self, events: Vec<egui::Event>, themed: bool) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let (open, pos) = (&mut self.open, self.pos);
        let mut rect = self.rect;
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            let w = if themed { theme::window(&ctx, "Callback Telemetry") } else { plain("Callback Telemetry") };
            if let Some(r) = w.open(open).collapsible(false).resizable(false).default_pos(pos).show(&ctx, |ui| {
                ui.allocate_space(egui::vec2(200.0, BODY_H));
            }) {
                rect = r.response.rect;
            }
        });
        out.textures_delta.clear();
        self.rect = rect;
    }

    fn settle(&mut self, themed: bool) {
        for _ in 0..6 {
            self.frame(Vec::new(), themed);
        }
    }

    fn pointer(&mut self, at: egui::Pos2, pressed: Option<bool>) {
        let mut ev = vec![egui::Event::PointerMoved(at)];
        if let Some(down) = pressed {
            ev.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: down, modifiers: Default::default() });
        }
        self.frame(ev, true);
    }
}

/// egui's own constructor, spelled so the source scan does not see it: the "before" window.
fn plain(title: &str) -> egui::Window<'static> {
    egui::Window::new(title.to_string())
}

/// The title bar's height: the window minus the body and the content margin (8 px each side).
fn bar_height(rect: egui::Rect) -> f32 {
    rect.height() - BODY_H - 2.0 * theme::SPACE
}

#[test]
fn the_title_bar_is_short_at_one_and_one_and_a_half_scale() {
    for scale in [1.0_f32, 1.5] {
        let mut before = W::new(scale);
        before.settle(false);
        let mut after = W::new(scale);
        after.settle(true);
        let (old, new) = (bar_height(before.rect), bar_height(after.rect));
        println!("scale {scale}: title bar {old:.1} px before, {new:.1} px after");
        assert!(new <= 24.5, "scale {scale}: the title bar is {new} px (bound 24.5)");
        assert!(new + 10.0 <= old, "scale {scale}: {new} is much shorter than the heading-sized {old}");
        assert!((after.rect.width() - before.rect.width()).abs() < 1.0, "the content margin is unchanged: {:?} vs {:?}", after.rect, before.rect);
    }
}

#[test]
fn the_title_bar_still_drags_and_closes_the_window() {
    for scale in [1.0_f32, 1.5] {
        let mut w = W::new(scale);
        w.settle(true);
        let bar = bar_height(w.rect);
        let y = w.rect.top() + bar / 2.0;
        let start = w.rect.min;
        // Drag by the bar, left of the close button.
        let grab = egui::pos2(w.rect.left() + 20.0, y);
        w.pointer(grab, None);
        w.pointer(grab, Some(true));
        w.pointer(grab + egui::vec2(60.0, 40.0), None);
        w.pointer(grab + egui::vec2(60.0, 40.0), Some(false));
        assert!(w.rect.min.x >= start.x + 40.0 && w.rect.min.y >= start.y + 25.0, "scale {scale}: dragged from {start:?} to {:?}", w.rect.min);
        // Close by the button at the right end of the bar.
        let close = egui::pos2(w.rect.right() - theme::SPACE - 6.0, w.rect.top() + bar / 2.0);
        w.pointer(close, None);
        w.pointer(close, Some(true));
        w.pointer(close, Some(false));
        w.settle(true);
        assert!(!w.open, "scale {scale}: the close button closes the window");
    }
}

#[test]
fn every_window_goes_through_the_theme_helper() {
    fn scan(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("window_title_tests.rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    let ours = path.ends_with("theme.rs") && code.contains("egui::Window::new(text)");
                    if code.contains("egui::Window::new") && !ours {
                        found.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
    assert!(found.is_empty(), "build windows with theme::window so they get the window-title style: {found:?}");
}

/// The close button's hit height: the largest distance above the bar's middle that still closes.
#[test]
fn the_close_button_hit_area_is_at_least_the_close_size_and_the_bar_does_not_grow() {
    for scale in [1.0_f32, 1.5] {
        let mut probe = W::new(scale);
        probe.settle(true);
        let bar = bar_height(probe.rect);
        assert!(bar <= 24.5, "scale {scale}: the bar is still {bar} px");
        let hits = |dy: f32| {
            let mut w = W::new(scale);
            w.settle(true);
            let at = egui::pos2(w.rect.right() - theme::SPACE - 6.0, w.rect.top() + bar / 2.0 + dy);
            w.pointer(at, None);
            w.pointer(at, Some(true));
            w.pointer(at, Some(false));
            w.settle(true);
            !w.open
        };
        let half = theme::WINDOW_CLOSE_SIZE / 2.0 - 1.0;
        assert!(
            hits(-half) && hits(half),
            "scale {scale}: a click {half} px off the middle closes (hit height at least {})",
            theme::WINDOW_CLOSE_SIZE - 2.0
        );
        assert!(!hits(theme::WINDOW_CLOSE_SIZE), "scale {scale}: a click well below the button does not close");
        println!("scale {scale}: close hit height >= {} px, bar {bar:.1} px", theme::WINDOW_CLOSE_SIZE - 2.0);
    }
}
