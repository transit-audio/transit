//! The rate ratio of the System Audio endpoint's clock to the input device's clock (#268), the
//! estimate ADR 0017 prescribes for a second clock: frame counts against one shared time base.
//!
//! Each side keeps a line of (time, frame) points: the loopback stream's packets (the frame index
//! of a packet's first frame and the QPC time WASAPI gives for it) and the input stream's callbacks
//! (the frames read so far and the callback's arrival time). A least-squares fit over each line's
//! window gives each side's frames per second of the shared clock, and their ratio over the
//! nominal one is the drift. It depends only on the counts and the times, never on
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
//! **The two kinds of time behave differently, and each line follows its kind.** A position time
//! is exact: when a point's time step exceeds the time its frames take by more than
//! [`STALL_SLACK_NS`] (a loopback that paused and resumed without a discontinuity flag), the
//! offset is permanent, so the line restarts on that point at once and the fit never spans the
//! step. An arrival time (a host callback time or the reader's own clock) is the true time plus a
//! delay that is never negative: a few milliseconds of jitter, and now and then one block that is
//! tens of milliseconds late before the next ones catch up, which leaves no lasting offset.
//! Anchoring a restarted line on that one late point would carry it at full weight for seconds
//! (the fourth review of #268: the trim at its clamp, the reading 1400 ppm off). So an
//! arrival-timed line never restarts on a single point: a point more than [`OUTLIER_NS`] later
//! than the newest stored point predicts is an outlier and is not stored; only when outliers
//! follow one another without a point back on the line for [`CONFIRM_NS`] of arrival time (and at
//! least [`CONFIRM_POINTS`] of them) is the offset real (a device that lost frames), and the line
//! restarts on the point that confirms it. The blocks that catch up behind a late one arrive with
//! it and carry no new time, so they are skipped, and the run spans no time and never confirms.
//! A spell of lateness that lasts that long confirms on a late point; when it ends, the first
//! point back on time is more than [`OUTLIER_NS`] earlier than the restarted line predicts, which
//! no delay can make it, so the line restarts again on that point (the fifth review of #268: the
//! trim at its clamp for 20 to 60 s), and a restarted line's anchor gives way to a block with less
//! delay that arrives with it or right after it.
//! Arrival jitter is the other noise of an arrival-timed line, and its fit uses a longer window
//! to average it: every point of its [`ARRIVAL_POINTS`] ring, up to [`ARRIVAL_WINDOW_NS`] (20 s at
//! 10 ms callbacks, 60 s at 8192-frame ones), against [`WINDOW_NS`] for a position-timed line.
//!
//! Audio-thread code: the points live in fixed arrays inside the struct (no allocation, no lock,
//! no wait); a fit is one pass over a line's ring and runs at most once per callback.

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

/// The points the endpoint's line holds: at [`SPACING_NS`] apart they cover a little more than
/// [`WINDOW_NS`].
pub(crate) const POINTS: usize = 1024;
/// The points the device's line holds (it is arrival-timed): 20 s at callbacks of 10 ms or less
/// (one point per [`SPACING_NS`]), more at larger ones (41 s at 960 frames), up to
/// [`ARRIVAL_WINDOW_NS`] (60 s, 350 points at 8192 frames).
pub(crate) const ARRIVAL_POINTS: usize = 2048;
/// The least time between two stored points (closer observations are skipped, their frames still
/// count through the next point's index).
pub(crate) const SPACING_NS: i64 = 10_000_000;
/// The span of a position-timed line's fit: the points of the last 10 s. With position stamps
/// (microsecond jitter) the slope is good to a fraction of a ppm.
pub(crate) const WINDOW_NS: i64 = 10_000_000_000;
/// The longest span of an arrival-timed line's fit; within it the fit takes every point the ring
/// holds. The slope's noise goes as the jitter over (the square root of the points x the span), so
/// a full ring of 10 ms callbacks (20 s) and 60 s of 8192-frame callbacks (350 points) average it
/// about equally well.
pub(crate) const ARRIVAL_WINDOW_NS: i64 = 60_000_000_000;
/// A point of a position-timed line whose time step exceeds the time its frames take at the
/// nominal rate by more than this restarts the line: a side that stopped delivering and resumed.
/// 50 ms is far above the jitter of a stamp (microseconds) and the drift over a step (1000 ppm of
/// 10 s is 10 ms), and far below the shortest pause (hundreds of ms) that would bias the fit.
pub(crate) const STALL_SLACK_NS: i64 = 50_000_000;
/// A point of an arrival-timed line more than this later than the newest stored point predicts
/// (its time step less the time its frames take) is an outlier and is not stored. 20 ms is several
/// times the arrival jitter of a callback (1 to 3 ms in the tests; a timer-driven shared-mode
/// period of 10 ms at worst), and a delay just under it costs the 20 s fit a few ppm.
pub(crate) const OUTLIER_NS: i64 = 20_000_000;
/// Outliers that follow one another for this long of arrival time, with no point back on the line
/// between them, are a lasting offset: the line restarts on the point that confirms it.
pub(crate) const CONFIRM_NS: i64 = 300_000_000;
/// ... and there are at least this many of them (a run of 8192-frame callbacks is three in 340 ms).
pub(crate) const CONFIRM_POINTS: u32 = 3;
/// A line gives a slope once its points span this long.
pub(crate) const MIN_SPAN_NS: i64 = 2_000_000_000;
/// ... and hold at least this many points.
const MIN_POINTS: usize = 16;

/// One clock's (time, frame) points in a fixed ring of `N`.
pub(crate) struct Line<const N: usize> {
    time: [i64; N],
    frame: [u64; N],
    /// The next slot to write.
    head: usize,
    len: usize,
    kind: Option<(Kind, Domain)>,
    /// The side's nominal frames per second, for the stall and outlier rules.
    hz: f64,
    /// Outliers in a row (an arrival-timed line), and the arrival times of the first and the last.
    outliers: u32,
    outlier_since: i64,
    outlier_last: i64,
    /// The line holds one point, its anchor, which a point with less delay may still replace
    /// (an arrival-timed line): the points within [`SPACING_NS`] of the first anchor's arrival,
    /// then one more, so a device a little fast against the time base still grows its line.
    provisional: bool,
    anchor_at: i64,
}

impl<const N: usize> Line<N> {
    fn new(hz: f64) -> Self {
        Self {
            time: [0; N],
            frame: [0; N],
            head: 0,
            len: 0,
            kind: None,
            hz,
            outliers: 0,
            outlier_since: 0,
            outlier_last: 0,
            provisional: false,
            anchor_at: 0,
        }
    }

    fn reset(&mut self) {
        (self.head, self.len, self.kind, self.outliers) = (0, 0, None, 0);
    }

    /// Start again on a line of `kind` and `domain` (the next stored point is its first).
    fn restart(&mut self, kind: Kind, domain: Domain) {
        self.reset();
        (self.kind, self.provisional) = (Some((kind, domain)), true);
    }

    fn newest(&self) -> Option<(i64, u64)> {
        (self.len > 0).then(|| {
            let i = (self.head + N - 1) % N;
            (self.time[i], self.frame[i])
        })
    }

    /// Store the point (`frame`, `time_ns`) unless it is closer than [`SPACING_NS`] to the last
    /// one. A change of kind or domain, a time going backward or a frame index going backward
    /// starts the line again. `late` is the point's time step less its frames' time at the
    /// nominal rate, against the newest stored point.
    ///
    /// On a position-timed line, `late` over [`STALL_SLACK_NS`] restarts the line on the point.
    ///
    /// On an arrival-timed line (arrival = true time + a delay that is never negative):
    /// - `late` over [`OUTLIER_NS`]: an outlier, not stored. It restarts the line only when it
    ///   confirms a run of outliers, at least [`CONFIRM_POINTS`] of them over [`CONFIRM_NS`] with
    ///   no point back on the line between them (a lasting offset). A point within [`SPACING_NS`]
    ///   of the last outlier's arrival (a block that caught up with it) is skipped.
    /// - `late` under -[`OUTLIER_NS`]: the point arrived earlier than the line predicts, which a
    ///   delay can never make it, so the newest stored point was late. The line restarts on this
    ///   point. This ends a spell of lateness that confirmed a "lasting offset" on a late point
    ///   (the first on-time point after the spell), and it is also the right answer when a real
    ///   lasting offset ends (the device goes back to normal): the first point after it restarts
    ///   the line.
    /// - A restarted line's only point is provisional: a point within [`SPACING_NS`] of its
    ///   arrival, or the first point after that, which is earlier than it predicts carries less
    ///   delay and replaces it. A restart lands on a late point or on the first of a burst of
    ///   blocks that arrived together; the last of them, or the first block after them, is the
    ///   anchor with the least delay, and a refilled line anchored on a delayed point would carry
    ///   that delay at full leverage.
    ///
    /// Every restart holds the estimate while the line refills. Each case is O(1) and touches
    /// only the inline ring. Returns whether the point was stored.
    fn add(&mut self, frame: u64, time_ns: i64, kind: Kind, domain: Domain) -> bool {
        if self.kind != Some((kind, domain)) {
            self.restart(kind, domain);
        }
        if let Some((t, f)) = self.newest() {
            if time_ns < t || frame < f {
                self.restart(kind, domain);
            } else {
                let late = (time_ns - t) as f64 - (frame - f) as f64 / self.hz * 1e9;
                if kind == Kind::Position {
                    if late > STALL_SLACK_NS as f64 {
                        self.restart(kind, domain);
                    } else if time_ns - t < SPACING_NS {
                        return false;
                    }
                } else if self.outliers > 0 && time_ns - self.outlier_last < SPACING_NS {
                    // The blocks that arrive with a late one carry no new time: neither judged nor
                    // stored.
                    return false;
                } else if late < -(OUTLIER_NS as f64) {
                    // An arrival delay is never negative: a point this early proves the newest
                    // stored point arrived late. Start again on this one.
                    self.restart(kind, domain);
                } else if late > OUTLIER_NS as f64 {
                    if self.outliers == 0 {
                        self.outlier_since = time_ns;
                    }
                    (self.outliers, self.outlier_last) = (self.outliers + 1, time_ns);
                    if self.outliers < CONFIRM_POINTS || time_ns - self.outlier_since < CONFIRM_NS {
                        return false;
                    }
                    self.restart(kind, domain);
                } else {
                    self.outliers = 0;
                    if self.provisional && late < 0.0 {
                        // A point earlier than the line's only point predicts carries less delay:
                        // it becomes the anchor (a block that arrived with the anchor, or the first
                        // one after it).
                        let i = (self.head + N - 1) % N;
                        (self.time[i], self.frame[i]) = (time_ns, frame);
                        self.provisional = time_ns - self.anchor_at < SPACING_NS;
                        return true;
                    }
                    if time_ns - t < SPACING_NS {
                        return false;
                    }
                    self.provisional = false;
                }
            }
        }
        if self.len == 0 {
            self.anchor_at = time_ns;
        }
        self.time[self.head] = time_ns;
        self.frame[self.head] = frame;
        self.head = (self.head + 1) % N;
        self.len = (self.len + 1).min(N);
        true
    }

    fn domain(&self) -> Option<Domain> {
        self.kind.map(|(_, domain)| domain)
    }

    /// The span of the fit: [`WINDOW_NS`] for a position-timed line, every point the ring holds up
    /// to [`ARRIVAL_WINDOW_NS`] for an arrival-timed one.
    fn window_ns(&self) -> i64 {
        match self.kind {
            Some((Kind::Position, _)) => WINDOW_NS,
            _ => ARRIVAL_WINDOW_NS,
        }
    }

    /// Frames per second of the time base: the least-squares slope over the points of the line's
    /// window, once they span [`MIN_SPAN_NS`]. Centered on the newest point, in f64.
    fn slope(&self) -> Option<f64> {
        let (t0, f0) = self.newest()?;
        let window = self.window_ns();
        let (mut n, mut sx, mut sy, mut sxx, mut sxy, mut span) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0i64);
        let mut count = 0usize;
        for k in 0..self.len {
            let i = (self.head + N - 1 - k) % N;
            let age = t0 - self.time[i];
            if age > window {
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
    endpoint: Line<POINTS>,
    device: Line<ARRIVAL_POINTS>,
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

    /// Both sides on exact time, 480 frames per 10 ms, for `points` points; `device_late(k)` is how
    /// late the device's point `k` arrives, in ns. Returns the estimate after each point and the
    /// device line's length.
    fn run_device_late(points: u64, device_late: &dyn Fn(u64) -> i64) -> (Box<RateEstimate>, Vec<(Option<f64>, usize)>) {
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        let mut seen = Vec::new();
        for k in 0..points {
            let t = (k * 10_000_000) as i64;
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
            est.device(k * 480, t + device_late(k), Kind::Host, Domain::Qpc);
            seen.push((est.drift(), est.device.len));
        }
        (est, seen)
    }

    #[test]
    fn a_late_arrival_that_the_next_blocks_catch_up_is_ignored_and_the_reading_holds() {
        for late_ms in [21, 45, 60, 120, 500] {
            // Point 1000 arrives late; the points due inside the delay arrive with it.
            let late = late_ms * 1_000_000;
            let (_, seen) = run_device_late(2000, &|k| {
                let behind = (k as i64 - 1000) * 10_000_000;
                if (0..late).contains(&behind) { late - behind } else { 0 }
            });
            for (k, (drift, len)) in seen.iter().enumerate() {
                if let Some(d) = drift {
                    assert!(d.abs() < 1e-6, "{late_ms} ms late: the reading left its bound ({d}) at point {k}");
                }
                if k > 0 && k < 2000 {
                    assert!(*len >= seen[k - 1].1, "{late_ms} ms late: the device line restarted at point {k}");
                }
            }
            assert!(seen[1999].0.is_some());
        }
    }

    #[test]
    fn a_lasting_offset_on_an_arrival_timed_line_restarts_it_once_confirmed() {
        // From point 1000 on, every device point is 200 ms late: the device lost frames.
        let (est, seen) = run_device_late(2000, &|k| if k >= 1000 { 200_000_000 } else { 0 });
        let first = 1000 + seen[1000..].iter().position(|(_, len)| *len == 1).expect("the line restarted");
        assert_eq!(first, 1030, "outliers from point 1000 confirm the offset once they span 300 ms");
        assert!(seen[1000..1030].iter().all(|(_, len)| *len == 1000), "the outliers are not stored");
        for (k, (drift, _)) in seen.iter().enumerate() {
            if let Some(d) = drift {
                assert!(d.abs() < 1e-6, "the reading left its bound ({d}) at point {k}: held while the line refills");
            }
        }
        assert_eq!(est.device.len, 970);
    }

    #[test]
    fn a_lasting_offset_that_ends_restarts_the_line_again_on_the_first_point_back_on_time() {
        // 8192-frame callbacks (170.7 ms apart): callbacks 400 to 699 are 60 ms late (a lasting
        // offset), then the device is on time again. Callback 700 arrives 110.7 ms after 699, 60 ms
        // earlier than the line predicts but later in time, so only the early-point rule can catch
        // it. (At 10 ms callbacks the end of an offset over 10 ms steps the time backward, which
        // restarted the line already.)
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        let (mut restarts, mut prev_len, mut last_estimate) = (Vec::new(), 0usize, None);
        for k in 0..1400u64 {
            let t = (k * 8192) as f64 / 48_000.0;
            let late = if (400..700).contains(&k) { 0.06 } else { 0.0 };
            est.device(k * 8192, ((t + late) * 1e9) as i64, Kind::Host, Domain::Qpc);
            let packets = ((k * 8192) / 480)..(((k + 1) * 8192) / 480);
            for p in packets {
                est.endpoint(Obs { first_frame: p * 480, frames: 480, time_ns: (p * 10_000_000) as i64, kind: Kind::Position, domain: Domain::Qpc });
            }
            if est.device.len < prev_len {
                restarts.push(k);
            }
            prev_len = est.device.len;
            if let Some(d) = est.drift() {
                assert!(d.abs() < 1e-6, "the reading left its bound ({d}) at callback {k}: held while the line refills");
                last_estimate = Some(k);
            }
        }
        assert_eq!(restarts, [402, 700], "confirmed on the third outlier (341 ms), then on the first callback 60 ms early");
        assert_eq!(last_estimate, Some(1399));
        // From callback 700 (701 if a nanosecond of rounding put it a hair early: it then replaced
        // the anchor).
        assert!((699..=700).contains(&est.device.len), "the line refilled from callback 700: {}", est.device.len);
    }

    #[test]
    fn a_restarted_line_takes_the_block_with_the_least_delay_as_its_anchor() {
        // A 200 ms offset confirms at point 1030, which restarts the line on it; point 1031
        // arrives 2 ms after it, 8 ms earlier than it predicts, and so does every later point (the
        // offset is 192 ms). Point 1031 replaces the anchor instead of being skipped, so the
        // refilled line carries no 8 ms step at its oldest end.
        let (est, seen) = run_device_late(2000, &|k| match k {
            1000..=1030 => 200_000_000,
            1031.. => 192_000_000,
            _ => 0,
        });
        assert_eq!(seen[1030].1, 1, "the line restarted on the confirming point");
        assert_eq!(seen[1031].1, 1, "the point with less delay replaced the anchor");
        assert_eq!(est.device.len, 2000 - 1031);
        for (k, (drift, _)) in seen.iter().enumerate() {
            if let Some(d) = drift {
                assert!(d.abs() < 1e-6, "the reading left its bound ({d}) at point {k}");
            }
        }
        assert!(seen[1999].0.is_some());
    }

    #[test]
    fn a_device_a_little_fast_against_the_time_base_still_grows_its_line() {
        // 128-frame callbacks whose times run 50 ppm short: every point is a little earlier than
        // the newest one predicts. The anchor is replaced only within 10 ms of the first one (and
        // once more), so the line grows and reads the drift.
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        for k in 0..11_250u64 {
            let t = (k * 128) as f64 / 48_000.0;
            est.device(k * 128, (t * (1.0 - 50e-6) * 1e9) as i64, Kind::Host, Domain::Qpc);
            let packet = (t * 100.0) as u64;
            est.endpoint(Obs {
                first_frame: packet * 480,
                frames: 480,
                time_ns: (packet * 10_000_000) as i64,
                kind: Kind::Position,
                domain: Domain::Qpc,
            });
        }
        assert!(est.device.len >= 2000, "the device line grew: {}", est.device.len);
        let got = est.drift().expect("an estimate after 30 s") * 1e6;
        assert!((got + 50.0).abs() < 1.0, "the device timed 50 ppm fast reads the endpoint 50 ppm slow: {got}");
    }
}
