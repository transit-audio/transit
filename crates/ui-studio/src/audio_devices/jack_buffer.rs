//! The notes under the Buffer size list on the JACK system, by server kind (ADR 0021, amendment for
//! #760, rule 2). Under JACK 2 a chosen size changes the server's buffer size for every client.
//! Under PipeWire it is a request on transit's own node that holds while transit's device is
//! active: a forced quantum or another program's request can override it, closing the device ends
//! it, and a request that was not fulfilled (the library refused it, or accepted it and the
//! graph did not keep it) is said as declined, apart from one whose result has not arrived (the
//! "Requested … · running at …" line compares with the settled figure).

use super::JACK_BUFFER_NOTE;
use crate::StudioApp;
use transit_playback::devices::{self, AudioSystem};

/// Said under the buffer list on the JACK system under PipeWire.
pub const JACK_PIPEWIRE_BUFFER_NOTE: &str = "Requests this buffer size from PipeWire while transit's device is active; a forced quantum or another program's request can override it, and closing the device ends the request.";
/// Added when PipeWire's library refused the open's buffer size request.
pub const JACK_BUFFER_DECLINED: &str = "PipeWire declined the request.";

/// The notes under the buffer list for a JACK system served by PipeWire's library or not, with
/// the open device's request fulfilled or not (said only under PipeWire).
pub fn jack_buffer_notes(pipewire: bool, declined: bool) -> Vec<&'static str> {
    match (pipewire, declined) {
        (false, _) => vec![JACK_BUFFER_NOTE],
        (true, false) => vec![JACK_PIPEWIRE_BUFFER_NOTE],
        (true, true) => vec![JACK_PIPEWIRE_BUFFER_NOTE, JACK_BUFFER_DECLINED],
    }
}

impl StudioApp {
    /// The notes under the buffer list for the JACK system `system` as listed: by its server, and
    /// whether the open JACK device's request was declined.
    pub(crate) fn jack_buffer_lines(&self, system: Option<&AudioSystem>) -> Vec<&'static str> {
        let pipewire = system.is_some_and(transit_playback::backend::native::jack_runs_pipewire);
        let declined = self.player.as_ref().is_some_and(|p| !p.silent && p.device_buffer_declined && devices::is_jack_system(Some(&p.device_system)));
        jack_buffer_notes(pipewire, declined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_note_says_what_a_size_means_on_each_server() {
        assert_eq!(jack_buffer_notes(false, false), ["Changes the JACK server's buffer size for all clients."], "JACK 2");
        assert_eq!(jack_buffer_notes(false, true), [JACK_BUFFER_NOTE], "JACK 2's refusal shows in the requested-versus-running line");
        assert_eq!(
            jack_buffer_notes(true, false),
            [
                "Requests this buffer size from PipeWire while transit's device is active; a forced quantum or another program's request can override it, and closing the device ends the request."
            ]
        );
        assert_eq!(jack_buffer_notes(true, true), [JACK_PIPEWIRE_BUFFER_NOTE, "PipeWire declined the request."]);
    }
}
