//! The driver's four callbacks (`ASIOCallbacks`) and the process-wide state they reach.
//!
//! **Why process-wide.** The ASIO callbacks are plain C functions without a user-data pointer, and
//! ASIO allows one loaded driver per process (the host's `Slot`), so one callback table
//! ([`table`]) and one installed [`CallbackState`] (behind the atomic pointer `CURRENT`) serve it.
//! A state is installed before `createBuffers`; a second install while one is in place fails.
//! The host never holds a buffer address across `disposeBuffers` (#524): a rebuild disposes with
//! the state installed but its addresses cleared and every claim on them waited out
//! ([`CallbackState::forget_planes`]); the end of a session uninstalls and drains the state first
//! ([`Installed::uninstall`]), so no callback is inside it, and only then disposes.
//!
//! **Which thread.** `bufferSwitch`/`bufferSwitchTimeInfo` run on a thread the driver owns (not the
//! STA host thread), `asioMessage` and `sampleRateDidChange` on whatever thread the driver uses,
//! sometimes the host thread from inside a call it made. Nothing here blocks, allocates or takes a
//! lock: the [`Engine`] and the buffer addresses sit in `UnsafeCell`s guarded by one `busy` flag
//! that the callbacks only ever *try* to take; the control side takes it, spinning for a bounded
//! time (a callback that never returns fails the request instead of hanging the host thread,
//! #524), only while the driver is stopped or before it starts (attach, restart). A buffer
//! switch that finds `busy` taken writes silence into its half (through atomic copies of the
//! output addresses) and is counted in `busy_skips` (#524). Every write to an output half holds
//! that half's bit in `claims`, so no two writes to one half overlap. Errors that must reach the
//! owner's error callbacks (a driver message, the watchdog, a panicking callback) set a pending
//! bit and are delivered under a flag of their own, `reporting`, from whichever thread gets it
//! next (never under `busy`, so a slow error callback cannot make a buffer switch skip); the
//! holder rechecks the bits after releasing, so none is lost.
//!
//! **Lifetime.** A callback counts itself in the process-wide `ENTERING` while it reads `CURRENT`,
//! and in the state's own `in_flight` count before it leaves `ENTERING`, so
//! [`Installed::uninstall`] can clear the pointer and wait (bounded) until no callback is inside
//! the state before freeing it (and the user callbacks with it): the state outlives every callback
//! that saw it. A callback that never returns holds only its own state's count, so it leaks that
//! state and nothing else: a later session drains at once (#524).
//!
//! Every `unsafe` block has a `// SAFETY:` comment; the logic is in the safe [`crate::engine`].
#![deny(clippy::disallowed_macros, clippy::disallowed_methods)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use crate::abi::{self, AsioBool, AsioCallbacks, AsioLong, AsioSampleRate, AsioTime};
use crate::com::{CallbackDriver, Event};
use crate::convert::{InputPlanes, OutputPlanes};
use crate::engine::{Engine, Errors, PENDING_RESET, PENDING_STALL, Time};
use crate::mmcss::Mmcss;
use crate::stats::DriverEvents;
use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use transit_audio_backend::HostOrigin;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// The installed state, or null.
static CURRENT: AtomicPtr<CallbackState> = AtomicPtr::new(std::ptr::null_mut());
/// Callbacks between their entry and the moment they counted themselves in the state they read
/// from `CURRENT` (a few atomic operations, nothing that blocks).
static ENTERING: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    /// How many of this crate's driver callbacks the calling thread is inside (a const-initialized
    /// `Cell`: no allocation and no destructor).
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Whether the calling thread is inside a driver callback (a buffer switch, `asioMessage`,
/// `sampleRateDidChange`): a control call made there must not wait for the host thread or call
/// the driver ([`crate::AsioStream`]'s `stop` defers to the host thread instead).
pub(crate) fn in_driver_callback() -> bool {
    DEPTH.with(Cell::get) > 0
}

/// The callback table handed to every `createBuffers`, in writable static memory: the SDK passes
/// it as `ASIOCallbacks *` and the driver keeps the pointer until `disposeBuffers`.
struct Table(UnsafeCell<AsioCallbacks>);

// SAFETY: Rust never reads or writes the table after initialization; only drivers read it (and
// the SDK lets them keep the pointer), through the raw pointer `table` returns.
unsafe impl Sync for Table {}

static TABLE: Table = Table(UnsafeCell::new(AsioCallbacks {
    bufferSwitch: Some(buffer_switch),
    sampleRateDidChange: Some(sample_rate_did_change),
    asioMessage: Some(asio_message),
    bufferSwitchTimeInfo: Some(buffer_switch_time_info),
}));

/// The `ASIOCallbacks *` for `createBuffers`; valid for the life of the process.
pub(crate) fn table() -> *mut AsioCallbacks {
    TABLE.0.get()
}

/// The QPC now, in 100 ns units; allocation-free (the `transit-audio-wasapi` reading).
pub(crate) fn qpc_100ns() -> u64 {
    let (mut ticks, mut frequency) = (0i64, 0i64);
    // SAFETY: QueryPerformanceCounter and QueryPerformanceFrequency fill live i64s and cannot
    // fail on Windows XP and later; a failure would leave zeros, which gives a zero time.
    unsafe {
        let _ = QueryPerformanceCounter(&mut ticks);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    (ticks.unsigned_abs() as u128 * 10_000_000 / frequency.unsigned_abs().max(1) as u128) as u64
}

/// The `(QPC in 100 ns units, host-origin nanoseconds)` pair of one instant (`origin_pair` of
/// `transit-audio-wasapi`): each stream takes its own at `start()` against the Player's origin.
pub(crate) fn origin_pair(origin: HostOrigin) -> (u64, i64) {
    let qpc = qpc_100ns();
    let instant = Instant::now();
    let origin_ns = match instant.checked_duration_since(origin.instant()) {
        Some(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        None => -i64::try_from(origin.instant().duration_since(instant).as_nanos()).unwrap_or(i64::MAX),
    };
    (qpc, origin_ns)
}

/// The two buffer addresses of each created channel and the byte length of one buffer.
#[derive(Debug, Default)]
pub(crate) struct PlaneTable {
    pub(crate) outputs: Vec<[usize; 2]>,
    pub(crate) inputs: Vec<[usize; 2]>,
    pub(crate) output_bytes: usize,
    pub(crate) input_bytes: usize,
}

impl PlaneTable {
    /// Zeroes both halves of every output buffer (drivers hand them over uninitialized).
    pub(crate) fn silence_outputs(&self) {
        for half in 0..2 {
            crate::convert::silence(&mut RawOutputs { table: &self.outputs, half, bytes: self.output_bytes });
        }
    }
}

/// The output buffers of one half, from the addresses `createBuffers` returned.
struct RawOutputs<'a> {
    table: &'a [[usize; 2]],
    half: usize,
    bytes: usize,
}

impl OutputPlanes for RawOutputs<'_> {
    fn count(&self) -> usize {
        self.table.len()
    }

    fn plane(&mut self, channel: usize) -> &mut [u8] {
        let Some(address) = self.table.get(channel).and_then(|a| a.get(self.half)).copied().filter(|a| *a != 0) else { return &mut [] };
        // SAFETY: `address` is one of the two buffers `createBuffers` returned for this output
        // channel, `bytes` long (frames times the sample width), valid until `disposeBuffers`
        // ("buffers are valid until ASIODisposeBuffers"; the host takes the addresses out of the
        // state before it calls that). It is reached only under the state's `busy` flag and with
        // this half's claim held (or on fresh buffers whose addresses are not yet published), so
        // no other Rust reference to it and no other write to it exists, and `&mut self` keeps
        // this one unique until it ends. During the buffer switch for this half the driver works
        // on the other half.
        unsafe { std::slice::from_raw_parts_mut(address as *mut u8, self.bytes) }
    }
}

/// The input buffers of one half.
struct RawInputs<'a> {
    table: &'a [[usize; 2]],
    half: usize,
    bytes: usize,
}

impl InputPlanes for RawInputs<'_> {
    fn count(&self) -> usize {
        self.table.len()
    }

    fn plane(&self, channel: usize) -> &[u8] {
        let Some(address) = self.table.get(channel).and_then(|a| a.get(self.half)).copied().filter(|a| *a != 0) else { return &[] };
        // SAFETY: as for `RawOutputs::plane`: a `createBuffers` input buffer of `bytes` bytes, valid
        // until `disposeBuffers`, read under `busy` while the driver fills the other half.
        unsafe { std::slice::from_raw_parts(address as *const u8, self.bytes) }
    }
}

/// Everything the callbacks of one driver session touch. Built on the host thread, installed for
/// the callbacks by [`Installed`].
pub(crate) struct CallbackState {
    busy: AtomicBool,
    engine: UnsafeCell<Engine>,
    planes: UnsafeCell<PlaneTable>,
    /// Guards `errors` as `busy` guards the engine, so that delivering an error (user code) never
    /// holds `busy` and makes a buffer switch skip (#524).
    reporting: AtomicBool,
    errors: UnsafeCell<Errors>,
    /// The halves (bit 0, bit 1) being written: every write to an output half holds its bit (#524).
    claims: AtomicU8,
    /// The output buffer addresses once more, as atomics: what a buffer switch that finds `busy`
    /// taken zeroes its half through ([`CallbackState::silence_half`]). Published and cleared with
    /// the plane table; one entry for each output channel of the driver.
    silence: Box<[[AtomicUsize; 2]]>,
    silence_bytes: AtomicUsize,
    driver: CallbackDriver,
    output_ready: AtomicBool,
    pending: AtomicU8,
    resync: AtomicBool,
    /// The session's rate, as `f64` bits, for `sampleRateDidChange`.
    rate_bits: AtomicU64,
    pub(crate) events: Arc<DriverEvents>,
    /// Set by `kAsioLatenciesChanged`; the host thread refetches `getLatencies`.
    pub(crate) latencies_dirty: AtomicBool,
    wake: Arc<Event>,
    /// The QPC (100 ns) of the last buffer switch's entry, 0 before the first since the driver
    /// started: what the host thread's watchdog reads.
    last_switch: AtomicU64,
    /// The buffer-switch thread's MMCSS registration, tended at each switch.
    mmcss: Arc<Mmcss>,
    /// Callbacks inside this state ([`Flight`]).
    in_flight: AtomicUsize,
}

// SAFETY: the engine and plane cells are only accessed under `busy`, the error cell only under
// `reporting` (each taken with a compare-exchange and given back with a store, both SeqCst), which
// makes the accesses to each cell mutually exclusive across threads; everything else is atomics,
// a boxed slice of atomics, `Arc`s of atomics, the `Sync` `Event` and the `Sync`
// `CallbackDriver`. The user callbacks are `Send`, so running them on whichever thread holds the
// flag is fine.
unsafe impl Sync for CallbackState {}

/// Both halves' bits in `claims`.
const BOTH_HALVES: u8 = 0b11;

impl CallbackState {
    /// The state of a session over a driver with `outputs` output channels.
    pub(crate) fn new(
        engine: Engine,
        driver: CallbackDriver,
        events: Arc<DriverEvents>,
        wake: Arc<Event>,
        mmcss: Arc<Mmcss>,
        outputs: usize,
    ) -> Box<Self> {
        let rate = f64::from(engine.rate());
        Box::new(Self {
            busy: AtomicBool::new(false),
            engine: UnsafeCell::new(engine),
            planes: UnsafeCell::new(PlaneTable::default()),
            reporting: AtomicBool::new(false),
            errors: UnsafeCell::new(Errors::default()),
            claims: AtomicU8::new(0),
            silence: (0..outputs).map(|_| [AtomicUsize::new(0), AtomicUsize::new(0)]).collect(),
            silence_bytes: AtomicUsize::new(0),
            driver,
            output_ready: AtomicBool::new(false),
            pending: AtomicU8::new(0),
            resync: AtomicBool::new(false),
            rate_bits: AtomicU64::new(rate.to_bits()),
            events,
            latencies_dirty: AtomicBool::new(false),
            wake,
            last_switch: AtomicU64::new(0),
            mmcss,
            in_flight: AtomicUsize::new(0),
        })
    }

    fn try_take(&self) -> bool {
        self.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed).is_ok()
    }

    fn give_back(&self) {
        self.busy.store(false, Ordering::SeqCst);
    }

    fn try_report(&self) -> bool {
        self.reporting.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed).is_ok()
    }

    fn end_report(&self) {
        self.reporting.store(false, Ordering::SeqCst);
    }

    /// Runs `f` on the engine and the buffer addresses on the control side. Spins (yielding) for
    /// the flag up to `limit`, `None` past it (#524: a callback that never returns holds it); call
    /// it only when no buffer switch should run (before `start`, after `stop`), where it is free at
    /// once.
    pub(crate) fn with<R>(&self, limit: Duration, f: impl FnOnce(&mut Engine, &mut PlaneTable) -> R) -> Option<R> {
        let deadline = Instant::now() + limit;
        while !self.try_take() {
            if Instant::now() > deadline {
                return None;
            }
            std::thread::yield_now();
        }
        // SAFETY: `busy` is held: no other access to the cells exists until `give_back`.
        let (engine, planes) = unsafe { (&mut *self.engine.get(), &mut *self.planes.get()) };
        let r = f(engine, planes);
        self.give_back();
        Some(r)
    }

    /// Runs `f` on the error callbacks on the control side, waiting up to `limit` for an error
    /// being delivered (user code) to end: `None` past it.
    pub(crate) fn with_errors<R>(&self, limit: Duration, f: impl FnOnce(&mut Errors) -> R) -> Option<R> {
        let deadline = Instant::now() + limit;
        while !self.try_report() {
            if Instant::now() > deadline {
                return None;
            }
            std::thread::yield_now();
        }
        // SAFETY: `reporting` is held: no other access to the error cell exists until `end_report`.
        let r = f(unsafe { &mut *self.errors.get() });
        self.end_report();
        self.deliver_pending();
        Some(r)
    }

    pub(crate) fn set_output_ready(&self, supported: bool) {
        self.output_ready.store(supported, Ordering::Relaxed);
    }

    /// Makes `table` the buffers of the buffer switches (control side, before the driver starts)
    /// and publishes its output addresses for a skipped switch's silence. The buffers must already
    /// be silent: nothing holds a claim on buffers whose addresses were not published. False when
    /// the engine stayed busy for `limit`: nothing was published, so the buffers may be disposed.
    #[must_use]
    pub(crate) fn install_planes(&self, table: PlaneTable, limit: Duration) -> bool {
        self.with(limit, |_, planes| {
            // The length before the addresses: a switch that reads a new address reads its length.
            self.silence_bytes.store(table.output_bytes, Ordering::SeqCst);
            for (channel, slot) in self.silence.iter().enumerate() {
                let addresses = table.outputs.get(channel).copied().unwrap_or([0, 0]);
                for (half, address) in slot.iter().zip(addresses) {
                    half.store(address, Ordering::SeqCst);
                }
            }
            *planes = table;
        })
        .is_some()
    }

    /// Takes the buffer addresses out of the state, so a buffer switch finds no buffer to touch,
    /// then waits up to `limit` until no switch holds a claim (one that read an address before it
    /// went may still be zeroing that half). Call it before `disposeBuffers`: the host never holds a
    /// buffer address across it (#524). False when the engine stayed busy or a claim was still
    /// held at the limit: the buffers must not be disposed.
    #[must_use]
    pub(crate) fn forget_planes(&self, limit: Duration) -> bool {
        self.set_output_ready(false);
        let cleared = self.with(limit, |_, planes| {
            *planes = PlaneTable::default();
            for half in self.silence.iter().flatten() {
                half.store(0, Ordering::SeqCst);
            }
            self.silence_bytes.store(0, Ordering::SeqCst);
        });
        if cleared.is_none() {
            return false;
        }
        // A skipped switch sets its claim, then reads an address (both SeqCst); the clear above
        // stored the zeros, then this reads the claims. So either that switch read a zero, or its
        // claim is seen here until it has finished zeroing.
        let deadline = Instant::now() + limit;
        while self.claims.load(Ordering::SeqCst) != 0 {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::yield_now();
        }
        true
    }

    /// Zeroes both halves of every output buffer before a fresh driver start, holding both
    /// halves' claims (waited for up to `limit`: false past it, nothing written).
    #[must_use]
    pub(crate) fn silence_outputs(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            let before = self.claims.fetch_or(BOTH_HALVES, Ordering::SeqCst);
            if before == 0 {
                break;
            }
            // Give back the bits only this call set; another writer holds the rest.
            self.claims.fetch_and(!(BOTH_HALVES & !before), Ordering::SeqCst);
            if Instant::now() > deadline {
                return false;
            }
            std::thread::yield_now();
        }
        let silenced = self.with(limit, |_, planes| planes.silence_outputs());
        self.claims.fetch_and(!BOTH_HALVES, Ordering::SeqCst);
        silenced.is_some()
    }

    /// One buffer switch on the driver's thread.
    fn switch(&self, index: AsioLong, time: Time, arrival_100ns: u64) {
        self.last_switch.store(arrival_100ns, Ordering::Relaxed);
        self.mmcss.tend();
        let half = usize::from(index & 1 == 1);
        let claim = 1u8 << half;
        if self.claims.fetch_or(claim, Ordering::SeqCst) & claim != 0 {
            // Another writer has this very half (a driver that asked twice for it, or the control
            // side's silence before a start): it is that writer's to fill.
            self.events.busy_skips.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if !self.try_take() {
            // The engine is busy (an earlier buffer switch still runs, or the control side holds
            // it): this half plays silence, not the block it held from two switches ago (#524).
            self.silence_half(half);
            self.claims.fetch_and(!claim, Ordering::SeqCst);
            self.events.busy_skips.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // SAFETY: `busy` is held (taken above): the only access to the cells until `give_back`.
        let (engine, planes) = unsafe { (&mut *self.engine.get(), &*self.planes.get()) };
        if self.resync.swap(false, Ordering::Relaxed) {
            engine.resync();
        }
        let mut outputs = RawOutputs { table: &planes.outputs, half, bytes: planes.output_bytes };
        let inputs = RawInputs { table: &planes.inputs, half, bytes: planes.input_bytes };
        let faults = engine.switch(time, arrival_100ns, &mut outputs, &inputs);
        self.claims.fetch_and(!claim, Ordering::SeqCst);
        if self.output_ready.load(Ordering::Relaxed) {
            self.driver.output_ready();
        }
        engine.finish(arrival_100ns, qpc_100ns());
        self.give_back();
        if faults != 0 {
            self.pending.fetch_or(faults, Ordering::SeqCst);
        }
        self.deliver_pending();
    }

    /// Zeroes output half `half` of every published output buffer: a buffer switch that holds
    /// that half's claim but not `busy`.
    fn silence_half(&self, half: usize) {
        for slot in self.silence.iter() {
            let address = slot.get(half).map_or(0, |a| a.load(Ordering::SeqCst));
            let bytes = self.silence_bytes.load(Ordering::SeqCst);
            if address != 0 && bytes != 0 {
                // SAFETY: `address` is an output buffer `createBuffers` returned, `bytes` long, and
                // published by `install_planes`. It is alive: before a rebuild's `disposeBuffers`,
                // `forget_planes` clears it and then waits until no claim is held (the SeqCst
                // argument there), and this switch holds this half's claim; before the end of a
                // session's `disposeBuffers`, `Installed::uninstall` waits until no callback is
                // inside the state (this one is, counted in its `in_flight`). Every write to this half
                // holds that claim, so no other write to it runs now; the driver works on the
                // other half during this half's switch. No Rust reference to it is made.
                unsafe { std::ptr::write_bytes(address as *mut u8, 0, bytes) };
            }
        }
    }

    /// The QPC of the last buffer switch since the driver started, 0 before the first.
    pub(crate) fn last_switch(&self) -> u64 {
        self.last_switch.load(Ordering::Relaxed)
    }

    /// The driver (re)starts: the watchdog waits for its first buffer switch again.
    pub(crate) fn forget_last_switch(&self) {
        self.last_switch.store(0, Ordering::Relaxed);
    }

    /// The watchdog found the driver had stopped calling back (host thread): counted, and each
    /// open stream gets `StreamError::Stalled` once, delivered at once unless another error is
    /// being delivered (then by that thread when it is done).
    pub(crate) fn stalled(&self) {
        self.events.timeouts.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_or(PENDING_STALL, Ordering::SeqCst);
        self.deliver_pending();
    }

    /// Delivers pending errors if no other thread is delivering; that one rechecks the bits after
    /// it lets go, so none is lost. Never touches `busy`.
    fn deliver_pending(&self) {
        while self.pending.load(Ordering::SeqCst) != 0 {
            if !self.try_report() {
                return;
            }
            // SAFETY: `reporting` is held (taken above): the only access to the error cell until
            // `end_report`.
            let errors = unsafe { &mut *self.errors.get() };
            errors.deliver(self.pending.swap(0, Ordering::SeqCst));
            self.end_report();
        }
    }

    fn reset_requested(&self) {
        self.events.resets.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_or(PENDING_RESET, Ordering::SeqCst);
        self.deliver_pending();
    }

    fn message(&self, selector: AsioLong) -> AsioLong {
        match selector {
            abi::K_ASIO_RESET_REQUEST => {
                self.reset_requested();
                1
            }
            abi::K_ASIO_RESYNC_REQUEST => {
                self.events.resets.fetch_add(1, Ordering::Relaxed);
                self.resync.store(true, Ordering::Relaxed);
                1
            }
            abi::K_ASIO_LATENCIES_CHANGED => {
                self.latencies_dirty.store(true, Ordering::Relaxed);
                self.wake.set();
                1
            }
            abi::K_ASIO_OVERLOAD => {
                self.events.overloads.fetch_add(1, Ordering::Relaxed);
                0
            }
            _ => 0,
        }
    }

    fn rate_changed(&self, rate: f64) {
        let ours = f64::from_bits(self.rate_bits.load(Ordering::Relaxed));
        if (rate - ours).abs() > 0.5 {
            self.reset_requested();
        }
    }
}

/// An installed [`CallbackState`]. [`Installed::uninstall`] (or the drop) uninstalls the state,
/// waits up to the drain limit for callbacks still inside it and frees it (with the user
/// callbacks). Uninstall it after `stop` and before `disposeBuffers` (#524).
pub(crate) struct Installed {
    /// Null once uninstalled.
    state: *mut CallbackState,
    drain: Duration,
}

impl Installed {
    /// Makes `state` the one the callbacks reach, to be drained within `drain` when it goes. Fails,
    /// giving it back, while another is installed.
    pub(crate) fn install(state: Box<CallbackState>, drain: Duration) -> Result<Self, Box<CallbackState>> {
        let raw = Box::into_raw(state);
        match CURRENT.compare_exchange(std::ptr::null_mut(), raw, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => Ok(Self { state: raw, drain }),
            // SAFETY: `raw` came from `Box::into_raw` just above and was not published.
            Err(_) => Err(unsafe { Box::from_raw(raw) }),
        }
    }

    pub(crate) fn state(&self) -> &CallbackState {
        debug_assert!(!self.state.is_null(), "the callback state is used after its uninstall");
        // SAFETY: the box is freed (or leaked) only by `uninstall`, which consumes `self`, or by
        // the drop, after which `self` is gone.
        unsafe { &*self.state }
    }

    /// Uninstalls the state and waits up to the drain limit until no callback is inside it, then
    /// frees it: true. False when a callback did not leave in time: the state is leaked (never
    /// freed under it), and so must be everything that callback can still reach, the buffers and
    /// the driver (#524).
    #[must_use]
    pub(crate) fn uninstall(mut self) -> bool {
        self.finish()
    }

    fn finish(&mut self) -> bool {
        let state = std::mem::replace(&mut self.state, std::ptr::null_mut());
        if state.is_null() {
            return true;
        }
        let _ = CURRENT.compare_exchange(state, std::ptr::null_mut(), Ordering::SeqCst, Ordering::SeqCst);
        // A callback that read the pointer before the swap counted itself in `ENTERING` first and
        // in the state's `in_flight` before it left `ENTERING` (SeqCst throughout). So once
        // `ENTERING` was seen at zero after the swap, and then `in_flight` at zero, no callback is
        // inside the state and none can still get in.
        let deadline = Instant::now() + self.drain;
        // SAFETY: `state` is the live box `install` published; it is freed only below.
        let inside = || unsafe { &(*state).in_flight }.load(Ordering::SeqCst);
        while ENTERING.load(Ordering::SeqCst) != 0 || inside() != 0 {
            if Instant::now() > deadline {
                log::warn!(
                    "ASIO: a driver callback did not return within {} ms; its state is leaked, and its buffers and driver with it",
                    self.drain.as_millis()
                );
                return false;
            }
            std::thread::yield_now();
        }
        // SAFETY: `state` came from `Box::into_raw` in `install`, is no longer reachable through
        // `CURRENT` and no callback is inside it (above); it is freed once, here.
        drop(unsafe { Box::from_raw(state) });
        true
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// A callback's stay inside the installed state: counted in `ENTERING` while it reads `CURRENT`,
/// then in that state's `in_flight` until it ends.
struct Flight(*const CallbackState);

impl Flight {
    fn enter() -> Self {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        ENTERING.fetch_add(1, Ordering::SeqCst);
        let state = CURRENT.load(Ordering::SeqCst);
        if !state.is_null() {
            // SAFETY: a non-null `CURRENT` points to a live installed state, and `ENTERING` counts
            // this callback: `Installed::uninstall` clears `CURRENT`, then waits until `ENTERING`
            // and the state's `in_flight` are zero before freeing, so the state is alive here.
            unsafe { &(*state).in_flight }.fetch_add(1, Ordering::SeqCst);
        }
        ENTERING.fetch_sub(1, Ordering::SeqCst);
        Flight(state)
    }

    fn state(&self) -> Option<&CallbackState> {
        // SAFETY: a non-null pointer is a state this `Flight` counted itself in (`enter`), which
        // `Installed::uninstall` does not free while that count is above zero; the borrow is tied
        // to this `Flight`, which drops the count only when it ends.
        (!self.0.is_null()).then(|| unsafe { &*self.0 })
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: as in `state`: the state lives while this `Flight` is counted in it, and this
            // is the last use of the pointer.
            unsafe { &(*self.0).in_flight }.fetch_sub(1, Ordering::SeqCst);
        }
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// `bufferSwitch(index, directProcess)`: drivers that do not use time info; the position comes
/// from `getSamplePosition`.
unsafe extern "C" fn buffer_switch(index: AsioLong, _direct_process: AsioBool) {
    let arrival = qpc_100ns();
    let flight = Flight::enter();
    let Some(state) = flight.state() else { return };
    let time = state
        .driver
        .sample_position()
        .map_or_else(Time::default, |(position, system)| Time { position: Some(position), system_time_ns: (system != 0).then_some(system) });
    state.switch(index, time, arrival);
}

/// `bufferSwitchTimeInfo(params, index, directProcess)`: the time info carries the position.
unsafe extern "C" fn buffer_switch_time_info(params: *mut AsioTime, index: AsioLong, _direct_process: AsioBool) -> *mut AsioTime {
    let arrival = qpc_100ns();
    let flight = Flight::enter();
    let Some(state) = flight.state() else { return params };
    let time = if params.is_null() {
        Time::default()
    } else {
        // SAFETY: the driver passes an `ASIOTime` (148 bytes, layout asserted in `abi`) that is valid
        // for the duration of the call; it is 4-byte packed, so the time info is copied out with
        // an unaligned read and no reference into it is made.
        let info = unsafe { std::ptr::addr_of!((*params).timeInfo).read_unaligned() };
        Time {
            position: (info.flags & abi::K_SAMPLE_POSITION_VALID != 0).then(|| info.samplePosition.value()),
            system_time_ns: (info.flags & abi::K_SYSTEM_TIME_VALID != 0).then(|| info.systemTime.value()),
        }
    };
    state.switch(index, time, arrival);
    params
}

/// `sampleRateDidChange(rate)`: a change under a running stream is a reset of the stream.
unsafe extern "C" fn sample_rate_did_change(rate: AsioSampleRate) {
    let flight = Flight::enter();
    if let Some(state) = flight.state() {
        state.rate_changed(rate);
    }
}

/// `asioMessage(selector, value, message, opt)`.
unsafe extern "C" fn asio_message(selector: AsioLong, value: AsioLong, _message: *mut c_void, _opt: *mut f64) -> AsioLong {
    match selector {
        abi::K_ASIO_SELECTOR_SUPPORTED => AsioLong::from(matches!(
            value,
            abi::K_ASIO_ENGINE_VERSION
                | abi::K_ASIO_RESET_REQUEST
                | abi::K_ASIO_RESYNC_REQUEST
                | abi::K_ASIO_LATENCIES_CHANGED
                | abi::K_ASIO_SUPPORTS_TIME_INFO
                | abi::K_ASIO_SUPPORTS_TIME_CODE
                | abi::K_ASIO_OVERLOAD
        )),
        abi::K_ASIO_ENGINE_VERSION => 2,
        abi::K_ASIO_SUPPORTS_TIME_INFO => abi::ASIO_TRUE,
        // No time code; a buffer size change is not handled in place ("use kAsioResetRequest").
        abi::K_ASIO_SUPPORTS_TIME_CODE | abi::K_ASIO_BUFFER_SIZE_CHANGE => 0,
        other => {
            let flight = Flight::enter();
            match flight.state() {
                Some(state) => state.message(other),
                // Before a session or after it: acknowledge what needs no state.
                None => AsioLong::from(matches!(other, abi::K_ASIO_RESET_REQUEST | abi::K_ASIO_RESYNC_REQUEST | abi::K_ASIO_LATENCIES_CHANGED)),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Calls into the table the way a driver does, from the test thread.
    use super::*;

    fn callbacks() -> AsioCallbacks {
        // SAFETY: the table is never written after initialization; a copy of its `Copy` value.
        unsafe { table().read() }
    }

    /// `bufferSwitchTimeInfo` with a time info of `position` and `system_ns` (each marked valid when
    /// `Some`).
    pub(crate) fn switch_time_info(index: i32, position: Option<u64>, system_ns: Option<u64>) {
        let mut time: AsioTime = AsioTime {
            reserved: [0; 4],
            timeInfo: abi::AsioTimeInfo {
                speed: 1.0,
                systemTime: abi::AsioTimeStamp::from_value(system_ns.unwrap_or(0)),
                samplePosition: abi::AsioSamples::from_value(position.unwrap_or(0)),
                sampleRate: 48_000.0,
                flags: if position.is_some() { abi::K_SAMPLE_POSITION_VALID } else { 0 }
                    | if system_ns.is_some() { abi::K_SYSTEM_TIME_VALID } else { 0 },
                reserved: [0; 12],
            },
            timeCode: abi::AsioTimeCode { speed: 0.0, timeCodeSamples: abi::AsioSamples::default(), flags: 0, future: [0; 64] },
        };
        let f = callbacks().bufferSwitchTimeInfo.expect("bufferSwitchTimeInfo");
        // SAFETY: a valid `ASIOTime` for the duration of the call, as a driver passes it.
        let back = unsafe { f(&mut time, index, abi::ASIO_TRUE) };
        assert_eq!(back, std::ptr::addr_of_mut!(time));
    }

    /// `bufferSwitch` (the driver then answers `getSamplePosition`).
    pub(crate) fn switch_plain(index: i32) {
        let f = callbacks().bufferSwitch.expect("bufferSwitch");
        // SAFETY: plain values, as a driver calls it.
        unsafe { f(index, abi::ASIO_TRUE) }
    }

    pub(crate) fn message(selector: i32, value: i32) -> i32 {
        let f = callbacks().asioMessage.expect("asioMessage");
        // SAFETY: null message and opt pointers, which these selectors do not read.
        unsafe { f(selector, value, std::ptr::null_mut(), std::ptr::null_mut()) }
    }

    pub(crate) fn rate_changed(rate: f64) {
        let f = callbacks().sampleRateDidChange.expect("sampleRateDidChange");
        // SAFETY: a plain value.
        unsafe { f(rate) }
    }

    /// Whether a state is installed.
    pub(crate) fn installed() -> bool {
        !CURRENT.load(Ordering::SeqCst).is_null()
    }
}
