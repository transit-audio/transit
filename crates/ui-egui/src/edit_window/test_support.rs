use super::*;
use egui_kittest::Harness;

pub(super) fn harness(n: usize) -> Harness<'static, SoundApp> {
    let mut engine = transit_engine::Engine::default();
    engine.execute("track.new", &json!({"count": n})).unwrap();
    harness_with(engine)
}

pub(super) fn harness_with(engine: transit_engine::Engine) -> Harness<'static, SoundApp> {
    let app = SoundApp::new(engine, None, crate::Services::default());
    let builder = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0);
    let builder = if std::env::var_os("SOUNDCRAFT_FADE_SHOTS").is_some() { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.input_mut().max_texture_side = Some(8192);
    h.run_steps(4);
    h
}

pub(super) fn smart_demo() -> Harness<'static, SoundApp> {
    let mut engine = transit_engine::demo::demo_engine();
    engine.execute("edit.tool", &json!({"tool": "smart"})).unwrap();
    engine.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
    harness_with(engine)
}

pub(super) fn row(h: &Harness<SoundApp>, i: usize) -> Rect {
    rect(h.state().edit_layout.rows[i].1)
}

pub(super) fn row_ids(h: &Harness<SoundApp>) -> Vec<TrackId> {
    h.state().edit_layout.rows.iter().map(|(i, _)| TrackId(*i)).collect()
}

pub(super) fn timeline(h: &Harness<SoundApp>) -> Rect {
    rect(h.state().edit_layout.timeline)
}

pub(super) fn lane_point(h: &Harness<SoundApp>, i: usize) -> Pos2 {
    pos2(timeline(h).min.x + 100.0, row(h, i).min.y + 10.0)
}

pub(super) fn clip_on_screen(h: &Harness<SoundApp>, ok: impl Fn(f32, f32) -> bool) -> (Clip, Rect) {
    let (s, tl) = (h.state().engine.session(), timeline(h));
    h.state()
        .edit_layout
        .rows
        .iter()
        .filter_map(|(id, r)| s.track(TrackId(*id)).map(|t| (t, r)))
        .find_map(|(t, r)| {
            let c = t.clips().iter().find(|c| ok(x_of(s, tl, c.start), x_of(s, tl, c.end())))?;
            Some((c.clone(), lane_rect(t, rect(*r), tl)))
        })
        .expect("no such clip on screen")
}

pub(super) fn start_drag(h: &mut Harness<SoundApp>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.hover_at(at + vec2(20.0, 0.0));
    h.step();
}

pub(super) fn drag(h: &mut Harness<SoundApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.drag_at(from);
    for k in 1..=8 {
        h.hover_at(from + (to - from) * (k as f32 / 8.0));
    }
    h.drop_at(to);
    h.step();
}

pub(super) fn quick_drag(h: &mut Harness<SoundApp>, at: Pos2, to: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.hover_at(at + (to - at) * 0.75);
    h.hover_at(to);
    h.drop_at(to);
    h.step();
}

pub(super) fn click(h: &mut Harness<SoundApp>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.drop_at(at);
    h.step();
}

pub(super) fn click_with(h: &mut Harness<SoundApp>, at: Pos2, mods: egui::Modifiers) {
    h.event(egui::Event::ModifiersChanged(mods));
    click(h, at);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
}

pub(super) fn painted_rows(h: &Harness<SoundApp>, y_of: impl Fn(&Shape) -> Option<f32>) -> Vec<usize> {
    let rows = &h.state().edit_layout.rows;
    let mut out: Vec<usize> =
        h.output().shapes.iter().filter_map(|c| y_of(&c.shape)).filter_map(|y| rows.iter().position(|(_, r)| (r[1]..=r[3]).contains(&y))).collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub(super) fn highlighted_rows(h: &Harness<SoundApp>) -> Vec<usize> {
    painted_rows(h, |shape| match shape {
        Shape::Rect(r) if r.fill == Tokens::current().selection => Some(r.rect.center().y),
        _ => None,
    })
}

pub(super) fn insertion_rows(h: &Harness<SoundApp>) -> Vec<usize> {
    painted_rows(h, |shape| match shape {
        Shape::LineSegment { points, stroke } if stroke.color == Tokens::current().insertion => Some((points[0].y + points[1].y) / 2.0),
        _ => None,
    })
}
