//! ADR 0021, amendment for #760, rule 1, through the app's automatic reopen: a native JACK open
//! that latched a rebuild (the graph runs another rate than the Player was built for) is reopened
//! without user action. Counted as native client opens through the Player: a stable mismatch costs
//! exactly one rebuild; a rate that keeps moving stops at the existing three-attempt limit with its
//! message, and nothing loops.

use super::*;
use transit_audio_backend as ab;

fn jack_client() -> ab::fake::FakeBackend {
    ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("JACK", "Headset").with_capture(2, true)
}

/// The app with a native JACK Player opened on `client`, every reopen opening `client` again.
fn rig(client: &ab::fake::FakeBackend) -> T {
    let cfg = AudioConfig { system: Some("JACK".into()), output: Some("Headset".into()), ..Default::default() };
    let mut t = T::new();
    t.app.player = Some(transit_playback::Player::open_fake_native_jack(t.app.engine.session_arc(), &cfg, client).unwrap());
    t.app.ui.audio = cfg;
    t.app.audio_devices.fake = None;
    let again = client.clone();
    t.app.reopen_with = Some(Box::new(move |session, cfg| transit_playback::Player::open_fake_native_jack(session, cfg, &again).unwrap()));
    t
}

/// Device opens so far (one render open per Player).
fn opens(client: &ab::fake::FakeBackend) -> usize {
    client.opens().iter().filter(|(direction, _)| *direction == ab::Direction::Output).count()
}

#[test]
fn a_stable_rate_mismatch_costs_exactly_one_rebuild() {
    let client = jack_client();
    client.latch_next_opens(1);
    let mut t = rig(&client);
    assert_eq!(t.app.player.as_ref().and_then(transit_playback::Player::reopen_cause), Some(transit_playback::reopen::ReopenCause::DriverReset));
    run(&mut t, &client, 120);
    assert_eq!(opens(&client), 2, "the provisional open and the rebuild");
    let p = t.app.player.as_ref().unwrap();
    assert!(!p.silent && p.reopen_cause().is_none(), "the rebuilt Player runs");
    run(&mut t, &client, 600);
    assert_eq!(opens(&client), 2, "and stays");
}

#[test]
fn a_rate_that_keeps_moving_stops_at_the_three_attempt_limit() {
    let client = jack_client();
    client.latch_next_opens(100);
    let mut t = rig(&client);
    run(&mut t, &client, 600);
    assert_eq!(opens(&client), 1 + transit_playback::reopen::REOPEN_ATTEMPTS as usize, "the open and three attempts");
    assert!(t.app.reopen.gave_up());
    assert!(
        t.app.status.contains("did not reopen after the driver reset (it did not open), after 3 attempts. Use Audio › Reconnect Device."),
        "{}",
        t.app.status
    );
    run(&mut t, &client, 600);
    assert_eq!(opens(&client), 4, "no loop");
}
