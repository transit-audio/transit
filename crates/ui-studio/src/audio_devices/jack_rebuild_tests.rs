//! ADR 0021, amendment for #760, rule 1, through the app's automatic reopen: a native JACK open
//! that latched a rebuild (the graph runs another rate than the Player was built for) is reopened
//! without user action. Counted as native client opens through the Player: a stable mismatch costs
//! exactly one rebuild; a rate that keeps moving stops at the existing three-attempt limit with its
//! message, and nothing loops. And slice 2's display-only lines on the page.

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

/// #760 slice 2: native JACK's display-only lines (the graph, the latency reported at startup,
/// PipeWire's sentence, a link not observed) show under the open lines, each with its help, and
/// Path latency is not among them (the readout replaces nothing).
#[test]
fn the_jack_device_lines_show_under_the_open_lines_with_their_help() {
    use crate::help::{Id, map::status_segments};
    let client = jack_client();
    let lines = [
        "Graph: 48000 Hz, buffer 1024 (reported by transit's active client)",
        "Latency reported at startup: output 256 frames (5.3 ms), input not reported; not measured, Measure measures the loop",
        "PipeWire may perform rate conversion and add buffering. JACK does not identify those stages separately or establish which delays are included in these reported ranges.",
        "Link from transit:out_1 to Headset:playback_1 was not observed at the startup check.",
    ];
    client.set_device_lines(&lines);
    let mut t = rig(&client);
    t.open_window();
    let opened = t.app.audio_devices.layout.opened.clone();
    let at = opened.iter().position(|l| l == lines[0]).unwrap_or_else(|| panic!("{opened:?}"));
    assert_eq!(&opened[at..at + 4], lines, "together, in order");
    assert!(opened[..at].iter().any(|l| l.starts_with("Output open:")), "under the output line");
    let ids: Vec<Option<Id>> = lines.iter().map(|l| status_segments(l)[0].id).collect();
    assert_eq!(
        ids,
        [Some(Id::AudioDevicesGraph), Some(Id::AudioDevicesLatencyReported), Some(Id::AudioDevicesGraph), Some(Id::AudioDevicesLinkCheck)]
    );
}
