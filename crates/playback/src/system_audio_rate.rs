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
//! Times come from the backends' frame stamps where they are valid (QPC positions on WASAPI and
//! ASIO) and from the reader's own monotonic clock on arrival otherwise ([`Kind`]); a side that
//! changes kind starts its line again, which also restarts it at a discontinuity (an invalid
//! stamp). The estimate is held while a restarted line refills.
//!
//! Audio-thread code: the points live in fixed arrays inside the struct (no allocation, no lock,
//! no wait); a fit is O([`POINTS`]) and runs at most once per callback.

/// Where a time comes from: the backend's frame stamp (a device position in QPC time) or the
/// reader's own clock when the block arrived. One line never mixes the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Position,
    Arrival,
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
    kind: Option<Kind>,
}

impl Line {
    fn new() -> Self {
        Self { time: [0; POINTS], frame: [0; POINTS], head: 0, len: 0, kind: None }
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
    /// one. A change of kind, a time going backward or a frame index going backward starts the
    /// line again. Returns whether the point was stored.
    fn add(&mut self, frame: u64, time_ns: i64, kind: Kind) -> bool {
        if self.kind != Some(kind) {
            self.reset();
            self.kind = Some(kind);
        }
        if let Some((t, f)) = self.newest() {
            if time_ns < t || frame < f {
                self.reset();
                self.kind = Some(kind);
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
    pub(crate) fn new(nominal: f64) -> Self {
        Self { endpoint: Line::new(), device: Line::new(), nominal, dirty: false, drift: None }
    }

    /// Start again for another stream pair, in place (no large temporary on the audio thread).
    pub(crate) fn restart(&mut self, nominal: f64) {
        self.endpoint.reset();
        self.device.reset();
        (self.nominal, self.dirty, self.drift) = (nominal, false, None);
    }

    /// A loopback packet.
    pub(crate) fn endpoint(&mut self, obs: Obs) {
        self.dirty |= self.endpoint.add(obs.first_frame, obs.time_ns, obs.kind);
    }

    /// An input callback starting at device frame `first_frame`.
    pub(crate) fn device(&mut self, first_frame: u64, time_ns: i64, kind: Kind) {
        self.dirty |= self.device.add(first_frame, time_ns, kind);
    }

    /// The drift estimate, refitted when a point was stored; `None` until both lines span
    /// [`MIN_SPAN_NS`].
    pub(crate) fn drift(&mut self) -> Option<f64> {
        if std::mem::take(&mut self.dirty)
            && let (Some(e), Some(d)) = (self.endpoint.slope(), self.device.slope())
            && d > 0.0
            && e > 0.0
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
        let mut est = Box::new(RateEstimate::new(1.0));
        let (mut device, mut packet, mut seed) = (0u64, 0u64, 7u64);
        let ep_rate = 48_000.0 * (1.0 + ppm * 1e-6);
        while (device as f64) < seconds * 48_000.0 {
            let t = device as f64 / 48_000.0;
            while (packet * 480) as f64 / ep_rate <= t {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                let jitter = if arrival_jitter_ns > 0 { (seed >> 33) as i64 % arrival_jitter_ns } else { 0 };
                let time_ns = ((packet * 480) as f64 / ep_rate * 1e9) as i64 + jitter;
                est.endpoint(Obs { first_frame: packet * 480, frames: 480, time_ns, kind: Kind::Position });
                packet += 1;
            }
            est.device(device, (t * 1e9) as i64, Kind::Position);
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
        let mut est = Box::new(RateEstimate::new(1.0));
        for k in 0..400u64 {
            let t = (k * 10_000_000) as i64;
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position });
            est.device(k * 480, t, Kind::Position);
        }
        let held = est.drift().expect("4 s of points");
        assert!(held.abs() < 1e-9);
        est.device(400 * 480, 4_000_000_000, Kind::Arrival);
        assert_eq!(est.drift(), Some(held), "the restarted device line is short: the estimate is held");
        assert_eq!(est.device.len, 1);
    }
}
