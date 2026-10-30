//! What the Audio Devices window says about the buffer it cannot simply honor (#230): a saved size
//! the device no longer offers, a saved system that did not open (the lists then describe the
//! device that did), and, for a stream PipeWire serves through ALSA, the ALSA buffer, the period
//! transit's callbacks see and the server's quantum.

use crate::StudioApp;
use transit_playback::devices::{AudioSystem, DeviceInfo};
use transit_playback::{AudioConfig, pipewire};

/// The offered size nearest to `want` (by ratio, the larger on a tie), `None` for an empty list.
pub(crate) fn nearest(sizes: &[u32], want: u32) -> Option<u32> {
    let distance = |s: u32| (f64::from(s).ln() - f64::from(want.max(1)).ln()).abs();
    sizes.iter().copied().min_by(|a, b| distance(*a).total_cmp(&distance(*b)).then(b.cmp(a)))
}

/// The line under the buffer list when the saved size is not offered and the window uses the
/// nearest one that is.
pub(crate) fn snapped_note(saved: u32, used: u32, sizes: &[u32]) -> String {
    let (lo, hi) = (sizes.iter().min().copied().unwrap_or(used), sizes.iter().max().copied().unwrap_or(used));
    format!("Saved buffer {saved} is outside this device's range ({lo}-{hi}); using {used}. Apply saves it.")
}

/// The line under the system list when the saved system did not open and another runs.
pub(crate) fn fallback_note(saved: &str, running_system: &str, device: &str) -> String {
    format!(
        "Saved system “{saved}” did not open; {running_system} “{device}” is running, and the lists below describe it. Choose {running_system} and Apply to keep it, or Reconnect once {saved} is available."
    )
}

/// The saved system that did not open, with the system and device that did: the window's choices
/// are the applied ones, a Player runs, and its system is another (a native JACK open that fell to
/// ALSA, an ASIO driver that fell to WASAPI).
pub(crate) fn fell_back(app: &StudioApp, draft: &AudioConfig) -> Option<(String, String, String)> {
    let p = app.player.as_ref().filter(|p| !p.silent && app.draft_is_open(draft))?;
    let saved = draft.system.as_deref()?;
    (!saved.eq_ignore_ascii_case(&p.device_system)).then(|| (saved.to_owned(), p.device_system.clone(), p.device_name.clone()))
}

/// The running device in `systems` (by system and device name), for the lists when the saved
/// system did not open.
pub(crate) fn running_device<'a>(systems: &'a [AudioSystem], system: &str, device: &str) -> Option<&'a DeviceInfo> {
    let s = systems.iter().find(|s| s.name.eq_ignore_ascii_case(system))?;
    s.outputs.iter().find(|d| d.name.eq_ignore_ascii_case(device))
}

/// The buffer list's label of `size` on a device PipeWire serves through ALSA ("512 samples ·
/// 128-frame period": cpal splits the buffer into periods), else the plain size.
pub(crate) fn size_label(size: u32, system: Option<&str>, device: Option<&str>) -> String {
    let alsa = system.is_some_and(|s| s.eq_ignore_ascii_case("ALSA")) || (system.is_none() && cfg!(target_os = "linux"));
    if alsa && device.is_some_and(pipewire::routed) && pipewire::last_clock().is_some() {
        format!("{size} samples \u{b7} {}-frame period", size / pipewire::ALSA_PERIODS)
    } else {
        format!("{size} samples")
    }
}

/// The open line about the server's period for a cpal stream PipeWire serves through ALSA
/// ([`pipewire::period_note`]), from the latest callback's frames.
pub(crate) fn stream_line(app: &StudioApp) -> Option<String> {
    let p = app.player.as_ref().filter(|p| !p.silent && !p.native_backend)?;
    if !p.device_system.eq_ignore_ascii_case("ALSA") || !pipewire::routed(&p.device_name) {
        return None;
    }
    let t = p.callback_telemetry();
    let period = (t.callbacks > 0).then_some(t.callback_frames);
    pipewire::period_note(app.ui.audio.buffer_size, p.device_buffer, period, pipewire::last_clock().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_size_not_offered_snaps_to_the_nearest_offered_one() {
        let server = [256, 512, 1024, 2048, 4096, 8192];
        assert_eq!(nearest(&server, 128), Some(256));
        assert_eq!(nearest(&server, 16_384), Some(8192));
        assert_eq!(nearest(&[64, 128, 256, 512, 1024, 2048], 4096), Some(2048), "the tester's 4096 on a 64-2048 server");
        assert_eq!(nearest(&[64, 256], 128), Some(256), "a tie by ratio takes the larger");
        assert_eq!(nearest(&[], 128), None);
        assert_eq!(
            snapped_note(4096, 2048, &[64, 128, 256, 512, 1024, 2048]),
            "Saved buffer 4096 is outside this device's range (64-2048); using 2048. Apply saves it."
        );
        assert_eq!(
            fallback_note("JACK", "ALSA", "default"),
            "Saved system “JACK” did not open; ALSA “default” is running, and the lists below describe it. Choose ALSA and Apply to keep it, or Reconnect once JACK is available."
        );
    }
}
