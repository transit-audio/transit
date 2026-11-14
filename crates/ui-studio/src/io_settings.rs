//! The machine's I/O paths (ADR 0015), kept with the preferences in `ui.json` (`UiState.io_maps`),
//! one table per audio system plus device name. A device that opens takes its saved table (the
//! opened channel counts are stored inside it, so one device in two USB modes is one entry); the
//! engine holds the live one (`setup.io_map`), and every change reaches the audio thread as a plan,
//! never a reopen.

use crate::StudioApp;
use transit_model::IoMap;

/// Tables kept in the preferences; the oldest go first.
const MAX_SAVED: usize = 64;

/// The table to use for a device that just opened: its saved one made to fit `opened` (the default
/// table for the channels the stream has), else `opened` itself. A device whose input count is
/// not known to the stream (a separate input device) keeps the saved input count.
pub fn adopt(saved: &[IoMap], opened: IoMap) -> IoMap {
    let Some(mut map) = saved.iter().find(|m| m.key() == opened.key()).cloned() else { return opened };
    let inputs = if opened.inputs > 0 { opened.inputs } else { map.inputs };
    map.reconcile(inputs, opened.outputs);
    // Where System Audio sits in the live input is the open input's, never the saved table's.
    map.system_audio = opened.system_audio;
    map
}

/// The table with the open input's own channel count (`input`, without the System Audio pair), or
/// `None` when there is no open input or the table already has that count and its input paths.
pub fn with_input_count(map: &IoMap, input: Option<usize>) -> Option<IoMap> {
    let inputs = u16::try_from(input?).unwrap_or(u16::MAX);
    let mut next = map.clone();
    next.adopt_inputs(inputs).then_some(next)
}

/// Keep `map` as its device's table (most recent last).
pub fn remember(saved: &mut Vec<IoMap>, map: &IoMap) {
    saved.retain(|m| m.key() != map.key());
    saved.push(map.clone());
    let excess = saved.len().saturating_sub(MAX_SAVED);
    saved.drain(..excess);
}

impl StudioApp {
    /// Once per opened device, give the engine this machine's table for it; afterwards pass every
    /// change of the engine's table to the audio thread and the preferences. Without a real
    /// device (`--no-audio`, the silent fallback) there is nothing to open or keep; the silent
    /// device of `--null-audio` gets its table, but it is not this machine's, so it is never saved.
    pub(crate) fn sync_io(&mut self) {
        // A Player waiting to be reopened after a driver reset gets the table with its successor.
        let Some(player) = self.player.as_ref().filter(|p| !p.silent && p.reopen_cause().is_none()) else { return };
        if self.io_adopt {
            let map = adopt(&self.ui.io_maps, (*player.io_map()).clone());
            self.engine.set_io_map(map);
            self.io_adopt = false;
        }
        // An input that opened as its own device (cpal: ALSA, CoreAudio) has a count the output's
        // open could not know (#429): the table takes it, so its input paths can be made.
        if let Some(map) = with_input_count(self.engine.io_map(), self.recorder.as_ref().map(|r| r.device_channels())) {
            self.engine.set_io_map(map);
        }
        if self.io_pushed == self.engine.io_revision() {
            return;
        }
        let map = self.engine.io_map().clone();
        if player.set_io(map.clone()).is_ok() {
            if !player.null_device() {
                remember(&mut self.ui.io_maps, &map);
            }
            self.io_pushed = self.engine.io_revision();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transit_model::{ChannelFormat, IoDirection, IoPath};

    /// `--null-audio`: the silent device's table reaches the engine and the player, and nothing is
    /// added to the saved tables in `ui.json`.
    #[test]
    fn a_null_audio_run_saves_no_table_for_the_silent_device() {
        use std::sync::Arc;
        use transit_playback::backend::null::{self, PaceClock, StepClock};
        let engine = transit_engine::Engine::default();
        let clock = StepClock::new();
        let player = transit_playback::Player::with_null_device_clock(engine.session_arc(), Arc::clone(&clock) as Arc<dyn PaceClock>);
        clock.settle();
        let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
        app.null_audio = true;
        assert!(app.player.as_ref().is_some_and(|p| p.null_device() && !p.silent));
        app.sync_io();
        assert_eq!(app.engine.io_map().key(), (null::SYSTEM, null::DEVICE), "the engine holds the silent device's table");
        assert_eq!(app.io_pushed, app.engine.io_revision(), "the table went to the player");
        assert!(app.ui.io_maps.is_empty(), "no table kept for the silent device: {:?}", app.ui.io_maps);
    }

    fn tascam() -> IoMap {
        IoMap::for_device("ASIO", "TASCAM Model 12", 12, 10)
    }

    #[test]
    fn an_unknown_device_gets_its_defaults_and_a_known_one_its_saved_table() {
        let mut saved = Vec::new();
        assert_eq!(adopt(&saved, tascam()), tascam());
        let mut mine = tascam();
        mine.add_path(IoPath {
            live: false,
            name: "Cue".into(),
            direction: IoDirection::Output,
            channels: vec![8, 9],
            format: ChannelFormat::Stereo,
        })
        .unwrap();
        remember(&mut saved, &mine);
        assert_eq!(adopt(&saved, tascam()), mine);
        assert_eq!(
            adopt(&saved, IoMap::for_device("ASIO", "Scarlett 2i2", 2, 2)).paths.len(),
            IoMap::for_device("ASIO", "Scarlett 2i2", 2, 2).paths.len()
        );
    }

    #[test]
    fn one_device_in_another_usb_mode_adapts_its_saved_table() {
        let mut mine = tascam();
        mine.add_path(IoPath {
            live: false,
            name: "Cue".into(),
            direction: IoDirection::Output,
            channels: vec![8, 9],
            format: ChannelFormat::Stereo,
        })
        .unwrap();
        mine.add_path(IoPath {
            live: false,
            name: "Pair".into(),
            direction: IoDirection::Output,
            channels: vec![2, 3],
            format: ChannelFormat::Stereo,
        })
        .unwrap();
        let mut saved = Vec::new();
        remember(&mut saved, &mine);
        let two = adopt(&saved, IoMap::for_device("ASIO", "TASCAM Model 12", 2, 2));
        assert_eq!((two.inputs, two.outputs), (2, 2), "the opened counts are stored inside the table");
        assert!(two.path("Cue").is_none() && two.path("Pair").is_none(), "paths beyond the opened channels are dropped");
        assert!(two.path("Out 1-2").is_some());
        // The stream knows no input count for a separate input device: the saved one stays.
        let separate = adopt(&saved, IoMap::for_device("ASIO", "TASCAM Model 12", 0, 10));
        assert_eq!((separate.inputs, separate.outputs), (12, 10));
    }

    /// #429: on ALSA the output opens the table with 0 inputs and the input opens as its own device;
    /// its count reaches the table, so the I/O page reads it and input paths can be made.
    #[test]
    fn the_open_inputs_count_reaches_a_table_opened_without_inputs() {
        let alsa = IoMap::for_device("ALSA", "pipewire", 0, 2);
        let map = with_input_count(&alsa, Some(2)).expect("the input's count is adopted");
        assert_eq!((map.inputs, map.outputs), (2, 2));
        assert!(map.path("In 1-2").is_some_and(|p| p.direction == IoDirection::Input));
        assert!(with_input_count(&map, Some(2)).is_none(), "nothing to change");
        assert!(with_input_count(&alsa, None).is_none(), "no open input: the table stays");
    }

    #[test]
    fn remembering_replaces_by_device_and_is_bounded() {
        let mut saved = Vec::new();
        remember(&mut saved, &tascam());
        remember(&mut saved, &tascam());
        assert_eq!(saved.len(), 1);
        for n in 0..(MAX_SAVED + 5) {
            remember(&mut saved, &IoMap::for_device("ASIO", &format!("device {n}"), 2, 2));
        }
        assert_eq!(saved.len(), MAX_SAVED);
        assert_eq!(saved.last().map(|m| m.device.as_str()), Some("device 68"));
    }
}
