use super::*;
use crate::edit_window::test_support::*;
use egui_kittest::Harness;

#[test]
fn a_redrawn_frame_counts_once_in_the_frame_time() {
    let mut h = harness(4);
    let mut t = 1.0;
    let mut frame = |h: &mut Harness<SoundApp>| {
        t += 1.0 / 60.0;
        h.input_mut().time = Some(t);
        h.step();
    };
    for _ in 0..120 {
        frame(&mut h);
    }
    let button = |pos: Pos2, pressed: bool| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for k in 0..30 {
        let at = lane_point(&h, k % 4) + vec2(k as f32 * 7.0, 0.0);
        h.hover_at(at);
        frame(&mut h);
        h.event(button(at, true));
        frame(&mut h);
        h.event(button(at, false));
        frame(&mut h);
    }
    let ms = h.state().frame_ms;
    assert!((ms - 1000.0 / 60.0).abs() < 0.5, "{ms} ms/frame");
}
