//! The armed track's input meter (#730): a record-armed audio track's meter shows its input level
//! whenever the track is not playing its own material, whether or not the track is monitored.
//! Display only: what the strip renders and routes is unchanged; only the meter's reading is.

use super::*;
use crate::strip::{LiveInput, hears_input, input_channel};

/// Whether `t`'s meter reads its live input instead of its strip this block. The moments are the
/// ones at which the mix monitors an armed audio track while Monitor Armed Tracks is on: live input
/// present, the track record-armed, and the transport stopped or recording ([`hears_input`]). A
/// track that already hears its input (the option on, or Input Monitoring) keeps its usual reading,
/// so only an armed track left unmonitored by the option changes. While playing without recording
/// the track plays its own material and its meter shows that, as before.
pub(crate) fn meters_input(t: &Track, live: &LiveInput<'_>) -> bool {
    t.kind == TrackKind::Audio && t.mixer.record_arm && !live.input.is_empty() && (live.monitor_only || live.recording) && !hears_input(t, live)
}

/// Audio thread: measures `st`'s meter for the block. For an armed, unmonitored track
/// ([`meters_input`]) the reading is the live input as the track would receive it (the channels
/// its input route reads, before trim, inserts and fader: the level the take records); otherwise
/// the strip's own buffer, as always. Allocation-, lock- and wait-free: slices of buffers the
/// callback already holds.
pub(crate) fn measure_strip(t: &Track, st: &mut Strip, live: &LiveInput<'_>, frames: usize, host_frame: u64) {
    if meters_input(t, live) {
        let n_in = live.input.len().max(1);
        let route = st.input_route.as_deref();
        let chans = (0..st.buf.len()).map(|k| {
            let src = input_channel(t, route, k, n_in).and_then(|c| live.input.get(c)).map_or(&[][..], Vec::as_slice);
            let src = src.get(live.input_offset..).unwrap_or(&[]);
            src.get(..frames.min(src.len())).unwrap_or(&[])
        });
        st.meter.measure_slices(chans, st.gr, host_frame);
    } else {
        st.meter.measure(&st.buf, frames, st.gr, host_frame);
    }
}
