use super::{Services, SoundApp};
use serde_json::json;

#[test]
fn the_status_names_a_plugin_the_engine_took_out() {
    use transit_model::{ChannelFormat, Insert, TrackKind};
    let mut s = transit_model::Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    s.track_mut(t).unwrap().mixer.inserts[3] = Some(Insert::new("gain"));
    let msg = super::failed_plugin_status(&s, t, 3).unwrap();
    assert!(msg.starts_with("Gain on Vox put out invalid audio"), "{msg}");
    assert_eq!(super::failed_plugin_status(&s, t, 0), None);
    assert_eq!(super::failed_plugin_status(&s, t, transit_mix::INSTRUMENT_SLOT), None);
}

#[test]
fn clear_all_clip_indicators_clears_every_meter() {
    let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.meters.entry(kick).or_default().clip = true;
    app.main_meter.clip = true;
    app.run("track.clear_clip_indicators", json!({})).unwrap();
    assert!(!app.meters[&kick].clip && !app.main_meter.clip);
}

#[test]
fn preferences_saved_before_the_window_was_remembered_keep_full_scale() {
    let ui: super::UiState = serde_json::from_value(json!({"show_transport": true})).unwrap();
    assert_eq!((ui.window_size, ui.window_maximized, ui.ui_scale), ([1600.0, 1000.0], false, 1.0));
}

#[test]
fn a_window_bigger_than_its_screen_opens_maximized() {
    let maximizes = |w: f32, frames: usize| {
        let ctx = egui::Context::default();
        let info = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1000.0))),
            monitor_size: Some(egui::vec2(1728.0, 1117.0)),
            ..Default::default()
        };
        let mut maximized = false;
        for _ in 0..frames {
            let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, info.clone())].into_iter().collect(), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| super::fit_window_to_screen(ui.ctx()));
            out.textures_delta.clear();
            maximized = out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::Maximized(true));
        }
        maximized
    };
    assert!(maximizes(2400.0, 1));
    assert!(!maximizes(1600.0, 1));
    assert!(!maximizes(2400.0, 2));
}
