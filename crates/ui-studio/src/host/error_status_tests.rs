//! The processing-error status line (#284, ADR 0059 Decision 2): an instrument gets its own
//! message, since it stays silent until Stop; an effect insert keeps the message whose output
//! returns when the error ends.

use super::plugin_error_status;
use transit_model::{ChannelFormat, Insert, Session, TrackKind};

#[test]
fn an_instruments_processing_error_says_it_stays_silent_until_stop() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let track = s.track_mut(t).unwrap();
    track.name = "Keys".into();
    track.instrument = Some(Insert::new("clap:test.synth"));
    track.mixer.inserts[0] = Some(Insert::new("clap:test.fx"));
    assert_eq!(
        plugin_error_status(&s, t, transit_mix::INSTRUMENT_SLOT).as_deref(),
        Some("clap:test.synth on Keys reported a processing error. It stays silent until you press Stop.")
    );
    assert_eq!(
        plugin_error_status(&s, t, 0).as_deref(),
        Some("clap:test.fx on Keys reported a processing error, so its output is silent while the error lasts.")
    );
}
