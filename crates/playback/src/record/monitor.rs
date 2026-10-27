use super::finite;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Lifetime monitor diagnostics. Independent fields may straddle a callback.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MonitorTelemetry {
    pub overflow_frames: u64,
    pub underrun_frames: u64,
    pub input_contention_frames: u64,
    pub output_contention_frames: u64,
    pub input_fault_frames: u64,
    pub output_fault_frames: u64,
    /// Frames deliberately discarded at clear or after a real overflow.
    pub discarded_frames: u64,
    pub overflow_recoveries: u64,
    pub incomplete_input_samples: u64,
    /// Requested output frames that did not fit the supplied channel storage.
    pub short_output_frames: u64,
}

#[derive(Default)]
struct MonitorCounters {
    overflow: AtomicU64,
    underrun: AtomicU64,
    input_contention: AtomicU64,
    output_contention: AtomicU64,
    input_fault: AtomicU64,
    output_fault: AtomicU64,
    discarded: AtomicU64,
    recoveries: AtomicU64,
    incomplete: AtomicU64,
    short_output: AtomicU64,
}

struct MonitorReader {
    queue: rtrb::Consumer<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    left_valid: bool,
    right_valid: bool,
    phase: f64,
    removed_frames: u64,
}

impl MonitorReader {
    fn frame(queue: &mut rtrb::Consumer<f32>, frame: &mut [f32], removed: &mut u64) -> bool {
        if queue.slots() < frame.len() {
            return false;
        }
        for sample in frame {
            // Only this serialized consumer pops; a producer cannot remove available samples.
            *sample = queue.pop().unwrap_or(0.0);
        }
        *removed = removed.wrapping_add(1);
        true
    }

    fn history_frames(&self) -> usize {
        // `left` is already consumed when phase >= 1; a prefetched right is still pending.
        usize::from(self.left_valid && self.phase < 1.0) + usize::from(self.right_valid)
    }

    fn retired_input(&self) -> u64 {
        self.removed_frames.wrapping_sub(self.history_frames() as u64)
    }

    /// Whole input frames queued now.
    fn queued_frames(&self) -> usize {
        self.queue.slots() / self.left.len()
    }

    /// Discard the oldest queued frames, and the interpolation history in front of them, until at
    /// most `keep` remain queued. Returns the frames discarded. One queue commit: a pointer move.
    fn align(&mut self, keep: usize) -> usize {
        let ch = self.left.len();
        let excess = self.queued_frames().saturating_sub(keep);
        if excess == 0 {
            return 0;
        }
        if let Ok(chunk) = self.queue.read_chunk(excess * ch) {
            chunk.commit_all();
        }
        self.removed_frames = self.removed_frames.wrapping_add(excess as u64);
        let held = self.history_frames();
        self.left_valid = false;
        self.right_valid = false;
        self.phase = 0.0;
        excess + held
    }

    /// Discard only the frames present at entry, never chase a running producer.
    fn clear(&mut self) -> usize {
        let ch = self.left.len();
        let queued = self.queue.slots() / ch;
        if let Ok(chunk) = self.queue.read_chunk(queued * ch) {
            // Commit the discard in one queue operation; f32 carries no destructor.
            chunk.commit_all();
        }
        self.removed_frames = self.removed_frames.wrapping_add(queued as u64);
        let held = self.history_frames();
        self.left_valid = false;
        self.right_valid = false;
        self.phase = 0.0;
        queued + held
    }

    fn render(&mut self, out: &mut [Vec<f32>], frames: usize, step: f64) -> usize {
        let mut written = 0;
        for f in 0..frames {
            if !self.left_valid {
                self.left_valid = Self::frame(&mut self.queue, &mut self.left, &mut self.removed_frames);
                if !self.left_valid {
                    break;
                }
            }
            while self.phase >= 1.0 {
                if self.right_valid {
                    std::mem::swap(&mut self.left, &mut self.right);
                    self.right_valid = false;
                } else if !Self::frame(&mut self.queue, &mut self.left, &mut self.removed_frames) {
                    return written;
                }
                self.phase -= 1.0;
            }
            // An exact sample needs no lookahead. Equal rates therefore add no interpolation
            // frame of latency and consume the final available frame, even for a one-frame call.
            if self.phase > 0.0 && !self.right_valid {
                self.right_valid = Self::frame(&mut self.queue, &mut self.right, &mut self.removed_frames);
                if !self.right_valid {
                    break;
                }
            }
            let fraction = self.phase as f32;
            for ((channel, &a), &b) in out.iter_mut().zip(&self.left).zip(&self.right) {
                if let Some(x) = channel.get_mut(f) {
                    *x = if fraction == 0.0 { a } else { finite(a * (1.0 - fraction) + b * fraction) };
                }
            }
            self.phase += step;
            written += 1;
        }
        written
    }
}

/// Read blocks of input left queued after the read that aligns the ring (see
/// [`InputRing::align_on_next_read`]) when one device callback delivers the capture block and
/// then asks for the render block (native ASIO duplex: one `bufferSwitch`). The aligning read
/// always finds this cycle's capture already queued, so it needs no slack: the backlog B is 0.
pub const ALIGNED_BACKLOG_BLOCKS_PAIRED: usize = 0;
/// The same when capture and render run on separate callbacks (WASAPI's capture and render
/// loops, cpal, an input on another device). An output callback can come before the input
/// callback of the same period, so the slack is the larger of one read block and the last block
/// the capture callback pushed, `max(read, last_push)` frames, kept on top of the read itself.
/// The reader then does not run dry before the next capture arrives while the capture period
/// P_c is at most `read + max(read, last_push)`, which holds whenever P_c is the push size: a
/// 10 ms WASAPI capture (480 frames) feeding a 128-frame render keeps 480 + 128, not 256. B is
/// that slack, fixed when the ring aligns.
pub const ALIGNED_BACKLOG_BLOCKS_UNPAIRED: usize = 1;

/// A bounded interleaved FIFO for input monitoring. The input and output roles have
/// separate locks, so they never exclude one another. Each callback only tries its own
/// role lock; contention from a second caller or a control-side clear is counted.
///
/// Normal callback partitioning never discards audio. Full storage rejects whole new
/// frames and requests a counted flush on the next read. A smaller stall can leave a
/// persistent backlog, exposed by [`Self::buffered_input_frames`]. The one latency policy is
/// alignment (K22): the first read after a plan adopts the ring, and after any
/// [`Self::clear_monitor`], discards the oldest queued frames so that only the read itself plus
/// [`ALIGNED_BACKLOG_BLOCKS_PAIRED`] (or `_UNPAIRED`) read blocks remain, whatever queued while
/// nothing was reading. Otherwise the reader never trims the FIFO. Rate conversion is currently linear interpolation: it is
/// not bandlimited SRC or compensation for independent device-clock drift.
pub struct InputRing {
    producer: Mutex<rtrb::Producer<f32>>,
    reader: Mutex<MonitorReader>,
    /// Device channel count, for consumers to size their prepared storage. Read-only by contract.
    pub channels: std::sync::atomic::AtomicUsize,
    ch: usize,
    fifo_frames: usize,
    accepted_frames: AtomicU64,
    retired_frames: AtomicU64,
    /// Input device sample rate in Hz (0 when unknown, which reads as "same as the session").
    rate: u32,
    overrun_pending: std::sync::atomic::AtomicBool,
    clear_pending: std::sync::atomic::AtomicBool,
    /// The next read that finds input discards the backlog down to the target (K22).
    align_pending: std::sync::atomic::AtomicBool,
    /// Multiplier of the slack `max(read, last_push)` kept at alignment:
    /// [`ALIGNED_BACKLOG_BLOCKS_PAIRED`] or `_UNPAIRED`.
    aligned_backlog_blocks: usize,
    /// Frames in the most recent `push`: the capture period the unpaired slack must cover.
    /// One Relaxed store on the capture callback; read only by the aligning read.
    last_push_frames: AtomicUsize,
    counters: MonitorCounters,
}

impl InputRing {
    /// Prepare up to half a second of storage, capped at 16 MiB of sample data. Channels
    /// follow CPAL's u16 domain (zero normalizes to one); an extreme format gets a shorter
    /// FIFO, not an unbounded allocation. Interpolation history adds two device frames.
    /// Capture and render are taken to run on separate callbacks
    /// ([`ALIGNED_BACKLOG_BLOCKS_UNPAIRED`]); see [`Self::with_pairing`].
    pub fn new(channels: usize, rate: u32) -> Arc<InputRing> {
        Self::with_pairing(channels, rate, false)
    }

    /// [`Self::new`], for a device whose one callback delivers the capture block and then asks
    /// for the render block when `paired` ([`ALIGNED_BACKLOG_BLOCKS_PAIRED`]).
    pub fn with_pairing(channels: usize, rate: u32, paired: bool) -> Arc<InputRing> {
        let ch = channels.clamp(1, usize::from(u16::MAX));
        let wanted = if rate == 0 { 24_000 } else { (rate as usize / 2).max(1) };
        let frames = wanted.min((1 << 22) / ch);
        let (producer, queue) = rtrb::RingBuffer::new(frames * ch);
        let (producer, reader) = (
            Mutex::new(producer),
            Mutex::new(MonitorReader {
                queue,
                left: vec![0.0; ch],
                right: vec![0.0; ch],
                left_valid: false,
                right_valid: false,
                phase: 0.0,
                removed_frames: 0,
            }),
        );
        // On macOS `std::sync::Mutex` is a lazily boxed `pthread_mutex_t`: the first lock or
        // `try_lock` allocates 64 bytes. Take each lock once here, on the control thread, so the
        // audio callbacks that only `try_lock` never allocate on first use.
        drop(producer.lock());
        drop(reader.lock());
        Arc::new(InputRing {
            producer,
            reader,
            channels: std::sync::atomic::AtomicUsize::new(ch),
            ch,
            fifo_frames: frames,
            accepted_frames: AtomicU64::new(0),
            retired_frames: AtomicU64::new(0),
            rate,
            overrun_pending: std::sync::atomic::AtomicBool::new(false),
            clear_pending: std::sync::atomic::AtomicBool::new(false),
            align_pending: std::sync::atomic::AtomicBool::new(false),
            last_push_frames: AtomicUsize::new(0),
            aligned_backlog_blocks: if paired { ALIGNED_BACKLOG_BLOCKS_PAIRED } else { ALIGNED_BACKLOG_BLOCKS_UNPAIRED },
            counters: MonitorCounters::default(),
        })
    }

    /// Each call supplies complete interleaved frames. A trailing partial frame is
    /// counted and discarded; the next call begins a new frame, not its continuation.
    pub fn push(&self, samples: &[f32]) {
        let frames = samples.len() / self.ch;
        self.last_push_frames.store(frames, Ordering::Relaxed);
        self.counters.incomplete.fetch_add((samples.len() % self.ch) as u64, Ordering::Relaxed);
        let mut producer = match self.producer.try_lock() {
            Ok(p) => p,
            Err(std::sync::TryLockError::WouldBlock) => {
                self.counters.input_contention.fetch_add(frames as u64, Ordering::Relaxed);
                return;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.counters.input_fault.fetch_add(frames as u64, Ordering::Relaxed);
                return;
            }
        };
        let accepted = frames.min(producer.slots() / self.ch);
        for &sample in samples.iter().take(accepted * self.ch) {
            // Capacity was reserved by this sole serialized producer; the consumer only frees it.
            let _ = producer.push(finite(sample));
        }
        self.accepted_frames.fetch_add(accepted as u64, Ordering::Release);
        let lost = frames - accepted;
        if lost > 0 {
            self.counters.overflow.fetch_add(lost as u64, Ordering::Relaxed);
            self.overrun_pending.store(true, Ordering::Release);
        }
    }

    /// Control thread: discard queued/history frames if the reader is idle, otherwise
    /// request a flush at the next output call (including input arriving before that flush).
    /// Never waits for the reader: its callback unlock must not wake a blocked control waiter.
    /// The next read that finds input then aligns ([`Self::align_on_next_read`]), so whatever
    /// queued between this clear and that read does not stay in front of the reader.
    pub fn clear_monitor(&self) {
        self.align_on_next_read();
        if let Ok(mut reader) = self.reader.try_lock() {
            self.clear_pending.swap(false, Ordering::AcqRel);
            let overrun = self.overrun_pending.swap(false, Ordering::AcqRel);
            self.counters.discarded.fetch_add(reader.clear() as u64, Ordering::Relaxed);
            self.counters.recoveries.fetch_add(u64::from(overrun), Ordering::Relaxed);
            self.retired_frames.store(reader.retired_input(), Ordering::Release);
        } else {
            self.clear_pending.store(true, Ordering::Release);
        }
    }

    /// Ask the next read that finds queued input to discard the oldest frames first, leaving
    /// that read plus the aligned backlog (K22). A plan calls this when it adopts the ring, and
    /// [`Self::clear_monitor`] calls it. One atomic store: safe on the audio thread.
    pub fn align_on_next_read(&self) {
        self.align_pending.store(true, Ordering::Release);
    }

    /// Fill `out` (planar, at `session_rate`) with up to `frames` frames, converting from the
    /// input rate. Missing frames are counted silence. No allocation or wait occurs here.
    pub fn pop_into(&self, out: &mut [Vec<f32>], frames: usize, session_rate: u32) {
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let usable = out.iter().take(self.ch).map(Vec::len).min().unwrap_or(0).min(frames);
        self.counters.short_output.fetch_add((frames - usable) as u64, Ordering::Relaxed);
        let frames = usable;
        let mut reader = match self.reader.try_lock() {
            Ok(r) => r,
            Err(std::sync::TryLockError::WouldBlock) => {
                self.counters.output_contention.fetch_add(frames as u64, Ordering::Relaxed);
                return;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.counters.output_fault.fetch_add(frames as u64, Ordering::Relaxed);
                return;
            }
        };
        let overrun = self.overrun_pending.swap(false, Ordering::AcqRel);
        let clear = self.clear_pending.swap(false, Ordering::AcqRel);
        if overrun || clear {
            self.counters.discarded.fetch_add(reader.clear() as u64, Ordering::Relaxed);
            self.counters.recoveries.fetch_add(u64::from(overrun), Ordering::Relaxed);
        }
        let step = if self.rate == 0 || session_rate == 0 { 1.0 } else { f64::from(self.rate) / f64::from(session_rate) };
        // Alignment waits for a read that has room and finds input: a read before the device's
        // input runs leaves it pending rather than aligning an empty ring.
        if frames > 0 && reader.queued_frames() > 0 && self.align_pending.swap(false, Ordering::AcqRel) {
            let read = (frames as f64 * step).ceil() as usize;
            let slack = self.aligned_backlog_blocks.saturating_mul(read.max(self.last_push_frames.load(Ordering::Relaxed)));
            let keep = read.saturating_add(slack);
            self.counters.discarded.fetch_add(reader.align(keep) as u64, Ordering::Relaxed);
        }
        let written = reader.render(out, frames, step);
        self.retired_frames.store(reader.retired_input(), Ordering::Release);
        self.counters.underrun.fetch_add((frames - written) as u64, Ordering::Relaxed);
    }

    /// Approximate buffered input frames, including pending interpolation history.
    /// Snapshots can straddle concurrent calls; this is a bounded diagnostic, not a
    /// calibrated latency measurement or a synchronization/control input.
    pub fn buffered_input_frames(&self) -> usize {
        let accepted = self.accepted_frames.load(Ordering::Acquire);
        let retired = self.retired_frames.load(Ordering::Acquire);
        accepted.saturating_sub(retired).min((self.fifo_frames + 2) as u64) as usize
    }

    /// FIFO capacity in input frames, excluding two interpolation-history frames.
    pub fn fifo_capacity_frames(&self) -> usize {
        self.fifo_frames
    }

    pub fn telemetry(&self) -> MonitorTelemetry {
        MonitorTelemetry {
            overflow_frames: self.counters.overflow.load(Ordering::Relaxed),
            underrun_frames: self.counters.underrun.load(Ordering::Relaxed),
            input_contention_frames: self.counters.input_contention.load(Ordering::Relaxed),
            output_contention_frames: self.counters.output_contention.load(Ordering::Relaxed),
            input_fault_frames: self.counters.input_fault.load(Ordering::Relaxed),
            output_fault_frames: self.counters.output_fault.load(Ordering::Relaxed),
            discarded_frames: self.counters.discarded.load(Ordering::Relaxed),
            overflow_recoveries: self.counters.recoveries.load(Ordering::Relaxed),
            incomplete_input_samples: self.counters.incomplete.load(Ordering::Relaxed),
            short_output_frames: self.counters.short_output.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod ring_tests {
    use super::InputRing;

    #[test]
    fn read_waits_for_a_whole_frame_when_publication_pauses_between_channels() {
        let ring = InputRing::new(3, 48_000);
        let mut producer = ring.producer.lock().unwrap();
        // Model a producer preempted between sample stores inside one complete-frame push.
        producer.push(10.0).unwrap();
        let mut output = vec![vec![9.0]; 3];
        ring.pop_into(&mut output, 1, 48_000);
        assert_eq!(output, vec![vec![0.0]; 3]);
        assert_eq!(ring.reader.lock().unwrap().queue.slots(), 1);
        producer.push(20.0).unwrap();
        producer.push(30.0).unwrap();
        ring.accepted_frames.fetch_add(1, super::Ordering::Release);
        drop(producer);
        ring.pop_into(&mut output, 1, 48_000);
        assert_eq!(output, vec![vec![10.0], vec![20.0], vec![30.0]]);
        assert_eq!(ring.buffered_input_frames(), 0);
        assert_eq!(ring.telemetry().underrun_frames, 1);
    }

    #[test]
    fn clear_preserves_a_partially_published_frame() {
        let ring = InputRing::new(3, 48_000);
        ring.push(&[1.0, 2.0, 3.0]);
        let mut producer = ring.producer.lock().unwrap();
        producer.push(10.0).unwrap();
        ring.clear_monitor();
        assert_eq!(ring.reader.lock().unwrap().queue.slots(), 1);
        assert_eq!(ring.telemetry().discarded_frames, 1);
        producer.push(20.0).unwrap();
        producer.push(30.0).unwrap();
        ring.accepted_frames.fetch_add(1, super::Ordering::Release);
        drop(producer);
        let mut output = vec![vec![0.0]; 3];
        ring.pop_into(&mut output, 1, 48_000);
        assert_eq!(output, vec![vec![10.0], vec![20.0], vec![30.0]]);
        assert_eq!(ring.buffered_input_frames(), 0);
    }

    #[test]
    fn input_and_output_roles_do_not_exclude_each_other() {
        let ring = InputRing::new(1, 48_000);
        let read_guard = ring.reader.lock().unwrap();
        ring.push(&[1.0, 2.0]);
        drop(read_guard);
        let write_guard = ring.producer.lock().unwrap();
        let mut output = vec![vec![0.0; 2]];
        ring.pop_into(&mut output, 2, 48_000);
        drop(write_guard);
        assert_eq!(output[0], [1.0, 2.0]);
        assert_eq!(ring.telemetry(), super::MonitorTelemetry::default());
    }

    #[test]
    fn same_role_contention_is_counted_and_never_waits() {
        let ring = InputRing::new(2, 48_000);
        let write_guard = ring.producer.lock().unwrap();
        ring.push(&[1.0; 6]);
        drop(write_guard);
        let read_guard = ring.reader.lock().unwrap();
        let mut output = vec![vec![1.0; 4]; 2];
        ring.pop_into(&mut output, 4, 48_000);
        drop(read_guard);
        assert!(output.iter().flatten().all(|&x| x == 0.0));
        assert_eq!(ring.telemetry().input_contention_frames, 3);
        assert_eq!(ring.telemetry().output_contention_frames, 4);
        assert_eq!(ring.telemetry().underrun_frames, 0);
    }

    #[test]
    fn clear_while_reading_defers_without_a_waiter_on_the_audio_lock() {
        let ring = InputRing::new(1, 48_000);
        ring.push(&[1.0, 2.0]);
        let read_guard = ring.reader.lock().unwrap();
        ring.clear_monitor();
        ring.push(&[3.0]);
        drop(read_guard);
        let mut output = vec![vec![9.0; 2]];
        ring.pop_into(&mut output, 2, 48_000);
        assert_eq!(output[0], [0.0, 0.0]);
        assert_eq!(ring.telemetry().discarded_frames, 3);
        assert_eq!(ring.telemetry().overflow_recoveries, 0);
    }

    #[test]
    fn poisoned_roles_fail_closed_and_are_distinct_from_contention() {
        let ring = InputRing::new(1, 48_000);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = ring.producer.lock().unwrap();
            panic!("injected producer fault");
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = ring.reader.lock().unwrap();
            panic!("injected reader fault");
        }));
        ring.push(&[1.0; 3]);
        let mut output = vec![vec![1.0; 4]];
        ring.pop_into(&mut output, 4, 48_000);
        assert_eq!(output[0], [0.0; 4]);
        let t = ring.telemetry();
        assert_eq!((t.input_fault_frames, t.output_fault_frames), (3, 4));
        assert_eq!((t.input_contention_frames, t.output_contention_frames), (0, 0));
    }

    /// A 1 kHz sine recorded at 16 kHz must come out as a 1 kHz sine at 48 kHz, across calls.
    #[test]
    fn monitor_converts_input_rate_to_session_rate() {
        let ring = InputRing::new(1, 16_000);
        let input: Vec<f32> = (0..4_000).map(|i| (2.0 * std::f64::consts::PI * 1_000.0 * f64::from(i) / 16_000.0).sin() as f32).collect();
        // Fed a block at a time, as the device does; callback size never trims the FIFO.
        ring.push(&input[..32]);
        let mut out = vec![vec![0.0f32; 48]];
        let mut k = 0usize;
        for i in 0..10 {
            ring.pop_into(&mut out, 48, 48_000);
            ring.push(&input[32 + 16 * i..48 + 16 * i]);
            for v in &out[0] {
                let want = (2.0 * std::f64::consts::PI * 1_000.0 * k as f64 / 48_000.0).sin() as f32;
                assert!((v - want).abs() < 0.05, "sample {k}: got {v}, want {want}");
                k += 1;
            }
        }
    }

    /// Input faster than the session (96 kHz into 48 kHz): every other frame is read, in order.
    #[test]
    fn monitor_decimates_when_input_is_faster() {
        let ring = InputRing::new(1, 96_000);
        let input: Vec<f32> = (0..400).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 100]];
        ring.pop_into(&mut out, 100, 48_000);
        for (k, v) in out[0].iter().enumerate() {
            assert_eq!(*v, (2 * k) as f32, "output {k}");
        }
    }

    /// Same rate: samples pass through unchanged.
    #[test]
    fn monitor_passes_through_at_equal_rates() {
        let ring = InputRing::new(2, 48_000);
        let input: Vec<f32> = (0..200).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 50], vec![0.0f32; 50]];
        ring.pop_into(&mut out, 50, 48_000);
        assert_eq!(out[0][0], 0.0);
        assert_eq!(out[1][0], 1.0);
        assert_eq!(out[0][49], 98.0);
    }

    /// A ring filled before anything read it (the ASIO input runs from startup) is emptied at
    /// hand-over: the monitor starts from the next block.
    #[test]
    fn clear_monitor_empties_the_ring() {
        let ring = InputRing::new(1, 48_000);
        ring.push(&vec![0.5; 48_000]);
        ring.clear_monitor();
        let mut out = vec![vec![1.0f32; 64]];
        ring.pop_into(&mut out, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.0), "nothing left to hear");
        assert_eq!(ring.reader.lock().unwrap().queue.slots(), 0);
    }

    /// Callback size is not a latency policy: a larger input block must survive smaller reads.
    #[test]
    fn pop_preserves_a_larger_input_block() {
        let ring = InputRing::new(1, 48_000);
        let input: Vec<f32> = (0..4 * 256).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 256]];
        ring.pop_into(&mut out, 256, 48_000);
        assert_eq!(out[0], input[..256]);
        ring.pop_into(&mut out, 256, 48_000);
        assert_eq!(out[0], input[256..512]);
        assert_eq!(ring.telemetry(), super::MonitorTelemetry::default());
    }

    /// One device cycle of a paired device: the capture block, then the plan's read of it.
    fn cycle(ring: &InputRing, block: &[f32]) -> Vec<f32> {
        ring.push(block);
        let mut out = vec![vec![0.0f32; block.len()]];
        ring.pop_into(&mut out, block.len(), 48_000);
        out.swap_remove(0)
    }

    /// K22: a ring that queued `N` frames before its first read (the plan adopted it late) aligns
    /// on that read. Paired, the read gets this cycle's capture and B is 0; unpaired, one read
    /// block of slack stays queued. Without a request the FIFO is never trimmed.
    #[test]
    fn the_aligning_read_drops_what_queued_before_it_down_to_the_target() {
        const N: usize = 448;
        let block: Vec<f32> = (N..N + 64).map(|i| i as f32).collect();
        let backlog: Vec<f32> = (0..N).map(|i| i as f32).collect();
        for (paired, kept) in [(true, 0), (false, 64)] {
            let ring = InputRing::with_pairing(1, 48_000, paired);
            ring.push(&backlog);
            ring.align_on_next_read();
            let mut out = vec![vec![0.0f32; 64]];
            ring.push(&block);
            ring.pop_into(&mut out, 64, 48_000);
            let first = (N - kept) as f32;
            assert_eq!(out[0], (0..64).map(|i| first + i as f32).collect::<Vec<_>>(), "paired {paired}");
            assert_eq!(ring.buffered_input_frames(), kept, "paired {paired}: B is the target");
            assert_eq!(ring.telemetry().discarded_frames, (N - kept) as u64);
            assert_eq!(ring.telemetry().underrun_frames, 0);
            // Lockstep afterwards keeps B at the target.
            for k in 0..10 {
                let next: Vec<f32> = (0..64).map(|i| (10_000 + 64 * k + i) as f32).collect();
                cycle(&ring, &next);
                assert_eq!(ring.buffered_input_frames(), kept);
            }
        }
        let ring = InputRing::with_pairing(1, 48_000, true);
        ring.push(&backlog);
        assert_eq!(cycle(&ring, &block)[0], 0.0, "no request, no trim: the FIFO order holds");
    }

    /// N1: an unpaired backend whose capture pushes 480 frames per 10 ms while the render reads
    /// 128 every 2.667 ms. The aligning read keeps `128 + max(128, 480)` frames, so the reader
    /// never runs dry in the first 10 s of simulated time. A slack of one read block left a gap
    /// of about 288 frames right after alignment.
    #[test]
    fn unpaired_slack_covers_a_capture_period_above_two_reads() {
        const RATE: u64 = 48_000;
        let ring = InputRing::new(1, RATE as u32);
        // Capture started 3 periods before the plan adopts the ring and reads.
        for _ in 0..3 {
            ring.push(&[1.0; 480]);
        }
        ring.align_on_next_read();
        // Time in frames; captures land at multiples of 480 (after the first read at t = 0).
        let mut next_capture = 480u64;
        let mut out = vec![vec![0.0f32; 128]];
        let mut t = 0u64;
        while t < 10 * RATE {
            while next_capture <= t {
                ring.push(&[1.0; 480]);
                next_capture += 480;
            }
            ring.pop_into(&mut out, 128, RATE as u32);
            assert_eq!(ring.telemetry().underrun_frames, 0, "underrun at frame {t}");
            t += 128;
        }
    }

    /// K22: a `clear_monitor` mid-session re-establishes the target, whether the reader was idle
    /// (immediate clear) or reading (deferred clear), and whatever queued between the clear and
    /// the next read.
    #[test]
    fn a_clear_mid_session_reestablishes_the_target() {
        for deferred in [false, true] {
            let ring = InputRing::with_pairing(1, 48_000, true);
            ring.align_on_next_read();
            for k in 0..5 {
                cycle(&ring, &[k as f32; 64]);
                assert_eq!(ring.buffered_input_frames(), 0);
            }
            // A stall: three blocks queue ahead of the reader and stay there.
            for _ in 0..3 {
                ring.push(&[-1.0; 64]);
            }
            cycle(&ring, &[7.0; 64]);
            assert_eq!(ring.buffered_input_frames(), 192);
            if deferred {
                let guard = ring.reader.lock().unwrap();
                ring.clear_monitor();
                drop(guard);
            } else {
                ring.clear_monitor();
            }
            // Input keeps arriving before the next read (the attach race of K22).
            for _ in 0..4 {
                ring.push(&[-2.0; 64]);
            }
            let out = cycle(&ring, &[9.0; 64]);
            if deferred {
                // The deferred clear also takes this cycle's block; the next cycle aligns.
                assert_eq!(out, [0.0; 64]);
                assert_eq!(cycle(&ring, &[11.0; 64]), [11.0; 64]);
            } else {
                assert_eq!(out, [9.0; 64], "the read gets this cycle's capture");
            }
            assert_eq!(ring.buffered_input_frames(), 0, "deferred {deferred}");
            assert_eq!(cycle(&ring, &[12.0; 64]), [12.0; 64]);
            assert_eq!(ring.buffered_input_frames(), 0);
        }
    }

    /// A read before the device's input runs finds nothing: the alignment waits for the first
    /// read that finds input instead of being spent on an empty ring.
    #[test]
    fn alignment_waits_for_a_read_that_finds_input() {
        let ring = InputRing::with_pairing(1, 48_000, true);
        ring.align_on_next_read();
        assert_eq!(cycle(&ring, &[]), Vec::<f32>::new());
        let mut out = vec![vec![1.0f32; 64]];
        ring.pop_into(&mut out, 64, 48_000);
        assert_eq!(out[0], [0.0; 64]);
        ring.push(&[-1.0; 256]);
        assert_eq!(cycle(&ring, &[3.0; 64]), [3.0; 64]);
        assert_eq!(ring.buffered_input_frames(), 0);
    }

    /// Nothing buffered: silence, not stale data.
    #[test]
    fn monitor_underrun_is_silence() {
        let ring = InputRing::new(1, 16_000);
        let mut out = vec![vec![1.0f32; 16]];
        ring.pop_into(&mut out, 16, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.0));
    }

    /// NaN and infinities from the device monitor as silence.
    #[test]
    fn monitor_silences_nan_and_infinite_input() {
        let ring = InputRing::new(1, 48_000);
        ring.push(&[0.5, f32::NAN, f32::INFINITY, -0.5, 0.0, 0.0, 0.0, 0.0]);
        let mut out = vec![vec![1.0f32; 4]];
        ring.pop_into(&mut out, 4, 48_000);
        assert_eq!(out[0], [0.5, 0.0, 0.0, -0.5]);
    }
}
