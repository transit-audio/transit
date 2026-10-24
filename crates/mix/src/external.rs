//! External Instruments in a plan (ADR 0020, slice 1).
//!
//! An instrument track's instrument slot can hold an External Instrument: the track's MIDI goes
//! out to a hardware synth (the playback layer sends it, #61) and the synth's audio comes back on
//! an input path. The plan reads that return into the strip, where a hosted instrument would
//! have rendered, gained by the device's `gain_db`, and the strip reports the device's latency C
//! at its head, as a latent hosted instrument's counts (decision 6). C is the number of plan
//! frames from the render frame of a note-on to the render frame at which its onset enters the
//! strip, so delay compensation lines the return up with every software path at Main exactly as
//! it does for a hardware insert's round trip (decision 2). MIDI is not sent early.
//!
//! In this slice C comes from the device's manual value only. With no live input (no duplex
//! device, an offline render), an unresolved or unchosen return, or no usable MIDI port, the strip
//! is silent and reports 0, so delay compensation does not move while the hardware is absent.
//! Nothing here allocates on the audio thread: the return is read from the block of live input the
//! engine holds.

use super::{MAX_LATENCY, Session, Track, TrackId, TrackKind};
use soundcraft_model::{IoDirection, IoMap};

/// What an External Instrument will do on this machine, as far as the plan knows (the MIDI port's
/// presence comes from the playback layer's output thread).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalState {
    /// The return resolves, live input is open and a latency is set: the strip plays the return.
    Ready,
    /// As `Ready`, but no latency is set: the synth sounds late by its round trip.
    Unmeasured,
    /// No return path chosen yet.
    NoReturn,
    /// The return path is not an input path of this device.
    MissingReturn,
    /// No capture stream is open (not a duplex device).
    NoInput,
    /// The track sends its MIDI nowhere usable (no port chosen, or the port is missing).
    NoPort,
}

/// One External Instrument of a session as this machine sees it (the UI's card).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReport {
    pub track: TrackId,
    pub ret: String,
    pub state: ExternalState,
    /// The latency C in plan frames the device reports when it plays (manual in this slice).
    pub latency: Option<u32>,
}

/// An External Instrument in a plan: the device input channel each strip channel reads, and C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtSpec {
    /// The device input channels of the return (empty: the strip is silent).
    pub(crate) src: Vec<u16>,
    /// Frames of latency, 0 when none is set.
    pub(crate) latency: u32,
}

/// C for `t` at `rate`, in plan frames: `manual_ms` converted at the rate (decision 4, case 1). The
/// measured lookup (cases 2 and 3) comes with Measure (slice 2).
pub(crate) fn device_latency(t: &Track, rate: u32) -> Option<u32> {
    t.external_instrument().and_then(|e| e.manual_frames(rate)).map(|f| f.min(MAX_LATENCY as u32))
}

/// The latency, in frames, `t`'s External Instrument would add: an upper bound for sizing delay
/// lines (a plan without live input adds none).
pub(crate) fn latency_of(t: &Track, rate: u32) -> usize {
    if t.inactive || t.freeze.is_some() { 0 } else { device_latency(t, rate).unwrap_or(0) as usize }
}

/// Whether `t` can send its MIDI: an external port is chosen and the playback layer has not
/// reported it missing (`no_port`).
fn has_port(t: &Track, no_port: &[TrackId]) -> bool {
    t.midi_output.is_external() && !no_port.contains(&t.id)
}

/// The device input channels of `t`'s return on an `in_channels`-wide input (0: not known).
fn return_channels(ret: &str, io: &IoMap, in_channels: u16) -> Option<Vec<u16>> {
    io.path(ret)
        .filter(|p| p.direction == IoDirection::Input && !p.channels.is_empty() && p.channels.iter().all(|&c| in_channels == 0 || c < in_channels))
        .map(|p| p.channels.clone())
}

/// The External Instrument of `t` for a plan with an `in_channels`-wide device input at `rate`;
/// `no_port` names the tracks whose MIDI port is missing.
pub(crate) fn spec_of(t: &Track, io: &IoMap, in_channels: u16, rate: u32, no_port: &[TrackId]) -> Option<ExtSpec> {
    let e = t.external_instrument()?;
    let src = if has_port(t, no_port) { return_channels(&e.ret, io, in_channels).unwrap_or_default() } else { Vec::new() };
    let latency = if src.is_empty() { 0 } else { device_latency(t, rate).unwrap_or(0) };
    Some(ExtSpec { src, latency })
}

/// Every External Instrument of `s` as the machine `io` sees it. `in_channels` is what the device
/// opened (0: no device), `duplex` whether a capture stream is open, `rate` the session's sample
/// rate, `no_port` the tracks whose MIDI port the output thread reports missing.
#[must_use]
pub fn external_report(s: &Session, io: &IoMap, in_channels: u16, duplex: bool, rate: u32, no_port: &[TrackId]) -> Vec<ExternalReport> {
    let mut out = Vec::new();
    for t in s.tracks.iter().filter(|t| t.kind == TrackKind::Instrument) {
        let Some(e) = t.external_instrument() else { continue };
        let latency = device_latency(t, rate);
        let state = if !has_port(t, no_port) {
            ExternalState::NoPort
        } else if e.ret.is_empty() {
            ExternalState::NoReturn
        } else if return_channels(&e.ret, io, in_channels).is_none() {
            ExternalState::MissingReturn
        } else if !duplex {
            ExternalState::NoInput
        } else if latency.is_none() {
            ExternalState::Unmeasured
        } else {
            ExternalState::Ready
        };
        out.push(ExternalReport { track: t.id, ret: e.ret.clone(), state, latency });
    }
    out
}

/// An External Instrument's state in a running strip.
pub(crate) struct ExtSlot {
    pub(crate) spec: ExtSpec,
}

impl ExtSlot {
    pub(crate) fn new(spec: ExtSpec) -> Self {
        ExtSlot { spec }
    }

    /// The frames this device adds when it plays (`duplex`: live input is open): C, else 0.
    pub(crate) fn latency(&self, duplex: bool) -> usize {
        if duplex && !self.spec.src.is_empty() { self.spec.latency as usize } else { 0 }
    }

    /// Fill the strip's `buf` with this block's return from the live `input` (planar, device
    /// channels), times `gain`. A mono return feeds every channel of the strip. False, with `buf`
    /// left as it is (silence), when there is no live input or the return is unresolved.
    pub(crate) fn run(&self, buf: &mut [Vec<f32>], input: &[Vec<f32>], frames: usize, gain: f32) -> bool {
        if input.is_empty() || self.spec.src.is_empty() || buf.is_empty() {
            return false;
        }
        let last = self.spec.src.len().saturating_sub(1);
        for (k, dst) in buf.iter_mut().enumerate() {
            let dev = usize::from(self.spec.src.get(k.min(last)).copied().unwrap_or(0));
            let dst = dst.get_mut(..frames).unwrap_or(&mut []);
            match input.get(dev) {
                Some(src) => {
                    for (d, x) in dst.iter_mut().zip(src.iter()) {
                        *d = *x * gain;
                    }
                    dst.iter_mut().skip(src.len()).for_each(|d| *d = 0.0);
                }
                None => dst.iter_mut().for_each(|d| *d = 0.0),
            }
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Insert, MidiOutChannel, MidiOutPort, MidiOutput};

    fn track(ret: &str, manual_ms: Option<f32>) -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let tr = s.track_mut(t).unwrap();
        let mut ins = Insert::external(ret);
        ins.external.as_mut().unwrap().manual_ms = manual_ms;
        tr.instrument = Some(ins);
        tr.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::AsRecorded, offset_ms: 0.0 };
        (s, t)
    }

    #[test]
    fn the_device_reports_its_manual_latency_in_plan_frames_only_when_it_plays() {
        let io = IoMap::for_device("test", "duplex", 2, 2);
        let (s, t) = track("In 1", Some(20.0));
        let tr = s.track(t).unwrap();
        assert_eq!(device_latency(tr, 48_000), Some(960));
        assert_eq!(device_latency(tr, 44_100), Some(882));
        let spec = spec_of(tr, &io, 2, 48_000, &[]).unwrap();
        assert_eq!(spec, ExtSpec { src: vec![0], latency: 960 });
        let slot = ExtSlot::new(spec);
        assert_eq!(slot.latency(true), 960);
        assert_eq!(slot.latency(false), 0, "no live input: silent, and delay compensation does not move");
        // A missing return or a missing port reports 0.
        let (s, t) = track("In 7", Some(20.0));
        assert_eq!(spec_of(s.track(t).unwrap(), &io, 2, 48_000, &[]).unwrap(), ExtSpec { src: Vec::new(), latency: 0 });
        let (s, t) = track("In 1", Some(20.0));
        assert_eq!(spec_of(s.track(t).unwrap(), &io, 2, 48_000, &[t]).unwrap().latency, 0);
        let (mut s, t) = track("In 1", Some(20.0));
        s.track_mut(t).unwrap().midi_output = MidiOutput::Internal;
        assert_eq!(spec_of(s.track(t).unwrap(), &io, 2, 48_000, &[]).unwrap().latency, 0);
    }

    #[test]
    fn the_report_names_each_missing_state() {
        let io = IoMap::for_device("test", "duplex", 2, 2);
        let state = |ret: &str, ms: Option<f32>, duplex: bool, no_port: bool| {
            let (s, t) = track(ret, ms);
            let missing = if no_port { vec![t] } else { Vec::new() };
            external_report(&s, &io, 2, duplex, 48_000, &missing)[0].state
        };
        assert_eq!(state("In 1", Some(20.0), true, false), ExternalState::Ready);
        assert_eq!(state("In 1", None, true, false), ExternalState::Unmeasured);
        assert_eq!(state("", Some(20.0), true, false), ExternalState::NoReturn);
        assert_eq!(state("In 9", Some(20.0), true, false), ExternalState::MissingReturn);
        assert_eq!(state("In 1", Some(20.0), false, false), ExternalState::NoInput);
        assert_eq!(state("In 1", Some(20.0), true, true), ExternalState::NoPort);
    }

    #[test]
    fn the_return_fills_the_strip_with_gain_and_a_mono_return_feeds_both_sides() {
        let slot = ExtSlot::new(ExtSpec { src: vec![1], latency: 0 });
        let input = vec![vec![0.0; 4], vec![0.5, -0.5, 0.25, 1.0]];
        let mut buf = vec![vec![9.0; 4]; 2];
        assert!(slot.run(&mut buf, &input, 4, 0.5));
        assert_eq!(buf, vec![vec![0.25, -0.25, 0.125, 0.5]; 2]);
        let mut buf = vec![vec![0.0; 4]; 2];
        assert!(!slot.run(&mut buf, &[], 4, 1.0), "no live input");
        assert!(!ExtSlot::new(ExtSpec { src: Vec::new(), latency: 9 }).run(&mut buf, &input, 4, 1.0));
        assert_eq!(buf, vec![vec![0.0; 4]; 2]);
    }
}
