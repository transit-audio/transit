//! Device notifications: a registered `IMMNotificationClient` on its own MTA thread (moved out of
//! `com.rs` unchanged; the COM rules there apply).
#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use crate::com::{CONTROL_TIMEOUT, ComGuard, enumerator, open_error};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Duration;
use transit_audio_backend as ab;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMNotificationClient, IMMNotificationClient_Impl, eConsole, eRender,
};
use windows::core::{PCWSTR, implement};

/// The notification client: callbacks arrive on COM's threads and only post events.
#[implement(IMMNotificationClient)]
struct Notifier {
    events: Sender<ab::DeviceEvent>,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, state: DEVICE_STATE) -> windows::core::Result<()> {
        if state != DEVICE_STATE_ACTIVE {
            // SAFETY: the callback's id is a NUL-terminated UTF-16 string valid for the call.
            let _ = self.events.send(ab::DeviceEvent::Unplugged(unsafe { id.to_string() }.unwrap_or_default()));
        }
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
        // SAFETY: as in `OnDeviceStateChanged`.
        let _ = self.events.send(ab::DeviceEvent::Unplugged(unsafe { id.to_string() }.unwrap_or_default()));
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if role == eConsole {
            let direction = if flow == eRender { ab::Direction::Output } else { ab::Direction::Input };
            let _ = self.events.send(ab::DeviceEvent::DefaultChanged(direction));
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

/// A registered `IMMNotificationClient` on its own MTA thread; dropping unregisters it (on that
/// thread, outside any handler) before the client is released.
pub(crate) struct Watcher {
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub(crate) fn start(mut handler: Box<dyn FnMut(ab::DeviceEvent) + Send>) -> Result<Self, ab::OpenError> {
        let done = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), ab::OpenError>>(1);
        let flag = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("transit-wasapi-watch".into())
            .spawn(move || {
                let setup = (|| {
                    let com = ComGuard::mta()?;
                    let enumerator = enumerator()?;
                    let (events, inbox) = mpsc::channel();
                    let client: IMMNotificationClient = Notifier { events }.into();
                    // SAFETY: registers a live notification client on a live enumerator, on the
                    // MTA thread that owns both; it is unregistered below, on this thread, before
                    // either is released.
                    unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }
                        .map_err(|e| open_error("RegisterEndpointNotificationCallback", &e))?;
                    Ok::<_, ab::OpenError>((com, enumerator, client, inbox))
                })();
                let (com, enumerator, client, inbox) = match setup {
                    Ok(parts) => parts,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                while !flag.load(Ordering::Acquire) {
                    if let Ok(event) = inbox.recv_timeout(Duration::from_millis(100)) {
                        handler(event);
                    }
                }
                // SAFETY: unregisters the client registered above, on the same enumerator and
                // thread, outside any notification handler.
                let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&client) };
                drop((client, enumerator));
                drop(com);
            })
            .map_err(|e| ab::OpenError::Other(format!("could not start the device watcher: {e}")))?;
        match ready_rx.recv_timeout(CONTROL_TIMEOUT) {
            Ok(Ok(())) => Ok(Self { done, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(failure) => {
                // Timed out or the thread died: ask it to finish (it checks `done` once set up) and
                // join it, so no thread outlives the failed start.
                done.store(true, Ordering::Release);
                let _ = thread.join();
                Err(ab::OpenError::Other(match failure {
                    mpsc::RecvTimeoutError::Timeout => "starting the device watcher timed out".into(),
                    mpsc::RecvTimeoutError::Disconnected => "the device watcher thread exited while starting".into(),
                }))
            }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
