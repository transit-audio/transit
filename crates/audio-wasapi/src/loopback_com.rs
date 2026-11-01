//! The COM side of the System Audio loopback open (see [`crate::loopback`]): the client calls of
//! [`LoopbackApi`] on one render `IMMDevice`. The threading rules of `com.rs` apply: everything
//! here runs on the loopback stream's own MTA thread.
#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use crate::com::{self, Fmt};
use crate::exclusive::ApiError;
use crate::loopback::{self, LoopbackApi, LoopbackInit};
use transit_audio_backend as ab;
use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_SHARED, IAudioClient, IMMDevice, WAVEFORMATEX, WAVEFORMATEXTENSIBLE};

fn api_error(error: &windows::core::Error) -> ApiError {
    ApiError { code: error.code().0, text: error.to_string() }
}

/// The loopback client calls on one render endpoint. The mix format read by `mix_format` is kept
/// for `initialize`, which must pass the mixer's exact format.
struct ComLoopback {
    device: IMMDevice,
    mix: Option<(WAVEFORMATEXTENSIBLE, Fmt)>,
}

impl LoopbackApi for ComLoopback {
    type Client = IAudioClient;

    fn activate(&mut self) -> Result<IAudioClient, ApiError> {
        com::activate_client(&self.device).map_err(|e| api_error(&e))
    }

    fn mix_format(&mut self, client: &IAudioClient) -> Result<ab::Format, ApiError> {
        let (raw, fmt) = com::mix_format(client).map_err(|e| ApiError { code: -1, text: e.to_string() })?;
        self.mix = Some((raw, fmt));
        Ok(fmt.format())
    }

    fn device_period(&mut self, client: &IAudioClient) -> Result<i64, ApiError> {
        let mut default = 0i64;
        // SAFETY: `GetDevicePeriod` on a live client (initialized or not) writes the default
        // period, in 100 ns units, through one live `i64` out-pointer; the minimum is not asked.
        unsafe { client.GetDevicePeriod(Some(&mut default), None) }.map_err(|e| api_error(&e))?;
        Ok(default)
    }

    fn initialize(&mut self, client: &IAudioClient, init: &LoopbackInit) -> Result<(), ApiError> {
        let Some((raw, _)) = self.mix.as_ref() else {
            return Err(ApiError { code: -1, text: "the mix format was not read before Initialize".into() });
        };
        // SAFETY: `raw` is the endpoint's mix format as `GetMixFormat` returned it (copied into a
        // `WAVEFORMATEXTENSIBLE`, `cbSize` saying how much of it is valid), live across the call
        // and read through its leading `WAVEFORMATEX`. `Initialize` is called at most once per
        // client: `loopback::open` activates a fresh client for the timer fallback. Shared mode
        // requires `hnsPeriodicity` 0; no session GUID (`None`, the default session).
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                init.flags,
                init.buffer_100ns,
                init.periodicity_100ns,
                (raw as *const WAVEFORMATEXTENSIBLE).cast::<WAVEFORMATEX>(),
                None,
            )
        }
        .map_err(|e| api_error(&e))
    }
}

/// The render endpoint `wanted` names (its id or friendly name; `None` or an unknown name: the
/// system's default output), as (id, friendly name).
pub(crate) fn resolve(wanted: Option<&str>) -> Result<(String, String), ab::OpenError> {
    let outputs = com::list_endpoints(true)?;
    let found = wanted.and_then(|w| outputs.iter().find(|e| e.id == w || e.name.eq_ignore_ascii_case(w)));
    // `list_endpoints` puts the default endpoint first.
    let info = found.or_else(|| outputs.first()).ok_or_else(|| ab::OpenError::DeviceNotAvailable("no output to capture".into()))?;
    Ok((info.id.clone(), info.name.clone()))
}

/// An initialized loopback client on the render endpoint `id`: the client, its wake period in
/// frames, the mix format it delivers, and whether it is event-driven.
pub(crate) fn open_client(id: &str, name: &str) -> Result<(IAudioClient, u32, Fmt, bool), ab::OpenError> {
    let mut api = ComLoopback { device: com::device_by_id(id)?, mix: None };
    let opened = loopback::open(&mut api, name)?;
    let (_, fmt) = api.mix.ok_or_else(|| ab::OpenError::Other("the loopback mix format is missing".into()))?;
    log::info!(
        "WASAPI loopback of {name}: {:?}, {} frames per wake, {}",
        opened.format,
        opened.period_frames,
        if opened.init.event_driven() { "event-driven" } else { "timer-driven" }
    );
    Ok((opened.client, opened.period_frames, fmt, opened.init.event_driven()))
}
