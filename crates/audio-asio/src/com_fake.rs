use super::*;
use crate::driver::SampleType;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

/// A latch a fake `init` waits on.
#[derive(Default)]
pub(crate) struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub(crate) fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
    /// Closes the latch again: the next `init` waits until it is opened.
    pub(crate) fn close(&self) {
        *self.open.lock().unwrap() = false;
    }
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
}

/// What a fake driver reports.
#[derive(Clone)]
pub(crate) struct FakeSpec {
    pub driver_name: String,
    pub inputs: i32,
    pub outputs: i32,
    pub input_type: i32,
    pub output_type: i32,
    /// (min, max, preferred, granularity).
    pub buffer: (i32, i32, i32, i32),
    pub rates: Vec<f64>,
    pub current_rate: f64,
    pub latencies: (i32, i32),
    pub init_ok: bool,
    /// The raw value `init` returns, overriding `init_ok` (a C++ `bool` driver's garbage).
    pub init_raw: Option<i32>,
    pub init_message: String,
    pub init_gate: Option<Arc<Gate>>,
    /// `init` sends `WM_NULL` to `sysRef` from a thread of its own and waits this long for it.
    pub init_sends: Option<Duration>,
    pub channels_error: Option<i32>,
    pub panel_error: Option<i32>,
    pub create_error: Option<i32>,
    pub start_error: Option<i32>,
    pub output_ready: bool,
    /// The object's Nth `createBuffers` call (counted from 1) and every later one fail with
    /// `ASE_NoMemory`.
    pub create_fails_from: Option<usize>,
    /// `disposeBuffers` fills every buffer with [`POISON`] and keeps it allocated
    /// ([`Observed::disposed`]) instead of freeing it, so a write through a stale address shows.
    pub poison_on_dispose: bool,
    /// `disposeBuffers` (after the poison) delivers one buffer switch per half through the callback
    /// table `createBuffers` was given: a driver whose callback thread races its own dispose.
    pub switch_in_dispose: bool,
}

/// The byte a poisoning `disposeBuffers` fills the buffers with.
pub(crate) const POISON: u8 = 0xDD;

impl FakeSpec {
    pub(crate) fn new(name: &str, inputs: i32, outputs: i32, sample_type: i32) -> Self {
        Self {
            driver_name: name.into(),
            inputs,
            outputs,
            input_type: sample_type,
            output_type: sample_type,
            buffer: (32, 2048, 256, -1),
            rates: vec![44_100.0, 48_000.0],
            current_rate: 48_000.0,
            latencies: (300, 400),
            init_ok: true,
            init_raw: None,
            init_message: String::new(),
            init_gate: None,
            init_sends: None,
            channels_error: None,
            panel_error: None,
            create_error: None,
            start_error: None,
            output_ready: true,
            create_fails_from: None,
            poison_on_dispose: false,
            switch_in_dispose: false,
        }
    }
}

/// One channel's two buffers as `createBuffers` handed them out (addresses of leaked boxes).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FakeBuffer {
    pub input: bool,
    pub channel: i32,
    pub addresses: [usize; 2],
    pub bytes: usize,
}

/// What tests observe across all fake objects of one loader.
#[derive(Default)]
pub(crate) struct Observed {
    pub live: AtomicUsize,
    pub max_live: AtomicUsize,
    pub created: AtomicUsize,
    pub released: AtomicUsize,
    /// The thread of each object's last `Release` (#363: always the host thread).
    pub releases: Mutex<Vec<ThreadId>>,
    pub panels: AtomicUsize,
    pub threads: Mutex<Vec<ThreadId>>,
    pub sys_refs: Mutex<Vec<usize>>,
    /// The streaming calls, in order, with their thread.
    pub calls: Mutex<Vec<(&'static str, ThreadId)>>,
    pub buffers: Mutex<Vec<FakeBuffer>>,
    pub output_ready: AtomicUsize,
    /// What `getSamplePosition` answers (`u64::MAX`: `ASE_SPNotAdvancing`) and its system time.
    pub position: AtomicU64,
    pub system_time: AtomicU64,
    /// Set when the `init_sends` thread's `SendMessage` returned.
    pub sent: Arc<AtomicBool>,
    /// Frames `getLatencies` adds to both of the spec's figures (a driver whose latency moved).
    pub extra_latency: AtomicU64,
    /// The buffers a poisoning `disposeBuffers` kept (freed when this goes).
    pub disposed: Mutex<Vec<FakeBuffer>>,
}

impl Drop for Observed {
    fn drop(&mut self) {
        for b in self.disposed.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).drain(..) {
            free(b);
        }
    }
}

/// Frees both buffers of `b`.
fn free(b: FakeBuffer) {
    for address in b.addresses {
        // SAFETY: each came from `Box::into_raw` of a `bytes`-long boxed slice in `create_buffers`
        // and is freed once: by `dispose_buffers`, or for a poisoned one when `Observed` goes.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(address as *mut u8, b.bytes)) });
    }
}

impl Observed {
    /// How many channels a poisoning `disposeBuffers` kept, and whether every byte of them still
    /// holds [`POISON`] (nothing wrote through a disposed buffer's address).
    pub(crate) fn poison(&self) -> (usize, bool) {
        let disposed = self.disposed.lock().unwrap();
        let intact = disposed.iter().all(|b| {
            b.addresses.iter().all(|a| {
                // SAFETY: a kept buffer of `b.bytes` bytes, freed only when `Observed` goes.
                unsafe { std::slice::from_raw_parts(*a as *const u8, b.bytes) }.iter().all(|x| *x == POISON)
            })
        });
        (disposed.len(), intact)
    }

    fn call(&self, name: &'static str) {
        self.calls.lock().unwrap().push((name, std::thread::current().id()));
    }

    /// The names of the streaming calls so far.
    pub(crate) fn call_names(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().iter().map(|(n, _)| *n).collect()
    }

    fn buffer(&self, input: bool, channel: i32) -> FakeBuffer {
        *self.buffers.lock().unwrap().iter().find(|b| b.input == input && b.channel == channel).expect("a created buffer")
    }

    /// Writes `bytes` into input `channel`'s buffer `half`, as the hardware would.
    pub(crate) fn write_input(&self, channel: i32, half: usize, bytes: &[u8]) {
        let b = self.buffer(true, channel);
        assert_eq!(bytes.len(), b.bytes);
        // SAFETY: a live buffer of `b.bytes` bytes from `create_buffers` (freed only by
        // `dispose_buffers`); the test thread is the only one touching it now.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.addresses[half] as *mut u8, b.bytes) };
    }

    /// The created input channels and their buffer length.
    pub(crate) fn inputs(&self) -> Vec<(i32, usize)> {
        self.buffers.lock().unwrap().iter().filter(|b| b.input).map(|b| (b.channel, b.bytes)).collect()
    }

    /// Output `channel`'s buffer `half`, as the hardware would play it.
    pub(crate) fn read_output(&self, channel: i32, half: usize) -> Vec<u8> {
        let b = self.buffer(false, channel);
        // SAFETY: as in `write_input`.
        unsafe { std::slice::from_raw_parts(b.addresses[half] as *const u8, b.bytes) }.to_vec()
    }
}

#[repr(C)]
struct FakeObject {
    vtbl: *const IAsioVtbl,
    refs: AtomicU32,
    rate: Mutex<f64>,
    spec: FakeSpec,
    observed: Arc<Observed>,
    /// `createBuffers` calls so far.
    creates: AtomicUsize,
    /// The `ASIOCallbacks *` the last `createBuffers` was given (0 before).
    callbacks: AtomicUsize,
}

/// The fake object behind `this`, recording the calling thread (a control call).
///
/// # Safety
///
/// `this` came from [`create`] and has not been released for the last time.
unsafe fn object<'a>(this: *mut IAsio) -> &'a FakeObject {
    // SAFETY: the caller's contract.
    let obj = unsafe { object_quiet(this) };
    obj.observed.threads.lock().unwrap().push(std::thread::current().id());
    obj
}

/// The fake object behind `this`, for the calls the callback makes on the driver's thread.
///
/// # Safety
///
/// As for [`object`].
unsafe fn object_quiet<'a>(this: *mut IAsio) -> &'a FakeObject {
    // SAFETY: the caller's contract: `this` is a live `Box<FakeObject>` whose first field is
    // the vtable pointer, so the `IAsio` pointer is the object pointer.
    unsafe { &*this.cast::<FakeObject>() }
}

unsafe extern "C" fn query_interface(_this: *mut IAsio, _riid: *const [u8; 16], object: *mut *mut c_void) -> i32 {
    // SAFETY: COM contract: `object` is a valid out-pointer.
    unsafe { *object = std::ptr::null_mut() };
    windows::Win32::Foundation::E_NOINTERFACE.0
}
unsafe extern "C" fn add_ref(this: *mut IAsio) -> u32 {
    // SAFETY: called through a live object's vtable.
    unsafe { object(this) }.refs.fetch_add(1, Ordering::AcqRel) + 1
}
unsafe extern "C" fn release(this: *mut IAsio) -> u32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    let left = obj.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        obj.observed.live.fetch_sub(1, Ordering::AcqRel);
        obj.observed.released.fetch_add(1, Ordering::AcqRel);
        obj.observed.releases.lock().unwrap().push(std::thread::current().id());
        // SAFETY: the last reference: the box came from `Box::into_raw` in `create` and is
        // freed once; `obj` is not used after this.
        drop(unsafe { Box::from_raw(this.cast::<FakeObject>()) });
    }
    left
}
unsafe extern "C" fn init(this: *mut IAsio, sys: *mut c_void) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    obj.observed.sys_refs.lock().unwrap().push(sys as usize);
    if let Some(gate) = &obj.spec.init_gate {
        gate.wait();
    }
    if let Some(wait) = obj.spec.init_sends {
        // A driver whose worker thread `SendMessage`s to `sysRef` while `init` waits for it.
        let (window, sent) = (sys as usize, Arc::clone(&obj.observed.sent));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            send_null(window);
            sent.store(true, Ordering::SeqCst);
            let _ = done_tx.send(());
        });
        let _ = done_rx.recv_timeout(wait);
    }
    obj.spec.init_raw.unwrap_or(i32::from(obj.spec.init_ok))
}
unsafe extern "C" fn get_driver_name(this: *mut IAsio, name: *mut u8) {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    let bytes = obj.spec.driver_name.as_bytes();
    let n = bytes.len().min(31);
    // SAFETY: the SDK contract: `name` holds at least 32 bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), name, n);
        *name.add(n) = 0;
    }
}
unsafe extern "C" fn get_driver_version(_this: *mut IAsio) -> i32 {
    7
}
unsafe extern "C" fn get_error_message(this: *mut IAsio, text: *mut u8) {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    let bytes = obj.spec.init_message.as_bytes();
    let n = bytes.len().min(123);
    // SAFETY: the SDK contract: `text` holds at least 124 bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), text, n);
        *text.add(n) = 0;
    }
}
unsafe extern "C" fn get_channels(this: *mut IAsio, inputs: *mut i32, outputs: *mut i32) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    if let Some(code) = obj.spec.channels_error {
        return code;
    }
    // SAFETY: valid out-pointers by the SDK contract.
    unsafe {
        *inputs = obj.spec.inputs;
        *outputs = obj.spec.outputs;
    }
    abi::ASE_OK
}
unsafe extern "C" fn get_latencies(this: *mut IAsio, input: *mut i32, output: *mut i32) -> i32 {
    // SAFETY: called through a live object's vtable; valid out-pointers by the SDK contract.
    unsafe {
        let obj = object(this);
        obj.observed.call("getLatencies");
        let extra = obj.observed.extra_latency.load(Ordering::SeqCst) as i32;
        *input = obj.spec.latencies.0 + extra;
        *output = obj.spec.latencies.1 + extra;
    }
    abi::ASE_OK
}
unsafe extern "C" fn get_buffer_size(this: *mut IAsio, min: *mut i32, max: *mut i32, preferred: *mut i32, granularity: *mut i32) -> i32 {
    // SAFETY: called through a live object's vtable; valid out-pointers by the SDK contract.
    unsafe {
        let (a, b, c, d) = object(this).spec.buffer;
        *min = a;
        *max = b;
        *preferred = c;
        *granularity = d;
    }
    abi::ASE_OK
}
unsafe extern "C" fn can_sample_rate(this: *mut IAsio, rate: f64) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    if obj.spec.rates.contains(&rate) { abi::ASE_OK } else { abi::ASE_NO_CLOCK }
}
unsafe extern "C" fn get_sample_rate(this: *mut IAsio, rate: *mut f64) -> i32 {
    // SAFETY: called through a live object's vtable; a valid out-pointer by the SDK contract.
    unsafe { *rate = *object(this).rate.lock().unwrap() };
    abi::ASE_OK
}
unsafe extern "C" fn set_sample_rate(this: *mut IAsio, rate: f64) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    if !obj.spec.rates.contains(&rate) {
        return abi::ASE_NO_CLOCK;
    }
    *obj.rate.lock().unwrap() = rate;
    abi::ASE_OK
}
unsafe extern "C" fn get_clock_sources(_this: *mut IAsio, _clocks: *mut abi::AsioClockSource, count: *mut i32) -> i32 {
    // SAFETY: a valid out-pointer by the SDK contract.
    unsafe { *count = 0 };
    abi::ASE_OK
}
unsafe extern "C" fn set_clock_source(_this: *mut IAsio, _reference: i32) -> i32 {
    abi::ASE_OK
}
unsafe extern "C" fn get_sample_position(this: *mut IAsio, position: *mut abi::AsioSamples, stamp: *mut abi::AsioTimeStamp) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object_quiet(this) };
    obj.observed.call("getSamplePosition");
    let at = obj.observed.position.load(Ordering::SeqCst);
    if at == u64::MAX {
        return abi::ASE_SP_NOT_ADVANCING;
    }
    // SAFETY: valid out-pointers by the SDK contract.
    unsafe {
        *position = abi::AsioSamples::from_value(at);
        *stamp = abi::AsioTimeStamp::from_value(obj.observed.system_time.load(Ordering::SeqCst));
    }
    abi::ASE_OK
}
unsafe extern "C" fn get_channel_info(this: *mut IAsio, info: *mut AsioChannelInfo) -> i32 {
    // SAFETY: called through a live object's vtable; `info` is valid by the SDK contract.
    unsafe {
        let obj = object(this);
        let info = &mut *info;
        let (count, sample) = if info.isInput != 0 { (obj.spec.inputs, obj.spec.input_type) } else { (obj.spec.outputs, obj.spec.output_type) };
        if info.channel < 0 || info.channel >= count {
            return abi::ASE_INVALID_PARAMETER;
        }
        info.isActive = 0;
        info.channelGroup = 0;
        info.type_ = sample;
        info.name = [0; 32];
        let name = format!("{} {}", if info.isInput != 0 { "Analog In" } else { "Analog Out" }, info.channel + 1);
        for (to, from) in info.name.iter_mut().zip(name.bytes().take(31)) {
            *to = from;
        }
    }
    abi::ASE_OK
}
unsafe extern "C" fn create_buffers(this: *mut IAsio, infos: *mut abi::AsioBufferInfo, count: i32, frames: i32, cb: *mut abi::AsioCallbacks) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    obj.observed.call("createBuffers");
    if let Some(code) = obj.spec.create_error {
        return code;
    }
    let nth = obj.creates.fetch_add(1, Ordering::SeqCst) + 1;
    if obj.spec.create_fails_from.is_some_and(|from| nth >= from) {
        return abi::ASE_NO_MEMORY;
    }
    assert!(!cb.is_null(), "createBuffers gets the callback table");
    obj.callbacks.store(cb as usize, Ordering::SeqCst);
    assert!(obj.observed.buffers.lock().unwrap().is_empty(), "createBuffers twice without disposeBuffers");
    // SAFETY: the SDK contract: `infos` holds `count` entries the driver fills.
    let infos = unsafe { std::slice::from_raw_parts_mut(infos, usize::try_from(count).unwrap()) };
    for info in infos {
        let input = info.isInput != 0;
        let (channels, sample) = if input { (obj.spec.inputs, obj.spec.input_type) } else { (obj.spec.outputs, obj.spec.output_type) };
        if info.channelNum < 0 || info.channelNum >= channels {
            return abi::ASE_INVALID_PARAMETER;
        }
        let width = crate::convert::Wire::of(SampleType::from_code(sample)).map_or(4, crate::convert::Wire::bytes);
        let bytes = usize::try_from(frames).unwrap() * width;
        // Drivers hand buffers over uninitialized: garbage, so silence must be written.
        let addresses = [0, 1].map(|_| Box::into_raw(vec![0xEEu8; bytes].into_boxed_slice()).cast::<u8>() as usize);
        info.buffers = addresses.map(|a| a as *mut c_void);
        obj.observed.buffers.lock().unwrap().push(FakeBuffer { input, channel: info.channelNum, addresses, bytes });
    }
    abi::ASE_OK
}
unsafe extern "C" fn dispose_buffers(this: *mut IAsio) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    obj.observed.call("disposeBuffers");
    let created: Vec<FakeBuffer> = obj.observed.buffers.lock().unwrap().drain(..).collect();
    for b in created {
        if obj.spec.poison_on_dispose {
            for address in b.addresses {
                // SAFETY: a live buffer of `b.bytes` bytes from `create_buffers`; the host gave up
                // its addresses by calling `disposeBuffers` (if it did not, the poison shows it).
                unsafe { std::ptr::write_bytes(address as *mut u8, POISON, b.bytes) };
            }
            obj.observed.disposed.lock().unwrap().push(b);
        } else {
            free(b);
        }
    }
    let table = obj.callbacks.load(Ordering::SeqCst) as *const abi::AsioCallbacks;
    if obj.spec.switch_in_dispose && !table.is_null() {
        // SAFETY: the `ASIOCallbacks *` `createBuffers` was given: the host's static table, valid
        // for the life of the process and never written after initialization.
        let switch = unsafe { (*table).bufferSwitch };
        for half in 0..2 {
            if let Some(switch) = switch {
                // SAFETY: plain values, as a driver calls it.
                unsafe { switch(half, abi::ASIO_TRUE) };
            }
        }
    }
    abi::ASE_OK
}
unsafe extern "C" fn start(this: *mut IAsio) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    obj.observed.call("start");
    obj.spec.start_error.unwrap_or(abi::ASE_OK)
}
unsafe extern "C" fn stop(this: *mut IAsio) -> i32 {
    // SAFETY: called through a live object's vtable.
    unsafe { object(this) }.observed.call("stop");
    abi::ASE_OK
}
unsafe extern "C" fn output_ready(this: *mut IAsio) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object_quiet(this) };
    obj.observed.output_ready.fetch_add(1, Ordering::SeqCst);
    if obj.spec.output_ready { abi::ASE_OK } else { abi::ASE_NOT_PRESENT }
}
unsafe extern "C" fn control_panel(this: *mut IAsio) -> i32 {
    // SAFETY: called through a live object's vtable.
    let obj = unsafe { object(this) };
    obj.observed.panels.fetch_add(1, Ordering::AcqRel);
    obj.spec.panel_error.unwrap_or(abi::ASE_OK)
}
unsafe extern "C" fn future(_this: *mut IAsio, _selector: i32, _opt: *mut c_void) -> i32 {
    abi::ASE_INVALID_PARAMETER
}

static VTBL: IAsioVtbl = IAsioVtbl {
    QueryInterface: query_interface,
    AddRef: add_ref,
    Release: release,
    init,
    getDriverName: get_driver_name,
    getDriverVersion: get_driver_version,
    getErrorMessage: get_error_message,
    start,
    stop,
    getChannels: get_channels,
    getLatencies: get_latencies,
    getBufferSize: get_buffer_size,
    canSampleRate: can_sample_rate,
    getSampleRate: get_sample_rate,
    setSampleRate: set_sample_rate,
    getClockSources: get_clock_sources,
    setClockSource: set_clock_source,
    getSamplePosition: get_sample_position,
    getChannelInfo: get_channel_info,
    createBuffers: create_buffers,
    disposeBuffers: dispose_buffers,
    controlPanel: control_panel,
    future,
    outputReady: output_ready,
};

/// A new fake driver object with one reference.
pub(crate) fn create(spec: FakeSpec, observed: &Arc<Observed>) -> Driver {
    let live = observed.live.fetch_add(1, Ordering::AcqRel) + 1;
    observed.max_live.fetch_max(live, Ordering::AcqRel);
    observed.created.fetch_add(1, Ordering::AcqRel);
    let rate = Mutex::new(spec.current_rate);
    let obj = Box::new(FakeObject {
        vtbl: &VTBL,
        refs: AtomicU32::new(1),
        rate,
        spec,
        observed: Arc::clone(observed),
        creates: AtomicUsize::new(0),
        callbacks: AtomicUsize::new(0),
    });
    // SAFETY: a fresh `FakeObject` whose first field is the vtable pointer, with one reference
    // that the `Driver` owns; released (and freed) through the vtable's `Release`.
    unsafe { Driver::from_raw(Box::into_raw(obj).cast()) }.expect("non-null")
}

/// A loader of fake drivers keyed by registry name.
pub(crate) struct FakeLoader {
    pub drivers: Vec<(DriverEntry, FakeSpec)>,
    /// Names whose creation fails like a failed `CoCreateInstance`.
    pub create_fails: Vec<String>,
    pub observed: Arc<Observed>,
}

impl FakeLoader {
    pub(crate) fn new(drivers: Vec<(DriverEntry, FakeSpec)>) -> (Self, Arc<Observed>) {
        let observed = Arc::new(Observed::default());
        // The callback-thread calls record into this without allocating (the allocation test).
        observed.calls.lock().unwrap().reserve(4096);
        (Self { drivers, create_fails: Vec::new(), observed: Arc::clone(&observed) }, observed)
    }
}

impl Loader for FakeLoader {
    fn list(&self) -> Result<Vec<DriverEntry>, ab::OpenError> {
        Ok(self.drivers.iter().map(|(e, _)| e.clone()).collect())
    }
    /// Counting MMCSS calls ([`crate::mmcss::testing`]): no test thread is really registered.
    fn mmcss(&self) -> crate::mmcss::Hooks {
        crate::mmcss::testing::HOOKS
    }
    fn create(&self, entry: &DriverEntry) -> Result<Driver, ab::OpenError> {
        if self.create_fails.contains(&entry.name) {
            return Err(create_error(entry, windows::Win32::Foundation::REGDB_E_CLASSNOTREG, "Class not registered"));
        }
        let (_, spec) = self.drivers.iter().find(|(e, _)| e.name == entry.name).expect("a fake driver of that name");
        Ok(create(spec.clone(), &self.observed))
    }
}

/// `SendMessageW(window, WM_NULL)` from the calling thread; returns when the window's thread
/// dispatched it.
pub(crate) fn send_null(window: usize) {
    // SAFETY: `SendMessageW` to a window handle (a stale one fails cleanly); `WM_NULL` carries
    // no pointers.
    unsafe { windows::Win32::UI::WindowsAndMessaging::SendMessageW(HWND(window as *mut c_void), 0, None, None) };
}

/// A registry entry for a fake driver.
pub(crate) fn entry(name: &str, n: u128) -> DriverEntry {
    DriverEntry {
        name: name.into(),
        clsid: windows_core::GUID::from_u128(n),
        clsid_text: format!("{{{n:032X}}}"),
        description: Some(name.into()),
        dll: Some(format!("C:\\fake\\{n}.dll")),
        dll_exists: true,
        threading_model: Some("Apartment".into()),
    }
}
