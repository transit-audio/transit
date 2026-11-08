use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// #363: a run without a player (`--no-audio`) lists none of the machine's audio devices, so it
/// creates no ASIO driver and no WASAPI client: opening Audio Devices on each section, Rescan and
/// Copy diagnostics never pass the listing gate (`may_list_machine`), and the scripted machine
/// lister standing in for the real one is never asked. Control: with a (device-free) player the
/// same window passes the gate and asks the lister.
#[test]
fn a_run_without_audio_lists_no_machine_device_and_creates_no_driver() {
    let mut t = T::new();
    t.control("audio.fake_devices", json!({"systems": null})).unwrap();
    assert!(t.app.player.is_none() && t.app.audio_devices.fake.is_none(), "no player, no stand-in devices: the --no-audio run");
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let wasapi = transit_playback::devices::AudioSystem {
        name: "WASAPI".into(),
        is_default: true,
        outputs: Vec::new(),
        inputs: Vec::new(),
        diagnostics: Vec::new(),
    };
    t.app.audio_devices.machine = Some((
        vec![wasapi],
        Arc::new(move |_: Option<String>| {
            counted.fetch_add(1, Ordering::Relaxed);
            None
        }),
    ));
    t.app.diag.machine = Some(crate::diagnostics::Machine::default());
    t.open_window();
    for key in ["section_io", "section_hardware", "section_system"] {
        t.section(key);
    }
    t.click(mid(t.control_rect("rescan")));
    t.click(mid(t.control_rect("copy_diagnostics")));
    t.frames(10);
    let gate = |t: &T| t.app.audio_devices.machine_listings.load(Ordering::Relaxed);
    assert_eq!(gate(&t), 0, "no listing passed the gate");
    assert_eq!(asked.load(Ordering::Relaxed), 0, "the ASIO lister was never asked");
    assert_eq!(t.layout()["systems"], json!([]), "nothing listed");
    assert!(t.app.diag.copied.contains(crate::diagnostics::NO_AUDIO_DEVICES), "the report says why: {}", t.app.diag.copied);
    // No System Audio output is listed. Where System Audio is offered (Windows) the block ran and
    // recorded an empty list; elsewhere there is no System Audio block, so nothing is ever listed.
    let outputs = t.app.audio_devices.system_outputs.as_deref();
    if transit_playback::system_audio::offered() {
        assert_eq!(outputs, Some(&[][..]), "the System Audio block listed no output");
    } else {
        assert_eq!(outputs, None, "no System Audio block off Windows: nothing listed");
    }

    // Control: a player lets the same window's listing through, to the scripted machine.
    t.app.player = Some(transit_playback::Player::prepare_device_free(t.app.engine.session_arc()).unwrap());
    t.click(mid(t.control_rect("rescan")));
    t.frames(10);
    assert!(gate(&t) > 0, "with a player the listing passes the gate");
    assert_eq!(t.layout()["systems"][0], json!("WASAPI"), "the scripted machine's systems");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // timing: stall-bound ten seconds for the listing worker thread
    while asked.load(Ordering::Relaxed) == 0 && std::time::Instant::now() < deadline {
        t.frames(1);
    }
    assert!(asked.load(Ordering::Relaxed) >= 1, "the ASIO lister was asked (the scripted one: no driver to describe)");
}
