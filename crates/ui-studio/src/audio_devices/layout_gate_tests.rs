//! The dialog layout gate (#735) on Audio Devices: on ASIO with the longest status the card can
//! hold (input open, the backend figures, output open, the hover hint and many notes), every page
//! of the window keeps its buttons whole at 1x and 1.5x, on a tall and on a short screen.
use super::window_tests::SECTIONS;
use super::*;
use crate::layout_gate;

fn long_status_rig(zoom: f32, height: f32, notes: usize) -> T {
    use transit_audio_backend as ab;
    use transit_playback::telemetry::StreamStats;
    let driver = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32))
        .named("ASIO", "Focusrite USB ASIO With A Very Long Driver Name For The Window")
        .with_capture(2, true)
        .with_fixed_buffer(128);
    let again = driver.clone();
    let mut t = asio_rig(&driver, move |session, cfg| transit_playback::Player::open_fake_native_asio(session, cfg, &again).unwrap());
    t.app.diag.machine = Some(crate::diagnostics::Machine { os: "Test OS".into(), cores: 4, ..Default::default() });
    layout_gate::watch(&t.ctx, layout_gate::window("Audio Devices"));
    t.app.ui.ui_zoom = zoom;
    t.screen = egui::vec2(1920.0, height) / zoom;
    t.frames(3);
    assert_eq!(t.ctx.pixels_per_point(), zoom);
    let mut hist = [0u64; 16];
    hist[0] = 90;
    hist[5] = 40;
    let output = StreamStats {
        wakes: 1200,
        wake_late_us: hist,
        max_wake_late_us: 12_345,
        service_us: hist,
        max_service_us: 23_456,
        busy_skips: 2,
        overloads: 1,
        ..Default::default()
    };
    let input = StreamStats { wakes: 1200, wake_late_us: hist, max_wake_late_us: 8_765, ..Default::default() };
    t.app.audio_devices.injected_backend = Some((Some(output), Some(input)));
    t.app.ui.audio.buffer_size = Some(256);
    t.app.audio_devices.notes =
        (0..notes).map(|i| format!("A fallback note, number {i}, that says what was tried and why, long enough to wrap")).collect();
    t.app.engine.set_io_map(transit_model::IoMap::for_device("ASIO", "TASCAM Model 12", 12, 10));
    t.open_window();
    t.frames(3);
    t
}

#[test]
fn every_page_of_audio_devices_keeps_its_buttons_whole_with_the_longest_status() {
    let mut failures = Vec::new();
    for zoom in layout_gate::SCALES {
        for (height, notes) in [(1080.0_f32, 2), (500.0, 12)] {
            let mut t = long_status_rig(zoom, height, notes);
            for key in SECTIONS {
                t.section(key);
                t.frames(2);
                let found = layout_gate::findings(&t.ctx, layout_gate::window("Audio Devices"));
                if !found.is_empty() {
                    failures.push(format!("{key} at {zoom} ppp, {height} high, {notes} notes: {found:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
