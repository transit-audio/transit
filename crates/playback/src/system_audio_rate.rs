//! The rate ratio of the System Audio endpoint's clock to the input device's clock (#268), the
//! estimate ADR 0017 prescribes for a second clock: frame counts against one shared time base.
//!
//! Each side keeps a line of (time, frame) points: the loopback stream's packets (the frame index
//! of a packet's first frame and the QPC time WASAPI gives for it) and the input stream's callbacks
//! (the frames read so far and the callback's frame time). A least-squares fit over the last
//! [`WINDOW_NS`] of points gives each side's frames per second of the shared clock, and their
//! ratio over the nominal one is the drift. It depends only on the counts and the times, never on
//! how the packets and the callbacks line up, so a 480-frame callback beside 480-frame packets
//! reads the same as any other.
//!
//! Both sides are timed on one clock domain, QPC ([`Domain::Qpc`]). The loopback packets carry the
//! QPC time WASAPI gives for their first frame ([`Kind::Position`]). The input callbacks carry the
//! stamp's host callback time on every backend ([`Kind::Host`]): the QPC read when the device's
//! block arrived, never the backend's own frame time, which on native ASIO is the driver's sample
//! position at the nominal rate and so always runs exactly at nominal against the interface's real
//! clock. An invalid stamp falls back to the reader's own monotonic clock ([`Kind::Arrival`], QPC
//! on Windows). Each line carries its kind and domain; a change of either starts the line again
//! (which also restarts it at a discontinuity), and a pair whose domains differ holds the
//! estimate instead of computing a ratio between two clocks that share no base. The estimate is
//! held while a restarted line refills.
//!
//! A side that stalls (a loopback that pauses and resumes without a discontinuity flag) leaves a
//! step in time with no frames; a point whose time step exceeds its frame step by more than
//! [`STALL_SLACK_NS`] restarts the line, so the fit never spans the step.
//!
//! Audio-thread code: the points live in fixed arrays inside the struct (no allocation, no lock,
//! no wait); a fit is O([`POINTS`]) and runs at most once per callback.

/// Where a time comes from. One line never mixes kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A stream's frame time: the QPC of the packet's first frame (the loopback side).
    Position,
    /// The stamp's host callback time: the QPC read when the block arrived (the input side).
    Host,
    /// The reader's own monotonic clock when the block arrived (an invalid stamp).
    Arrival,
}

/// The clock a time is read on. Two lines are only compared when they share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Domain {
    /// QPC (the host monotonic clock), which every time this crate takes is on.
    Qpc,
    /// A backend's own frame clock, such as a driver's sample position: not comparable.
    #[allow(dead_code)]
    Driver,
}

/// One observation of a clock: `frames` frames starting at frame index `first_frame` (counted
/// from the stream's start, silence and gap fill included) at `time_ns` (nanoseconds of the
/// shared monotonic time base; any origin, only differences are used).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Obs {
    pub first_frame: u64,
    pub frames: u32,
    pub time_ns: i64,
    pub kind: Kind,
    pub domain: Domain,
}

/// The points one line holds: at [`SPACING_NS`] apart they cover a little more than the window.
pub(crate) const POINTS: usize = 1024;
/// The least time between two stored points (closer observations are skipped, their frames still
/// count through the next point's index).
pub(crate) const SPACING_NS: i64 = 10_000_000;
/// The span of the fit: the points of the last 10 s. With position stamps (microsecond jitter)
/// the slope is good to a fraction of a ppm; with arrival times jittered by a millisecond it is
/// good to about 10 ppm.
pub(crate) const WINDOW_NS: i64 = 10_000_000_000;
/// A point whose time step exceeds the time its frames take at the nominal rate by more than this
/// restarts the line: a side that stopped delivering and resumed. 50 ms is far above the jitter of
/// a stamp (a few ms) and the drift over a step (1000 ppm of 10 s is 10 ms), and far below the
/// shortest pause (hundreds of ms) that would bias the fit.
pub(crate) const STALL_SLACK_NS: i64 = 50_000_000;
/// A line gives a slope once its points span this long.
pub(crate) const MIN_SPAN_NS: i64 = 2_000_000_000;
/// ... and hold at least this many points.
const MIN_POINTS: usize = 16;

/// One clock's (time, frame) points in a fixed ring.
pub(crate) struct Line {
    time: [i64; POINTS],
    frame: [u64; POINTS],
    /// The next slot to write.
    head: usize,
    len: usize,
    kind: Option<(Kind, Domain)>,
    /// The side's nominal frames per second, for the stall rule.
    hz: f64,
}

impl Line {
    fn new(hz: f64) -> Self {
        Self { time: [0; POINTS], frame: [0; POINTS], head: 0, len: 0, kind: None, hz }
    }

    fn reset(&mut self) {
        (self.head, self.len, self.kind) = (0, 0, None);
    }

    fn newest(&self) -> Option<(i64, u64)> {
        (self.len > 0).then(|| {
            let i = (self.head + POINTS - 1) % POINTS;
            (self.time[i], self.frame[i])
        })
    }

    /// Store the point (`frame`, `time_ns`) unless it is closer than [`SPACING_NS`] to the last
    /// one. A change of kind or domain, a time going backward, a frame index going backward or a
    /// stall (the time step exceeds the frames' time by more than [`STALL_SLACK_NS`]) starts the
    /// line again. Returns whether the point was stored.
    fn add(&mut self, frame: u64, time_ns: i64, kind: Kind, domain: Domain) -> bool {
        if self.kind != Some((kind, domain)) {
            self.reset();
            self.kind = Some((kind, domain));
        }
        if let Some((t, f)) = self.newest() {
            let frames_ns = frame.saturating_sub(f) as f64 / self.hz * 1e9;
            if time_ns < t || frame < f || (time_ns - t) as f64 - frames_ns > STALL_SLACK_NS as f64 {
                self.reset();
                self.kind = Some((kind, domain));
            } else if time_ns - t < SPACING_NS {
                return false;
            }
        }
        self.time[self.head] = time_ns;
        self.frame[self.head] = frame;
        self.head = (self.head + 1) % POINTS;
        self.len = (self.len + 1).min(POINTS);
        true
    }

    fn domain(&self) -> Option<Domain> {
        self.kind.map(|(_, domain)| domain)
    }

    /// Frames per second of the time base: the least-squares slope over the points of the last
    /// [`WINDOW_NS`], once they span [`MIN_SPAN_NS`]. Centered on the newest point, in f64.
    fn slope(&self) -> Option<f64> {
        let (t0, f0) = self.newest()?;
        let (mut n, mut sx, mut sy, mut sxx, mut sxy, mut span) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0i64);
        let mut count = 0usize;
        for k in 0..self.len {
            let i = (self.head + POINTS - 1 - k) % POINTS;
            let age = t0 - self.time[i];
            if age > WINDOW_NS {
                break;
            }
            let x = -(age as f64) * 1e-9;
            let y = -((f0 - self.frame[i]) as f64);
            (n, sx, sy, sxx, sxy) = (n + 1.0, sx + x, sy + y, sxx + x * x, sxy + x * y);
            (span, count) = (age, count + 1);
        }
        if count < MIN_POINTS || span < MIN_SPAN_NS {
            return None;
        }
        let d = n * sxx - sx * sx;
        (d > 0.0).then(|| (n * sxy - sx * sy) / d)
    }
}

/// The drift of the endpoint's clock against the device's: `(endpoint rate / device rate) /
/// nominal - 1`, relative (positive when the endpoint runs fast).
pub(crate) struct RateEstimate {
    endpoint: Line,
    device: Line,
    /// Endpoint frames per device frame at the nominal rates.
    nominal: f64,
    /// A point was stored since the last fit.
    dirty: bool,
    /// The last estimate; held while a restarted line refills.
    drift: Option<f64>,
}

impl RateEstimate {
    /// For an endpoint at `endpoint_hz` and a device at `device_hz` frames per second.
    pub(crate) fn new(endpoint_hz: f64, device_hz: f64) -> Self {
        Self { endpoint: Line::new(endpoint_hz), device: Line::new(device_hz), nominal: endpoint_hz / device_hz, dirty: false, drift: None }
    }

    /// Start again for another stream pair, in place (no large temporary on the audio thread).
    pub(crate) fn restart(&mut self, endpoint_hz: f64, device_hz: f64) {
        self.endpoint.reset();
        self.device.reset();
        (self.endpoint.hz, self.device.hz) = (endpoint_hz, device_hz);
        (self.nominal, self.dirty, self.drift) = (endpoint_hz / device_hz, false, None);
    }

    /// A loopback packet.
    pub(crate) fn endpoint(&mut self, obs: Obs) {
        self.dirty |= self.endpoint.add(obs.first_frame, obs.time_ns, obs.kind, obs.domain);
    }

    /// An input callback starting at device frame `first_frame`.
    pub(crate) fn device(&mut self, first_frame: u64, time_ns: i64, kind: Kind, domain: Domain) {
        self.dirty |= self.device.add(first_frame, time_ns, kind, domain);
    }

    /// The drift estimate, refitted when a point was stored; `None` until both lines span
    /// [`MIN_SPAN_NS`].
    pub(crate) fn drift(&mut self) -> Option<f64> {
        if std::mem::take(&mut self.dirty)
            && let (Some(e), Some(d)) = (self.endpoint.slope(), self.device.slope())
            && d > 0.0
            && e > 0.0
            && self.endpoint.domain() == self.device.domain()
        {
            self.drift = Some(e / d / self.nominal - 1.0);
        }
        self.drift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `seconds` of a 48 kHz device in `cb`-frame callbacks and an endpoint `ppm` fast in
    /// 480-frame packets, both on exact times, and return the estimate in ppm.
    fn run(ppm: f64, cb: u64, seconds: f64, arrival_jitter_ns: i64) -> Option<f64> {
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        let (mut device, mut packet, mut seed) = (0u64, 0u64, 7u64);
        let ep_rate = 48_000.0 * (1.0 + ppm * 1e-6);
        while (device as f64) < seconds * 48_000.0 {
            let t = device as f64 / 48_000.0;
            while (packet * 480) as f64 / ep_rate <= t {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                let jitter = if arrival_jitter_ns > 0 { (seed >> 33) as i64 % arrival_jitter_ns } else { 0 };
                let time_ns = ((packet * 480) as f64 / ep_rate * 1e9) as i64 + jitter;
                est.endpoint(Obs { first_frame: packet * 480, frames: 480, time_ns, kind: Kind::Position, domain: Domain::Qpc });
                packet += 1;
            }
            est.device(device, (t * 1e9) as i64, Kind::Host, Domain::Qpc);
            device += cb;
        }
        est.drift().map(|d| d * 1e6)
    }

    #[test]
    fn the_estimate_reads_the_drift_whatever_the_callback() {
        for cb in [128, 480, 960, 2048] {
            for ppm in [0.0, 50.0, -50.0, 1_000.0, -1_000.0] {
                let got = run(ppm, cb, 30.0, 0).expect("an estimate after 30 s");
                assert!((got - ppm).abs() < 1.0, "cb {cb}, {ppm} ppm read as {got}");
            }
        }
    }

    #[test]
    fn arrival_times_jittered_by_three_milliseconds_still_read_within_twenty_ppm() {
        for ppm in [0.0, 200.0, -500.0] {
            let got = run(ppm, 480, 60.0, 3_000_000).expect("an estimate");
            assert!((got - ppm).abs() < 20.0, "{ppm} ppm read as {got}");
        }
    }

    #[test]
    fn no_estimate_before_two_seconds_and_a_change_of_kind_restarts_the_line_but_holds_the_estimate() {
        assert_eq!(run(100.0, 480, 1.5, 0), None);
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        for k in 0..400u64 {
            let t = (k * 10_000_000) as i64;
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
            est.device(k * 480, t, Kind::Host, Domain::Qpc);
        }
        let held = est.drift().expect("4 s of points");
        assert!(held.abs() < 1e-9);
        est.device(400 * 480, 4_000_000_000, Kind::Arrival, Domain::Qpc);
        assert_eq!(est.drift(), Some(held), "the restarted device line is short: the estimate is held");
        assert_eq!(est.device.len, 1);
    }

    #[test]
    fn a_pair_on_different_clock_domains_holds_the_estimate_instead_of_computing_one() {
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        for k in 0..600u64 {
            let t = (k * 10_000_000) as i64;
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
            est.device(k * 480, t, Kind::Host, Domain::Driver);
        }
        assert_eq!(est.drift(), None, "no estimate from a pair that shares no clock");
        for k in 600..1200u64 {
            let t = (k * 10_000_000) as i64;
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
            est.device(k * 480, t, Kind::Host, Domain::Qpc);
        }
        let held = est.drift().expect("the device line is on QPC again and has refilled");
        assert!(held.abs() < 1e-9);
        for k in 1200..1800u64 {
            let t = (k * 10_000_000) as i64;
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
            est.device(k * 480, t, Kind::Host, Domain::Driver);
        }
        assert_eq!(est.drift(), Some(held), "mixed again: the last estimate is held");
    }

    #[test]
    fn a_step_in_time_with_no_frames_restarts_the_line_and_the_reading_holds() {
        for stall_s in [0.5, 2.0, 8.0] {
            let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
            let step = (stall_s * 1e9) as i64;
            for k in 0..2000u64 {
                // 20 s of both sides on exact time; at 10 s the endpoint's time steps and its frames do not.
                let t = (k * 10_000_000) as i64;
                let endpoint_t = if k < 1000 { t } else { t + step };
                est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: endpoint_t, kind: Kind::Position, domain: Domain::Qpc });
                est.device(k * 480, t, Kind::Host, Domain::Qpc);
                if let Some(d) = est.drift() {
                    assert!(d.abs() < 1e-6, "{stall_s} s stall: the reading left its bound ({d}) at point {k}");
                }
                if k == 1000 {
                    assert_eq!(est.endpoint.len, 1, "{stall_s} s stall: the endpoint line restarted at the step");
                }
            }
        }
    }
}
