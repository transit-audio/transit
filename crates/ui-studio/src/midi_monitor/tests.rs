#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::{Services, StudioApp, UiState};

fn ev(dir: Dir, status: u8, d1: u8, d2: u8) -> MonitorEvent {
    MonitorEvent::message(dir, 1, 5_000_000_000, &[status, d1, d2][..if status >= 0xF8 { 1 } else { 3 }])
}

#[test]
fn every_message_class_decodes() {
    let table: &[(MonitorEvent, Class, &str)] = &[
        (ev(Dir::In, 0x90, 60, 100), Class::Note, "Note On C3 (60) vel 100"),
        (ev(Dir::In, 0x80, 60, 64), Class::Note, "Note Off C3 (60) vel 64"),
        (ev(Dir::In, 0x93, 61, 0), Class::Note, "Note Off C#3 (61) vel 0"),
        (ev(Dir::In, 0xA0, 60, 40), Class::Touch, "Poly Aftertouch C3 (60) 40"),
        (ev(Dir::In, 0xB0, 64, 127), Class::Cc, "CC 64 Sustain 127"),
        (ev(Dir::In, 0xB0, 0, 3), Class::Cc, "CC 0 Bank Select 3"),
        (ev(Dir::In, 0xB0, 1, 9), Class::Cc, "CC 1 Mod Wheel 9"),
        (ev(Dir::In, 0xB0, 7, 100), Class::Cc, "CC 7 Volume 100"),
        (ev(Dir::In, 0xB0, 10, 64), Class::Cc, "CC 10 Pan 64"),
        (ev(Dir::In, 0xB0, 11, 1), Class::Cc, "CC 11 Expression 1"),
        (ev(Dir::In, 0xB0, 74, 20), Class::Cc, "CC 74 Brightness 20"),
        (ev(Dir::In, 0xB0, 120, 0), Class::Cc, "CC 120 All Sound Off 0"),
        (ev(Dir::In, 0xB0, 123, 0), Class::Cc, "CC 123 All Notes Off 0"),
        (ev(Dir::In, 0xB0, 127, 0), Class::Cc, "CC 127 Poly On 0"),
        (ev(Dir::In, 0xB0, 21, 5), Class::Cc, "CC 21 5"),
        (ev(Dir::In, 0xC0, 5, 0), Class::Program, "Program 5"),
        (ev(Dir::In, 0xD0, 33, 0), Class::Touch, "Channel Aftertouch 33"),
        (ev(Dir::In, 0xE0, 0, 64), Class::Touch, "Pitch Bend +0"),
        (ev(Dir::In, 0xE0, 0, 0), Class::Touch, "Pitch Bend -8192"),
        (ev(Dir::In, 0xE0, 127, 127), Class::Touch, "Pitch Bend +8191"),
        (MonitorEvent::sysex(Dir::In, 1, 0, 42), Class::SysEx, "SysEx 42 bytes"),
        (ev(Dir::In, 0xF1, 0x12, 0), Class::RealTime, "MTC Quarter Frame"),
        (ev(Dir::In, 0xF2, 0x10, 0x01), Class::RealTime, "Song Position 144"),
        (ev(Dir::In, 0xF3, 4, 0), Class::RealTime, "Song Select 4"),
        (ev(Dir::In, 0xF6, 0, 0), Class::RealTime, "Tune Request"),
        (ev(Dir::In, 0xF8, 0, 0), Class::RealTime, "Clock"),
        (ev(Dir::In, 0xFA, 0, 0), Class::RealTime, "Start"),
        (ev(Dir::In, 0xFB, 0, 0), Class::RealTime, "Continue"),
        (ev(Dir::In, 0xFC, 0, 0), Class::RealTime, "Stop"),
        (ev(Dir::In, 0xFE, 0, 0), Class::RealTime, "Active Sensing"),
        (ev(Dir::In, 0xFF, 0, 0), Class::RealTime, "System Reset"),
    ];
    for (e, class, text) in table {
        assert_eq!(describe(e), (*class, (*text).to_string()), "{e:?}");
    }
}

#[test]
fn time_formats_as_minutes_seconds_millis() {
    assert_eq!(time_text(0), "0:00.000");
    assert_eq!(time_text(61_234_000_000), "1:01.234");
    assert_eq!(time_text(7_200_000_000_000), "120:00.000");
}

fn ports() -> Vec<PortReport> {
    let p = |dir, id, name: &str, bpm, sensing| PortReport {
        state: PortState::Open,
        tracks: Vec::new(),
        dir,
        id,
        name: name.into(),
        clock_bpm: bpm,
        sensing,
    };
    vec![p(Dir::In, 1, "Keys", Some(120.04), true), p(Dir::Out, 1, "Synth OUT", None, false), p(Dir::In, 2, "Pads", None, false)]
}

fn state_with(events: &[MonitorEvent]) -> State {
    let mut s = State::default();
    s.learn_ports(&ports());
    for e in events {
        s.push(e);
    }
    s
}

fn on_port(mut e: MonitorEvent, port: u16) -> MonitorEvent {
    e.port = port;
    e
}

fn sample() -> State {
    state_with(&[
        ev(Dir::In, 0x90, 60, 100),            // Keys ch 1 note
        ev(Dir::In, 0xB1, 74, 20),             // Keys ch 2 cc
        on_port(ev(Dir::In, 0xE2, 0, 64), 2),  // Pads ch 3 pitch bend
        ev(Dir::In, 0xFA, 0, 0),               // Keys start
        ev(Dir::Out, 0x90, 60, 100),           // Synth OUT ch 1 note
        MonitorEvent::sysex(Dir::In, 2, 0, 7), // Pads sysex
        on_port(ev(Dir::In, 0xC4, 9, 0), 2),   // Pads ch 5 program
    ])
}

fn texts(s: &State, f: &Filters) -> Vec<String> {
    s.rows.iter().filter(|r| passes(f, r)).map(|r| r.text.clone()).collect()
}

#[test]
fn filter_combinations() {
    let s = sample();
    let all = Filters::default();
    assert_eq!(texts(&s, &all).len(), 7);
    let types = |notes, cc, touch, program, sysex, real_time| Filters { notes, cc, touch, program, sysex, real_time, ..all.clone() };
    assert_eq!(texts(&s, &types(true, false, false, false, false, false)).len(), 2);
    assert_eq!(texts(&s, &types(false, true, false, false, false, false)), ["CC 74 Brightness 20"]);
    assert_eq!(texts(&s, &types(false, false, true, false, false, false)), ["Pitch Bend +0"]);
    assert_eq!(texts(&s, &types(false, false, false, true, false, false)), ["Program 9"]);
    assert_eq!(texts(&s, &types(false, false, false, false, true, false)), ["SysEx 7 bytes"]);
    assert_eq!(texts(&s, &types(false, false, false, false, false, true)), ["Start"]);
    // Direction.
    assert_eq!(texts(&s, &Filters { dir: DirFilter::Out, ..all.clone() }), ["Note On C3 (60) vel 100"]);
    assert_eq!(texts(&s, &Filters { dir: DirFilter::In, ..all.clone() }).len(), 6);
    // Devices: hidden ones go, new ones stay.
    let hide = |names: &[&str]| Filters { hidden_ports: names.iter().map(|n| (*n).to_string()).collect(), ..all.clone() };
    assert_eq!(texts(&s, &hide(&["Pads", "Synth OUT"])).len(), 3);
    assert_eq!(texts(&s, &hide(&["Keys", "Pads", "Synth OUT"])).len(), 0);
    // Channels: a mask; channel-less messages go once any filter is on.
    let ch = |mask: u16| Filters { channels: mask, ..all.clone() };
    assert_eq!(texts(&s, &ch(0b10)), ["CC 74 Brightness 20"]);
    assert_eq!(texts(&s, &ch(0b1)).len(), 2);
    assert_eq!(texts(&s, &ch(0b10000)), ["Program 9"]);
    assert_eq!(texts(&s, &ch(0)).len(), 0);
    // Text matches decoded text and hex, any case.
    let text = |t: &str| Filters { text: t.into(), ..all.clone() };
    assert_eq!(texts(&s, &text("cc 74")), ["CC 74 Brightness 20"]);
    assert_eq!(texts(&s, &text("c3")).len(), 2);
    assert_eq!(texts(&s, &text("START")), ["Start"]);
    assert_eq!(texts(&s, &text("90 3c")).len(), 2, "hex bytes match too");
    assert_eq!(texts(&s, &text("  ")).len(), 7, "blank text is no filter");
    // Combined.
    let combo = Filters { dir: DirFilter::In, channels: 0b11, notes: true, cc: false, hidden_ports: vec!["Pads".into()], ..all };
    assert_eq!(texts(&s, &combo), ["Note On C3 (60) vel 100"]);
}

#[test]
fn rows_carry_raw_bytes_and_keys_for_the_highlight() {
    let s = sample();
    let hex: Vec<String> = s.rows.iter().map(Row::hex).collect();
    assert_eq!(hex, ["90 3C 64", "B1 4A 14", "E2 00 40", "FA", "90 3C 64", "F0", "C4 09"]);
    let keys: Vec<Option<u8>> = s.rows.iter().map(|r| r.key).collect();
    assert_eq!(keys, [Some(60), Some(74), None, None, Some(60), None, None]);
}

#[test]
fn spotlight_catches_the_next_input_message_only() {
    let mut s = State::default();
    s.learn_ports(&ports());
    s.push(&ev(Dir::In, 0x90, 60, 100));
    assert_eq!(s.learn, Learn::Off, "not armed: nothing is caught");
    s.learn = Learn::Armed;
    s.push(&ev(Dir::Out, 0x90, 61, 100));
    assert_eq!(s.learn, Learn::Armed, "an output message is not what the controller sent");
    s.push(&on_port(ev(Dir::In, 0xB2, 74, 99), 2));
    let Learn::Caught(row) = &s.learn else { panic!("{:?}", s.learn) };
    assert_eq!((row.port.as_str(), row.channel, row.text.as_str()), ("Pads", 2, "CC 74 Brightness 99"));
    s.push(&ev(Dir::In, 0x90, 62, 100));
    assert!(matches!(&s.learn, Learn::Caught(r) if r.key == Some(74)), "the first message stays until dismissed");
}

#[test]
fn pause_counts_the_events_behind_the_view() {
    let mut s = sample();
    assert_eq!(s.behind(), 0);
    s.paused_at = Some(s.next_seq - 1);
    s.push(&ev(Dir::In, 0x90, 70, 100));
    s.push(&ev(Dir::In, 0x90, 71, 100));
    assert_eq!(s.behind(), 2);
    assert_eq!(s.rows.len(), 9, "the ring keeps capturing while the view is frozen");
    let shown = copy_text(&s.rows, &Filters::default(), s.paused_at, 100);
    assert_eq!(shown.lines().count(), 7);
}

#[test]
fn port_header_lines() {
    let mut p = ports().remove(0);
    let tracks = vec!["Piano".to_string(), "Bass".to_string()];
    assert_eq!(port_line(&p, &tracks, 12.4, 0.5), "Keys (In) · open · tracks: Piano, Bass · 12 msg/s · Clock 120.0 BPM · Active Sensing present");
    p.clock_bpm = None;
    p.sensing = false;
    assert_eq!(port_line(&p, &[], 0.0, 3.0), "Keys (In) · open · no track listens · 0 msg/s");
    assert_eq!(port_line(&p, &[], 0.0, 10.0), "Keys (In) · open · no track listens · 0 msg/s · no data in 10 s");
    p.state = PortState::Missing;
    assert_eq!(port_line(&p, &[], 0.0, 99.0), "Keys (In) · missing (unplugged?)");
    p.state = PortState::Failed;
    assert!(port_line(&p, &[], 0.0, 0.0).contains("could not be opened"));
    let out = ports().remove(1);
    assert_eq!(port_line(&out, &[], 0.0, 99.0), "Synth OUT (Out) · open");
}

#[test]
fn silent_ports_are_flagged_after_ten_seconds() {
    let mut s = State::default();
    s.learn_ports(&ports());
    let name = |id: u64| format!("T{id}");
    s.refresh_ports(100.0, &name);
    assert!(!s.port_lines[2].contains("no data"), "{:?}", s.port_lines);
    s.refresh_ports(111.0, &name);
    // Keys has Clock and Active Sensing, so it is alive; Pads is silent.
    assert!(!s.port_lines[0].contains("no data"), "{:?}", s.port_lines);
    assert!(s.port_lines[2].contains("no data in 11 s"), "{:?}", s.port_lines);
}

#[test]
fn message_rate_closes_each_second() {
    let mut r = Rate::default();
    r.add(30);
    r.tick(0.5);
    assert_eq!(r.per_sec, 0.0, "not yet a second");
    r.tick(1.0);
    assert_eq!(r.per_sec, 30.0);
    r.tick(2.0);
    assert_eq!(r.per_sec, 0.0);
}

#[test]
fn copy_text_is_plain_aligned_lines_of_the_last_hundred_visible_rows() {
    let events: Vec<MonitorEvent> = (0..150u64)
        .map(|n| {
            let mut e = ev(Dir::In, 0x90, (n % 100) as u8, 100);
            e.host_ns = n * 1_000_000;
            e
        })
        .collect();
    let s = state_with(&events);
    let text = copy_text(&s.rows, &Filters::default(), None, COPY_LINES);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 100);
    let sp = |n: usize| " ".repeat(n);
    assert_eq!(lines[0], format!("  0:00.050  In   Keys{}1  Note On {} (50) vel 100", sp(17), soundcraft_midi::note_name(50)));
    assert_eq!(lines[99], format!("  0:00.149  In   Keys{}1  Note On {} (49) vel 100", sp(17), soundcraft_midi::note_name(49)));
    assert!(!text.ends_with('\n'));
    let s = state_with(&[ev(Dir::Out, 0x90, 60, 1), ev(Dir::In, 0xFA, 0, 0), ev(Dir::In, 0xFC, 0, 0)]);
    let all = copy_text(&s.rows, &Filters::default(), None, 100);
    assert_eq!(
        all,
        format!(
            "  0:05.000  Out  Synth OUT{}1  Note On C3 (60) vel 1\n  0:05.000  In   Keys{}--  Start\n  0:05.000  In   Keys{}--  Stop",
            sp(12),
            sp(16),
            sp(16)
        )
    );
    assert_eq!(copy_text(&s.rows, &Filters::default(), Some(1), 100).lines().count(), 2, "rows after the pause are not copied");
    assert_eq!(copy_text(&s.rows, &Filters { real_time: false, ..Filters::default() }, None, 100).lines().count(), 1);
    assert_eq!(copy_text(&s.rows, &Filters::default(), None, 1), format!("  0:05.000  In   Keys{}--  Stop", sp(16)));
}

#[test]
fn the_report_lists_ports_drops_and_the_last_lines_with_hex() {
    let s = sample();
    let lines = vec!["Keys (In) · open".to_string(), "Synth OUT (Out) · open".to_string()];
    let report = report_text(&lines, (3, 1), &s.rows);
    assert!(report.starts_with("transit MIDI Monitor report\n\nPorts:\n  Keys (In) · open\n  Synth OUT (Out) · open\n"), "{report}");
    assert!(report.contains("\nDropped events: 3 in, 1 out\n"));
    assert!(report.contains("Last 200 lines:\n"));
    assert!(report.contains("Note On C3 (60) vel 100  [90 3C 64]"), "{report}");
    assert_eq!(report.lines().filter(|l| l.contains('[')).count(), 7);
    assert!(report_text(&[], (0, 0), &VecDeque::new()).contains("(none open)"));
    // Only the newest 200.
    let mut many = State::default();
    for n in 0..300u64 {
        let mut e = ev(Dir::In, 0x90, 60, 100);
        e.host_ns = n * 1_000_000;
        many.push(&e);
    }
    assert_eq!(report_text(&[], (0, 0), &many.rows).lines().filter(|l| l.contains('[')).count(), REPORT_LINES);
}

#[test]
fn the_buffer_keeps_the_newest_rows() {
    let mut s = State::default();
    for n in 0..(MAX_ROWS as u64 + 10) {
        let mut e = ev(Dir::In, 0x90, 60, 100);
        e.host_ns = n;
        s.push(&e);
    }
    assert_eq!(s.rows.len(), MAX_ROWS);
    assert_eq!(s.rows.front().unwrap().host_ns, 10, "the oldest were evicted");
    assert_eq!(s.rows.back().unwrap().host_ns, MAX_ROWS as u64 + 9);
}

#[test]
fn port_colors_are_stable_and_differ() {
    let s = state_with(&[]);
    assert_eq!(s.port_color("Keys"), s.port_color("Keys"));
    assert_ne!(s.port_color("Keys"), s.port_color("Synth OUT"));
    assert_ne!(s.port_color("Keys"), s.port_color("Pads"));
}

#[test]
fn led_lights_on_a_new_count_then_decays() {
    let mut led = Led::default();
    assert_eq!(led.level(5, 10.0), 0.0, "the first reading is only a baseline");
    assert_eq!(led.level(5, 10.1), 0.0);
    assert_eq!(led.level(6, 11.0), 1.0, "a message lights it fully");
    let half = led.level(6, 11.0 + LED_DECAY_SECS / 2.0);
    assert!((half - 0.5).abs() < 0.01, "{half}");
    assert!(led.level(6, 11.0 + LED_DECAY_SECS) < 0.001, "dark after about 120 ms");
    // A steady stream keeps it lit.
    let mut lit_frames = 0;
    for i in 0..60u64 {
        let t = 20.0 + i as f64 / 60.0;
        if led.level(100 + i, t) > 0.9 {
            lit_frames += 1;
        }
    }
    assert!(lit_frames >= 59, "{lit_frames}");
    assert!(led.level(159, 22.0) < 0.001, "counter unchanged: unlit");
}

#[test]
fn led_tooltip_names_the_last_port_and_message() {
    let none = Activity::default();
    assert!(led_tip(&none, "").starts_with("No MIDI input yet"));
    let a = Activity { count: 4, port: 1, bytes: [0xB0, 74, 12] };
    assert_eq!(led_tip(&a, "Keys"), "Keys: CC 74 Brightness 12. Click to open the MIDI Monitor.");
    let start = Activity { count: 1, port: 1, bytes: [0xFA, 0, 0] };
    assert_eq!(led_tip(&start, "Keys"), "Keys: Start. Click to open the MIDI Monitor.");
}

#[test]
fn prefs_are_saved_with_the_ui_state_and_old_files_still_load() {
    let ui = UiState {
        midi_monitor: Prefs {
            open: true,
            filters: Filters {
                dir: DirFilter::In,
                hidden_ports: vec!["Pads".into()],
                channels: 0b1010,
                notes: false,
                text: "cc 74".into(),
                ..Filters::default()
            },
            show_clock: true,
            autoscroll: false,
            size: [800.0, 300.0],
        },
        ..UiState::default()
    };
    let back: UiState = serde_json::from_value(serde_json::to_value(&ui).unwrap()).unwrap();
    assert_eq!(back.midi_monitor, ui.midi_monitor);
    let mut old = serde_json::to_value(UiState::default()).unwrap();
    old.as_object_mut().unwrap().remove("midi_monitor");
    let loaded: UiState = serde_json::from_value(old).unwrap();
    assert_eq!(loaded.midi_monitor, Prefs::default());
    assert!(!Prefs::default().open, "the window starts closed");
}

fn frame(ctx: &egui::Context, app: &mut StudioApp, t: f64) {
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
        time: Some(t),
        ..Default::default()
    };
    app.raw_input_hook(&mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
}

#[test]
fn the_window_menu_and_command_open_and_close_it_without_audio() {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    assert!(!app.ui.midi_monitor.open);
    assert_eq!(app.run("window.midi_monitor", json!({})).unwrap()["open"], true);
    let ctx = egui::Context::default();
    for i in 0..3 {
        frame(&ctx, &mut app, 1.0 + f64::from(i) / 60.0);
    }
    let inspect = app.inspect(&ctx);
    assert_eq!(inspect["midi_monitor"]["open"], true);
    assert!(inspect["midi_monitor"]["window"][2].as_f64().unwrap() > 100.0, "the window was drawn: {}", inspect["midi_monitor"]);
    assert!(app.arrange.bar_menu.iter().any(|m| m.label == "Window"), "the Window menu is in the bar");
    assert_eq!(app.run("window.midi_monitor", json!({"value": false})).unwrap()["open"], false);
    frame(&ctx, &mut app, 2.0);
    assert_eq!(app.inspect(&ctx)["midi_monitor"]["window"][2].as_f64().unwrap(), 0.0);
    assert_eq!(app.run("window.midi_monitor", json!({})).unwrap()["open"], true, "without value it flips");
}

#[test]
fn the_header_led_is_drawn_with_audio_and_a_click_opens_the_window() {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.player = Some(soundcraft_playback::Player::prepare_device_free(std::sync::Arc::new(app.engine.session().clone())).unwrap());
    let ctx = egui::Context::default();
    for i in 0..3 {
        frame(&ctx, &mut app, 1.0 + f64::from(i) / 60.0);
    }
    let led = app.inspect(&ctx)["midi_monitor"]["led"].clone();
    let n = |i: usize| led[i].as_f64().unwrap();
    assert!(n(2) - n(0) > 20.0 && n(3) - n(1) > 10.0, "the LED has a place in the transport bar: {led}");
    assert!(n(0) > 1000.0, "at the right of the bar: {led}");
    assert!(n(1) < f64::from(crate::theme::TRANSPORT_H) + 30.0, "in the top bar area: {led}");
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": (n(0) + n(2)) / 2.0, "y": (n(1) + n(3)) / 2.0}));
    let _ = crate::control::handle_for_test(&mut app, &egui::Context::default(), &req);
    for i in 0..30 {
        frame(&ctx, &mut app, 2.0 + f64::from(i) / 60.0);
    }
    assert!(app.ui.midi_monitor.open, "a click on the LED opens the MIDI Monitor");
}

#[test]
fn an_audio_reconnect_keeps_the_monitor_on_for_an_open_window() {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let new_player = |app: &StudioApp| soundcraft_playback::Player::prepare_device_free(std::sync::Arc::new(app.engine.session().clone())).unwrap();
    app.player = Some(new_player(&app));
    app.run("window.midi_monitor", json!({"value": true})).unwrap();
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, 1.0);
    assert!(app.player.as_ref().unwrap().midi_monitor_enabled(), "the window turns the taps on");
    // What a reconnect does: a new Player, whose monitor starts off.
    app.player = Some(new_player(&app));
    assert!(!app.player.as_ref().unwrap().midi_monitor_enabled());
    frame(&ctx, &mut app, 1.1);
    assert!(app.player.as_ref().unwrap().midi_monitor_enabled(), "the next frame turns them on again");
    app.run("window.midi_monitor", json!({"value": false})).unwrap();
    frame(&ctx, &mut app, 1.2);
    assert!(!app.player.as_ref().unwrap().midi_monitor_enabled());
}
