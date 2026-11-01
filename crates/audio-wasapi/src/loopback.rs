//! The System Audio input (#268): a loopback capture of a render endpoint, what the computer plays
//! there, over a small trait of the client calls so the open parameters, the event-to-timer
//! fallback and the refusal message run on a fake in tests (like [`crate::exclusive`]).
//! Control-thread code: it may allocate and format; `loopback_com.rs` implements the trait.
//!
//! The client is always a separate shared-mode client on the render endpoint
//! (`AUDCLNT_STREAMFLAGS_LOOPBACK`), whatever mode or backend the session's device runs in: it
//! takes the endpoint's mix format (rate, channels and sample format as the Windows mixer runs
//! them) and is clocked by that endpoint. It is event-driven where the system signals loopback
//! buffers, else polled on a timer at the device period. The capture loop treats a wait that times
//! out as an idle endpoint, not a lost device: a loopback stream delivers no packets while nothing
//! plays on some drivers, and the reader on the session's side fills that with zeros.

use crate::exclusive::{ApiError, E_ENDPOINT_CREATE_FAILED, E_NOTFOUND};
use crate::render_loop::{E_DEVICE_IN_USE, Fault};
use transit_audio_backend as ab;

/// `AUDCLNT_STREAMFLAGS_LOOPBACK`: capture what a render endpoint plays.
pub(crate) const STREAMFLAGS_LOOPBACK: u32 = 0x0002_0000;
/// `AUDCLNT_STREAMFLAGS_EVENTCALLBACK`: the system signals an event per buffer.
pub(crate) const STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
/// `AUDCLNT_E_WRONG_ENDPOINT_TYPE`: loopback was asked of a capture endpoint.
pub(crate) const E_WRONG_ENDPOINT_TYPE: i32 = 0x8889_0003_u32 as i32;
/// The shared-mode buffer a loopback client asks for, in 100 ns units (100 ms): the loop drains it
/// every period, so it only bounds how long a late wake may be before packets are lost.
pub(crate) const BUFFER_100NS: i64 = 1_000_000;

/// How a loopback client is initialized: always shared mode, the loopback flag, the event flag
/// when event-driven, the buffer duration, and `hnsPeriodicity` 0 (required in shared mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoopbackInit {
    /// `AUDCLNT_SHAREMODE_SHARED`. A loopback client is never exclusive.
    pub(crate) shared: bool,
    pub(crate) flags: u32,
    pub(crate) buffer_100ns: i64,
    pub(crate) periodicity_100ns: i64,
}

impl LoopbackInit {
    /// The parameters of the event-driven (`event`) or the timer-driven client.
    pub(crate) const fn new(event: bool) -> Self {
        let flags = STREAMFLAGS_LOOPBACK | if event { STREAMFLAGS_EVENTCALLBACK } else { 0 };
        Self { shared: true, flags, buffer_100ns: BUFFER_100NS, periodicity_100ns: 0 }
    }

    pub(crate) const fn event_driven(&self) -> bool {
        self.flags & STREAMFLAGS_EVENTCALLBACK != 0
    }
}

/// The client calls of a loopback open, on one render `IMMDevice`.
pub(crate) trait LoopbackApi {
    type Client;
    /// A fresh, uninitialized client of the device (`IMMDevice::Activate`).
    fn activate(&mut self) -> Result<Self::Client, ApiError>;
    /// `GetMixFormat`: the format the endpoint's mixer runs, which the loopback client takes.
    fn mix_format(&mut self, client: &Self::Client) -> Result<ab::Format, ApiError>;
    /// `GetDevicePeriod`: the default period, in 100 ns units.
    fn device_period(&mut self, client: &Self::Client) -> Result<i64, ApiError>;
    /// `Initialize` with `init` at the mix format.
    fn initialize(&mut self, client: &Self::Client, init: &LoopbackInit) -> Result<(), ApiError>;
}

/// An initialized loopback client.
#[derive(Debug)]
pub(crate) struct Opened<C> {
    pub(crate) client: C,
    pub(crate) format: ab::Format,
    pub(crate) init: LoopbackInit,
    /// The device's default period in frames at the mix rate: the loop's wake interval.
    pub(crate) period_frames: u32,
}

/// Whether `code` means Windows will not give a loopback client on this endpoint at all, so the
/// timer fallback is not tried: the endpoint is held exclusively, is gone, or is not a render
/// endpoint.
fn refused(code: i32) -> bool {
    matches!(code, E_DEVICE_IN_USE | E_ENDPOINT_CREATE_FAILED | E_NOTFOUND | E_WRONG_ENDPOINT_TYPE)
}

/// The `OpenError`, worded for Audio Devices, of a failed loopback call `what` on the render
/// endpoint `name`.
pub(crate) fn open_error(name: &str, what: &str, error: &ApiError) -> ab::OpenError {
    match error.code {
        E_DEVICE_IN_USE => ab::OpenError::InUse(format!(
            "Windows refuses to capture {name} while it is open in exclusive mode (by transit or another app). Open the device shared, or choose another output for System Audio."
        )),
        E_ENDPOINT_CREATE_FAILED | E_NOTFOUND => ab::OpenError::DeviceNotAvailable(format!("{name} is not available: {what}: {}", error.text)),
        E_WRONG_ENDPOINT_TYPE => ab::OpenError::Other(format!("{name} is not an output, so it has nothing to capture")),
        code => {
            let text = format!("System Audio could not capture {name}: {what}: {}", error.text);
            match Fault::from_hresult(code) {
                Fault::DeviceGone => ab::OpenError::DeviceNotAvailable(text),
                Fault::Other(_) => ab::OpenError::Other(text),
            }
        }
    }
}

/// Activate and initialize a loopback client on the render endpoint `name`: event-driven first;
/// where that `Initialize` fails for any reason but a refusal, a fresh client polled on a timer (a
/// failed `Initialize` cannot be retried on the same client).
pub(crate) fn open<A: LoopbackApi>(api: &mut A, name: &str) -> Result<Opened<A::Client>, ab::OpenError> {
    let client = api.activate().map_err(|e| open_error(name, "Activate(IAudioClient)", &e))?;
    let format = api.mix_format(&client).map_err(|e| open_error(name, "GetMixFormat", &e))?;
    let period_100ns = api.device_period(&client).map_err(|e| open_error(name, "GetDevicePeriod", &e))?;
    let period_frames = crate::exclusive::frames_of_100ns(period_100ns, format.rate).max(1);
    let event = LoopbackInit::new(true);
    let (client, init) = match api.initialize(&client, &event) {
        Ok(()) => (client, event),
        Err(e) if refused(e.code) => return Err(open_error(name, "Initialize(loopback)", &e)),
        Err(e) => {
            log::info!("WASAPI loopback of {name}: the event-driven open failed ({}); polling on a timer", e.text);
            drop(client);
            let client = api.activate().map_err(|e| open_error(name, "Activate(IAudioClient) for the timer", &e))?;
            let timer = LoopbackInit::new(false);
            api.initialize(&client, &timer).map_err(|e| open_error(name, "Initialize(loopback, timer)", &e))?;
            (client, timer)
        }
    };
    Ok(Opened { client, format, init, period_frames })
}

/// How long the loop waits for a buffer before it drains anyway, in milliseconds: one device period
/// (at least 5 ms, at most 50). A timeout is an idle endpoint, never a fault.
pub(crate) fn wait_ms(period_frames: u32, rate: u32) -> u32 {
    u32::try_from(u64::from(period_frames) * 1000 / u64::from(rate.max(1))).unwrap_or(u32::MAX).clamp(5, 50)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusive::E_UNSUPPORTED_FORMAT;

    /// A render endpoint standing in for the device: its mix format, its period, and the result of
    /// each `Initialize` in turn.
    struct FakeEndpoint {
        mix: ab::Format,
        period_100ns: i64,
        results: Vec<Result<(), ApiError>>,
        activations: u32,
        inits: Vec<(u32, LoopbackInit)>,
    }

    impl FakeEndpoint {
        fn new(rate: u32, channels: u16, results: Vec<Result<(), ApiError>>) -> Self {
            let mix = ab::Format::new(rate, channels, ab::SampleFormat::F32);
            Self { mix, period_100ns: 100_000, results, activations: 0, inits: Vec::new() }
        }
    }

    impl LoopbackApi for FakeEndpoint {
        type Client = u32;
        fn activate(&mut self) -> Result<u32, ApiError> {
            self.activations += 1;
            Ok(self.activations)
        }
        fn mix_format(&mut self, _client: &u32) -> Result<ab::Format, ApiError> {
            Ok(self.mix)
        }
        fn device_period(&mut self, _client: &u32) -> Result<i64, ApiError> {
            Ok(self.period_100ns)
        }
        fn initialize(&mut self, client: &u32, init: &LoopbackInit) -> Result<(), ApiError> {
            self.inits.push((*client, *init));
            if self.results.is_empty() { Ok(()) } else { self.results.remove(0) }
        }
    }

    fn failure(code: i32) -> ApiError {
        ApiError { code, text: "scripted".into() }
    }

    #[test]
    fn the_client_is_shared_loopback_event_driven_at_the_mix_format() {
        let mut endpoint = FakeEndpoint::new(44_100, 2, vec![]);
        let opened = open(&mut endpoint, "Speakers").expect("opens");
        assert_eq!(opened.format, ab::Format::new(44_100, 2, ab::SampleFormat::F32), "the endpoint's mix format, not the session's");
        assert!(opened.init.shared, "a loopback client is never exclusive");
        assert_eq!(opened.init.flags, STREAMFLAGS_LOOPBACK | STREAMFLAGS_EVENTCALLBACK);
        assert_eq!(opened.init.periodicity_100ns, 0, "shared mode takes no periodicity");
        assert_eq!(opened.init.buffer_100ns, BUFFER_100NS);
        assert_eq!(opened.period_frames, 441, "10 ms at 44.1 kHz");
        assert_eq!(endpoint.inits.len(), 1);
    }

    #[test]
    fn a_failed_event_open_falls_back_to_a_fresh_timer_client() {
        let mut endpoint = FakeEndpoint::new(48_000, 8, vec![Err(failure(E_UNSUPPORTED_FORMAT)), Ok(())]);
        let opened = open(&mut endpoint, "Speakers").expect("opens on the timer");
        assert!(!opened.init.event_driven());
        assert_eq!(opened.init.flags, STREAMFLAGS_LOOPBACK, "loopback without the event flag");
        assert!(opened.init.shared);
        let clients: Vec<u32> = endpoint.inits.iter().map(|(c, _)| *c).collect();
        assert_eq!(clients, [1, 2], "the timer client is a new one: a failed Initialize is not retried on the same client");
        assert_eq!(opened.format.channels, 8, "every channel of the mix; the reader folds them to stereo");
    }

    #[test]
    fn an_exclusive_owner_refuses_at_once_with_the_audio_devices_message() {
        let mut endpoint = FakeEndpoint::new(48_000, 2, vec![Err(failure(E_DEVICE_IN_USE))]);
        let error = open(&mut endpoint, "Scarlett 2i2").expect_err("refused");
        assert_eq!(endpoint.inits.len(), 1, "no timer retry against a refusal");
        let ab::OpenError::InUse(text) = error else { panic!("an in-use refusal, got {error:?}") };
        assert!(text.starts_with("Windows refuses to capture Scarlett 2i2 while it is open in exclusive mode"), "{text}");
        assert!(text.contains("choose another output for System Audio"), "{text}");
    }

    #[test]
    fn a_vanished_endpoint_and_a_capture_endpoint_are_named() {
        let gone = open_error("USB Headset", "Initialize(loopback)", &failure(E_ENDPOINT_CREATE_FAILED));
        assert!(matches!(&gone, ab::OpenError::DeviceNotAvailable(t) if t.starts_with("USB Headset is not available")), "{gone:?}");
        let capture = open_error("Microphone", "Initialize(loopback)", &failure(E_WRONG_ENDPOINT_TYPE));
        assert!(matches!(&capture, ab::OpenError::Other(t) if t.contains("not an output")), "{capture:?}");
    }

    #[test]
    fn the_wait_is_one_period_within_bounds() {
        assert_eq!(wait_ms(480, 48_000), 10);
        assert_eq!(wait_ms(64, 48_000), 5);
        assert_eq!(wait_ms(48_000, 48_000), 50);
    }
}
