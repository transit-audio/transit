use super::*;

#[test]
fn automation_first_pass_after_start_is_due_without_overflow() {
    // `last_write_at` starts (and resets) at `i64::MIN`: the previous
    // `(pos - last).abs()` panicked here in debug builds.
    assert!(automation_step_due(0, i64::MIN, 960));
    assert!(automation_step_due(48_000, i64::MIN, 960));
}

#[test]
fn automation_write_throttles_to_step() {
    assert!(!automation_step_due(1_000, 900, 960));
    assert!(automation_step_due(2_000, 900, 960));
}

#[test]
fn automation_write_fires_after_loop_wrap() {
    assert!(automation_step_due(0, 1_000_000, 960));
}

#[test]
fn a_write_pass_replaces_the_automation_it_plays_over() {
    let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    let fader = app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db;
    let q = i64::from(app.engine.session().sample_rate.hz()) / 4;
    // The old ride: a point every 10 ms for 3 s.
    for at in (0..12 * q).step_by(usize::try_from(q / 25).unwrap()) {
        app.run("automation.set_point", json!({"track": "Kick", "at": at, "value": -30.0})).unwrap();
    }
    app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
    app.run("transport.locate", json!({"at": 0})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    // Passes a quarter second apart, sparse like a covered window's.
    let ctx = egui::Context::default();
    let pass = |app: &mut SoundApp| {
        app.handle_transport(0.25);
        app.write_automation(&ctx);
    };
    pass(&mut app);
    pass(&mut app);
    // Two jumps: the stretches they skip keep the old ride.
    app.run("transport.locate", json!({"at": 4 * q})).unwrap();
    pass(&mut app);
    app.run("edit.select", json!({"tracks": ["Kick"], "start": 7 * q, "end": 11 * q})).unwrap();
    app.run("transport.play_selection", json!({})).unwrap();
    pass(&mut app);
    app.handle_transport(0.25);
    app.run("transport.stop", json!({})).unwrap();
    app.handle_transport(0.0);
    app.write_automation(&ctx);
    let lane = app.engine.session().track_by_name("Kick").unwrap().lane(&transit_model::AutoParam::Volume).unwrap();
    let values = |from, to| lane.points.iter().filter(|p| (from..=to).contains(&p.at)).map(|p| p.value).collect::<Vec<_>>();
    assert_eq!(values(q, 2 * q), [fader; 2]);
    assert_eq!(lane.value_at(3 * q, 0.0), -30.0);
    assert_eq!(lane.value_at(6 * q, 0.0), -30.0);
    // Stop writes the last point, at 9q.
    assert_eq!(values(8 * q, 9 * q), [fader; 2]);
    assert_eq!(lane.value_at(10 * q, 0.0), -30.0);
}

#[test]
fn a_pass_and_the_fader_ride_driving_it_are_one_undo_step() {
    let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    let steps = app.engine.undo_history().len();
    let ctx = egui::Context::default();
    for frame in 0..60 {
        app.handle_transport(1.0 / 60.0);
        app.write_automation(&ctx);
        // What the Mix window's fader sends while it is dragged.
        let db = -10.0 + frame as f32 / 10.0;
        app.engine.execute_merged("mix.volume", &json!({"track": kick.0, "db": db}), &format!("fader:{}", kick.0)).unwrap();
    }
    app.run("transport.stop", json!({})).unwrap();
    app.handle_transport(0.0);
    app.write_automation(&ctx);
    assert_eq!(app.engine.undo_history().len(), steps + 1);
}
