//! The rate ratio of the System Audio endpoint's clock to the input device's clock (#268), the
//! estimate ADR 0017 prescribes for a second clock: frame counts against one shared time base.
//!
//! Each side keeps a line of (time, frame) points: the loopback stream's packets (the frame index
//! of a packet's first frame and the QPC time WASAPI gives for it) and the input stream's callbacks
//! (the frames read so far and the callback's arrival time). A fit over each line's window gives
//! each side's frames per second of the shared clock, and their ratio over the nominal one is the
//! drift. It depends only on the counts and the times, never on how the packets and the callbacks
//! line up, so a 480-frame callback beside 480-frame packets reads the same as any other.
//!
//! Both sides are timed on one clock domain, QPC ([`Domain::Qpc`]). The loopback packets carry the
//! QPC time WASAPI gives for their first frame ([`Kind::Position`]). The input callbacks carry the
//! stamp's host callback time on every backend ([`Kind::Host`]): the QPC read when the device's
//! block arrived, never the backend's own frame time, which on native ASIO is the driver's sample
//! position at the nominal rate and so always runs exactly at nominal against the interface's real
//! clock. An invalid stamp falls back to the reader's own monotonic clock ([`Kind::Arrival`], QPC
//! on Windows). Each line carries its kind and domain; a change of either starts the line again
//! (which also restarts it at a discontinuity), and a pair whose domains differ holds the
//! estimate instead of computing a ratio between two clocks that share no base.
//!
//! **The two kinds of time behave differently, and each line follows its kind.** A position time
//! is exact: its line is fitted by least squares over [`WINDOW_NS`], and when a point's time step
//! exceeds the time its frames take by more than [`STALL_SLACK_NS`] (a loopback that paused and
//! resumed without a discontinuity flag), the offset is permanent, so the line restarts on that
//! point at once and the fit never spans the step.
//!
//! An arrival time (a host callback time or the reader's own clock) is the true time plus a delay
//! that is never negative: a few milliseconds of jitter, and now and then one block or a spell of
//! blocks tens of milliseconds late, easing off within a few seconds (a lateness that eases off over
//! 8 to 20 s reads 100 to 379 ppm off at 128 frames and up to 74 ppm at 8192, the class of a plain
//! fit). So the device's clock is the *lower
//! envelope* of the arrival times, and an arrival-timed line is fitted to that envelope, not to
//! every point ([`Line::add`] stores every point; [`Line::envelope_fit`] chooses): the lower convex
//! hull of the window's points (time over frame), the hull edge whose line has the most points
//! within [`BAND_NS`] above it (a lower support line: no point lies below it), and a least-squares
//! line through those points only. A late point lies above that line by its delay; once the delay
//! exceeds [`BAND_NS`] it has no weight at all, wherever it sits in the window and however late it
//! is, so nothing restarts on a late point and a spell of lateness that ends leaves only a
//! stretch of excluded points. Earlier rounds anchored a restarted line on a delayed point and
//! carried that delay at full leverage (the fourth, fifth and sixth reviews of #268); the envelope
//! fit gives no point that leverage. Only an offset that lasts restarts the line, on the point
//! that shows it: points that stay above the band for [`CONFIRM_NS`] and [`CONFIRM_POINTS`] (a
//! device that lost frames), or a point more than [`BELOW_NS`] below the envelope, which no delay
//! can put there (the older points carried a delay: a lasting offset that ended). After any
//! restart the estimate is held until the refilled line's fit is as well conditioned as a long
//! line's ([`HOLD_SE`]). A lasting offset within about the band (under 1 to 3 ms, with the jitter)
//! is neither: the fit spans it for one window (K100).
//!
//! Arrival jitter is averaged by a longer window: every point of the [`ARRIVAL_POINTS`] ring, up to
//! [`ARRIVAL_WINDOW_NS`] (20 s at 10 ms callbacks, 60 s at 8192-frame ones).
//!
//! Audio-thread code: the points live in fixed arrays inside the struct and the hull in a fixed
//! array on the stack (no allocation, no lock, no wait); a fit is a few passes over a line's ring
//! and runs at most once per callback.

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
/// count through the next point's index). The blocks that catch up behind a late one arrive with
/// it, so they are skipped too.
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
/// The band of an arrival-timed line's fit: the points no more than this above the lower support
/// line are fitted, the rest have no weight. 1 ms because:
/// - a late point's leverage is capped at this, whatever its delay;
/// - it holds a fair share of the points under arrival jitter (a third of them with 3 ms of
///   uniform jitter, a tenth with 10 ms), so the fit still averages;
/// - the points it holds spread over at most 1 ms (a standard deviation of 0.29 ms), a third of
///   the spread of 3 ms of jitter;
/// - it is far above what bends a straight line over the window: QPC's 100 ns resolution and a
///   thermal drift of 20 ppm a minute (0.15 ms of curvature over 60 s).
pub(crate) const BAND_NS: f64 = 1_000_000.0;
/// A point more than this below the envelope of the last fit restarts an arrival-timed line: no
/// delay can put it there, so the older points carried one (a lasting offset that ended, or a line
/// that started on late points). A quarter of the band: far above how far the envelope's line can
/// miss a new point (it joins two of the lowest points, seconds apart, so it is good to a few ppm:
/// tens of microseconds over the window), and small enough that a point just inside it, which
/// joins the hull, tilts a 20 s fit by at most 12 ppm.
pub(crate) const BELOW_NS: f64 = 250_000.0;
/// The fit's hull drops a vertex that lies less than this below the line past it (20 us, a
/// fiftieth of the band): it changes the band by no more than that and keeps a near-straight run
/// of points one edge.
const HULL_SLACK_NS: f64 = 20_000.0;
/// Points that stay above the band, none back in it, for this long of arrival time are a lasting
/// offset (a device that lost frames): the line restarts on them. Longer than a spell of lateness
/// (the tests' longest is 2 s), so a spell never restarts the line; while the offset waits for
/// this, the fit keeps the line before it, whose slope is right.
pub(crate) const CONFIRM_NS: i64 = 3_000_000_000;
/// ... and there are at least this many of them: with 3 ms of uniform jitter two points in three
/// lie above the band, and 32 of them in a row happen once in about 400 000 (at 8192 frames, 32
/// points take 5.5 s).
pub(crate) const CONFIRM_POINTS: u32 = 32;
/// A line gives a slope once its points (an arrival-timed line: the points in its band) span this
/// long.
pub(crate) const MIN_SPAN_NS: i64 = 2_000_000_000;
/// ... and hold at least this many points.
const MIN_POINTS: usize = 16;
/// A held estimate is replaced only by one whose arrival-timed slope has a standard error within
/// this (relative), taking the band's points as spread evenly over [`BAND_NS`]; or within the held
/// estimate's own, if that is larger (the first estimates, while the line first fills). A line
/// that restarted therefore resumes only once it is as well conditioned as the jitter rows
/// assume: 4 ppm is a fifth of their 20 ppm bound. It takes about 9 s of 10 ms callbacks (12 s
/// with 3 ms of jitter, which keeps a third of the points in the band) and 22 s of 8192-frame ones
/// (32 s with jitter).
pub(crate) const HOLD_SE: f64 = 4e-6;

/// The lower envelope of an arrival-timed line's last fit: the lower support line of its band at
/// the fitted slope, and how well that slope is known.
#[derive(Debug, Clone, Copy)]
struct Envelope {
    /// The time the line gives for `frame` (ns of the shared time base), and its slope (ns per
    /// frame).
    frame: u64,
    time: f64,
    slope: f64,
    /// The frame at the band's center, and the slope's standard error (ns per frame).
    center: f64,
    slope_se: f64,
}

impl Envelope {
    /// How far the point (`frame`, `time_ns`) lies above the line (negative: below), and how far
    /// the line itself can miss it: three standard errors of the slope over the point's distance
    /// from the band's center (a young line's envelope is judged with that much slack).
    fn judge(&self, frame: u64, time_ns: i64) -> (f64, f64) {
        let above = time_ns as f64 - self.time - (frame as f64 - self.frame as f64) * self.slope;
        (above, 3.0 * self.slope_se * (frame as f64 - self.center).abs())
    }
}

/// One clock's (time, frame) points in a fixed ring of `N`.
pub(crate) struct Line<const N: usize> {
    time: [i64; N],
    frame: [u64; N],
    /// The next slot to write.
    head: usize,
    len: usize,
    kind: Option<(Kind, Domain)>,
    /// The side's nominal frames per second, for the stall rule and the fit's coordinates.
    hz: f64,
    /// The lower envelope the last fit found (an arrival-timed line).
    envelope: Option<Envelope>,
    /// Points stored in a row more than [`BAND_NS`] above the envelope, and the first one's time.
    stale: u32,
    stale_since: i64,
    /// The last fit ([`Self::fit`]), and whether a point was stored or the line restarted since.
    fitted: Option<(f64, f64)>,
    refit: bool,
}

impl<const N: usize> Line<N> {
    fn new(hz: f64) -> Self {
        const { assert!(N <= 1 << 16, "the hull indexes the ring with u16") };
        Self { time: [0; N], frame: [0; N], head: 0, len: 0, kind: None, hz, envelope: None, stale: 0, stale_since: 0, fitted: None, refit: false }
    }

    fn reset(&mut self) {
        (self.head, self.len, self.kind, self.envelope, self.stale) = (0, 0, None, None, 0);
        (self.fitted, self.refit) = (None, false);
    }

    /// Start again on a line of `kind` and `domain` (the next stored point is its first).
    fn restart(&mut self, kind: Kind, domain: Domain) {
        self.reset();
        self.kind = Some((kind, domain));
    }

    /// The `j`th point of the line, oldest first.
    fn at(&self, j: usize) -> (i64, u64) {
        let i = (self.head + N - self.len + j) % N;
        (self.time[i], self.frame[i])
    }

    fn newest(&self) -> Option<(i64, u64)> {
        (self.len > 0).then(|| self.at(self.len - 1))
    }

    /// Store the point (`frame`, `time_ns`) unless it is closer than [`SPACING_NS`] to the last
    /// one. A change of kind or domain, a time going backward or a frame index going backward
    /// starts the line again.
    ///
    /// On a position-timed line, a point whose time step exceeds its frames' time at the nominal
    /// rate by more than [`STALL_SLACK_NS`] restarts the line on it.
    ///
    /// An arrival-timed line stores every point, late ones included: its fit
    /// ([`Self::envelope_fit`]) gives a point more than [`BAND_NS`] above the lower envelope no
    /// weight. The point is judged against the envelope of the last fit ([`Self::judge`]) only to
    /// find a lasting offset, which restarts the line on the point: [`CONFIRM_POINTS`] points in a
    /// row over [`CONFIRM_NS`] above the band (an offset that began), or one point more than
    /// [`BELOW_NS`] below the envelope (the older points carried a delay: an offset that ended, or
    /// a line that started on late points). The estimate is held after a restart until the
    /// refilled line is well conditioned ([`HOLD_SE`]).
    ///
    /// O(1), touching only the inline ring. Returns whether the point was stored.
    fn add(&mut self, frame: u64, time_ns: i64, kind: Kind, domain: Domain) -> bool {
        if self.kind != Some((kind, domain)) {
            self.restart(kind, domain);
        }
        if let Some((t, f)) = self.newest() {
            let late = (time_ns - t) as f64 - frame.saturating_sub(f) as f64 / self.hz * 1e9;
            if time_ns < t || frame < f || (kind == Kind::Position && late > STALL_SLACK_NS as f64) {
                self.restart(kind, domain);
            } else if time_ns - t < SPACING_NS {
                return false;
            }
        }
        self.time[self.head] = time_ns;
        self.frame[self.head] = frame;
        self.head = (self.head + 1) % N;
        self.len = (self.len + 1).min(N);
        self.refit = true;
        if kind != Kind::Position {
            self.judge(frame, time_ns);
        }
        true
    }

    /// Judge the stored point against the last envelope: restart the line on it when it lies
    /// more than [`BELOW_NS`] (plus the envelope's slack) below it (the older points carried a
    /// delay), or when it confirms a lasting offset above it.
    fn judge(&mut self, frame: u64, time_ns: i64) {
        let Some(envelope) = self.envelope else { return };
        let (above, slack) = envelope.judge(frame, time_ns);
        if (-BELOW_NS - slack..=BAND_NS + slack).contains(&above) {
            self.stale = 0;
            return;
        }
        if above > 0.0 {
            if self.stale == 0 {
                self.stale_since = time_ns;
            }
            self.stale += 1;
            if self.stale < CONFIRM_POINTS || time_ns - self.stale_since < CONFIRM_NS {
                return;
            }
        }
        // Keep only this point: the line starts again on it.
        (self.len, self.envelope, self.stale) = (1, None, 0);
    }

    fn domain(&self) -> Option<Domain> {
        self.kind.map(|(_, domain)| domain)
    }

    /// Frames per second of the time base and the slope's relative standard error, once the
    /// line's fitted points span [`MIN_SPAN_NS`] and number [`MIN_POINTS`]: least squares over
    /// [`WINDOW_NS`] for a position-timed line (its error taken as nil: exact stamps), the
    /// envelope fit for an arrival-timed one. Refitted only when a point was stored since the last
    /// fit, so the device's line is fitted at most once per [`SPACING_NS`].
    fn fit(&mut self) -> Option<(f64, f64)> {
        if std::mem::take(&mut self.refit) {
            self.fitted = match self.kind {
                Some((Kind::Position, _)) => self.slope().map(|s| (s, 0.0)),
                Some(_) => self.envelope_fit(),
                None => None,
            };
        }
        self.fitted
    }

    /// Frames per second of the time base: the least-squares slope over the points of the last
    /// [`WINDOW_NS`], once they span [`MIN_SPAN_NS`]. Centered on the newest point, in f64.
    fn slope(&self) -> Option<f64> {
        let (t0, f0) = self.newest()?;
        let (mut n, mut sx, mut sy, mut sxx, mut sxy, mut span) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0i64);
        let mut count = 0usize;
        for j in (0..self.len).rev() {
            let (t, f) = self.at(j);
            let age = t0 - t;
            if age > WINDOW_NS {
                break;
            }
            let x = -(age as f64) * 1e-9;
            let y = -((f0 - f) as f64);
            (n, sx, sy, sxx, sxy) = (n + 1.0, sx + x, sy + y, sxx + x * x, sxy + x * y);
            (span, count) = (age, count + 1);
        }
        if count < MIN_POINTS || span < MIN_SPAN_NS {
            return None;
        }
        let d = n * sxx - sx * sx;
        (d > 0.0).then(|| (n * sxy - sx * sy) / d)
    }

    /// The fit of an arrival-timed line to the lower envelope of its points, over every point of
    /// the ring up to [`ARRIVAL_WINDOW_NS`]. In coordinates centered on the newest point (`x` the
    /// frames before it, `y` the time less `x` frames at the nominal rate, in ns):
    /// 1. the lower convex hull of the points (Andrew's monotone chain; the ring is in frame
    ///    order);
    /// 2. of the hull's edges (each a lower support line: no point lies below it), the one whose
    ///    line has the most points within [`BAND_NS`] above it (on a tie, the newer);
    /// 3. least squares (time on frame) over those points only.
    ///
    /// A delay larger than [`BAND_NS`] puts a point outside the band, so it has no weight; a spell
    /// of late points leaves the edge under it, which bridges the points on either side. The
    /// slope's relative standard error takes the band's spread as [`BAND_NS`] evenly (deterministic,
    /// whatever the jitter). Returns the frames per second and that error once the band's points
    /// span [`MIN_SPAN_NS`] and number [`MIN_POINTS`], and keeps the chosen edge's line for
    /// [`Self::judge`]. A few passes over the window (the hull pass, the edges' counts, which
    /// cover the points near each edge's line only, and the band's least squares), no allocation.
    ///
    /// Cost (release, a full 2048-point ring at 10 ms): 16 to 28 us for ordinary arrival patterns;
    /// the worst I could construct (repeated convex arcs of 8 to 128 points) is 52 us. It runs once
    /// per stored point and at most once per callback, so 52 us is 2 % of a 128-frame callback.
    fn envelope_fit(&mut self) -> Option<(f64, f64)> {
        let (t0, f0) = self.newest()?;
        let k0 = 1e9 / self.hz;
        let point = |j: usize| {
            let (t, f) = self.at(j);
            let x = -((f0 - f) as f64);
            (x, (t - t0) as f64 - x * k0)
        };
        // The window's oldest point (the times are in order).
        let (mut first, mut last) = (0usize, self.len - 1);
        while first < last {
            let mid = (first + last) / 2;
            if t0 - self.at(mid).0 > ARRIVAL_WINDOW_NS {
                first = mid + 1;
            } else {
                last = mid;
            }
        }
        // The lower hull, the top two vertices kept at hand. A vertex is dropped when it lies less
        // than HULL_SLACK_NS below the line past it, so a near-straight run (exact times rounded
        // to the ns, or the floor of the jitter) is one edge, not hundreds.
        let mut hull = [0u16; N];
        let (mut h, mut o, mut a) = (0usize, (0.0f64, 0.0f64), (0.0f64, 0.0f64));
        for j in first..self.len {
            let p = point(j);
            while h >= 2 && (a.0 - o.0) * (p.1 - o.1) - (a.1 - o.1) * (p.0 - o.0) <= HULL_SLACK_NS * (p.0 - o.0) {
                (h, a) = (h - 1, o);
                if h >= 2 {
                    o = point(usize::from(hull[h - 2]));
                }
            }
            hull[h] = j as u16;
            (h, o, a) = (h + 1, a, p);
        }
        // Each edge's band count, over the points between the hull vertices where the hull is
        // still within the band (plus the slack) of the edge's line: past them the hull, and every
        // point above it, is higher.
        let mut best: Option<(usize, (f64, f64), f64, usize, usize)> = None;
        for e in 1..h {
            let (a, b) = (point(usize::from(hull[e - 1])), point(usize::from(hull[e])));
            if b.0 <= a.0 {
                continue;
            }
            let m = (b.1 - a.1) / (b.0 - a.0);
            let above = |p: (f64, f64)| p.1 - a.1 - (p.0 - a.0) * m;
            let reach = BAND_NS + HULL_SLACK_NS;
            let (mut lo, mut hi) = (e - 1, e);
            while lo > 0 && above(point(usize::from(hull[lo]))) <= reach {
                lo -= 1;
            }
            while hi + 1 < h && above(point(usize::from(hull[hi]))) <= reach {
                hi += 1;
            }
            let (lo, hi) = (usize::from(hull[lo]), usize::from(hull[hi]));
            let count = (lo..=hi).filter(|&j| above(point(j)) <= BAND_NS).count();
            if best.is_none_or(|(c, ..)| count >= c) {
                best = Some((count, a, m, lo, hi));
            }
        }
        let (count, a, m, lo, hi) = best?;
        if count < MIN_POINTS {
            return None;
        }
        // Least squares (time on frame) over the band's points.
        let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let (mut oldest, mut newest) = (f64::MAX, f64::MIN);
        for j in lo..=hi {
            let p = point(j);
            if p.1 - a.1 - (p.0 - a.0) * m <= BAND_NS {
                (n, sx, sy, sxx, sxy) = (n + 1.0, sx + p.0, sy + p.1, sxx + p.0 * p.0, sxy + p.0 * p.1);
                (oldest, newest) = (oldest.min(p.0), newest.max(p.0));
            }
        }
        let spread = sxx - sx * sx / n;
        if spread <= 0.0 || (newest - oldest) * k0 < MIN_SPAN_NS as f64 {
            return None;
        }
        let slope = (sxy - sx * sy / n) / spread;
        let slope_se = BAND_NS / 12f64.sqrt() / spread.sqrt();
        // The band's lower support line at the fitted slope, for judging the next points.
        let support = (lo..=hi).map(point).filter(|p| p.1 - a.1 - (p.0 - a.0) * m <= BAND_NS).map(|p| p.1 - slope * p.0).fold(f64::MAX, f64::min);
        let k = k0 + slope;
        self.envelope = Some(Envelope { frame: f0, time: t0 as f64 + support, slope: k, center: f0 as f64 + sx / n, slope_se });
        Some((1e9 / k, slope_se / k))
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
    /// The relative standard error of the held estimate's fit ([`HOLD_SE`]).
    held_se: f64,
}

impl RateEstimate {
    /// For an endpoint at `endpoint_hz` and a device at `device_hz` frames per second.
    pub(crate) fn new(endpoint_hz: f64, device_hz: f64) -> Self {
        Self {
            endpoint: Line::new(endpoint_hz),
            device: Line::new(device_hz),
            nominal: endpoint_hz / device_hz,
            dirty: false,
            drift: None,
            held_se: f64::INFINITY,
        }
    }

    /// Start again for another stream pair, in place (no large temporary on the audio thread).
    pub(crate) fn restart(&mut self, endpoint_hz: f64, device_hz: f64) {
        self.endpoint.reset();
        self.device.reset();
        (self.endpoint.hz, self.device.hz) = (endpoint_hz, device_hz);
        (self.nominal, self.dirty, self.drift, self.held_se) = (endpoint_hz / device_hz, false, None, f64::INFINITY);
    }

    /// A loopback packet.
    pub(crate) fn endpoint(&mut self, obs: Obs) {
        self.dirty |= self.endpoint.add(obs.first_frame, obs.time_ns, obs.kind, obs.domain);
    }

    /// An input callback starting at device frame `first_frame`.
    pub(crate) fn device(&mut self, first_frame: u64, time_ns: i64, kind: Kind, domain: Domain) {
        self.dirty |= self.device.add(first_frame, time_ns, kind, domain);
    }

    /// The drift estimate, refitted when a point was stored; `None` until both lines' fits span
    /// [`MIN_SPAN_NS`]. A fit replaces the held estimate only when its standard error is within
    /// [`HOLD_SE`] or the held one's, so a line that restarted resumes once it is well conditioned.
    pub(crate) fn drift(&mut self) -> Option<f64> {
        if std::mem::take(&mut self.dirty)
            && let (Some((e, e_se)), Some((d, d_se))) = (self.endpoint.fit(), self.device.fit())
            && d > 0.0
            && e > 0.0
            && self.endpoint.domain() == self.device.domain()
        {
            let se = e_se.hypot(d_se);
            if se <= self.held_se.max(HOLD_SE) {
                (self.drift, self.held_se) = (Some(e / d / self.nominal - 1.0), se);
            }
        }
        self.drift
    }
}

#[cfg(test)]
#[path = "system_audio_rate_tests.rs"]
mod tests;
