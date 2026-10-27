//! A lost graphics device and a panic on the UI thread end the app through the bounded shutdown
//! (issue #104, escape E56), not a frozen window.
//!
//! wgpu reports a lost device only through the callback registered here (its error sink drops the
//! error), and egui-wgpu then panics on the next buffer upload. The callback flags the loss; the
//! next frame's `logic` sees the flag before egui-wgpu paints and takes the shutdown path
//! ([`DeviceWatch::lost`]). A loss first seen inside that upload still panics there; `main`
//! catches the panic around `eframe::run_native` and finishes the same shutdown ([`after_panic`]).
//! Re-creating the device is not attempted (known issue K84).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use transit_ui_studio::host::ShutDown;

/// Whether the graphics device was lost, and why: set from wgpu's device-lost callback (on
/// whichever thread wgpu detected it), read by the frame loop.
#[derive(Default)]
pub struct DeviceWatch {
    lost: AtomicBool,
    reason: Mutex<Option<String>>,
}

impl DeviceWatch {
    /// Records a loss (the first reason is kept) and logs it.
    pub fn report(&self, reason: eframe::wgpu::DeviceLostReason, message: &str) {
        let text = match reason {
            eframe::wgpu::DeviceLostReason::Destroyed => format!("the graphics device was destroyed {message}"),
            eframe::wgpu::DeviceLostReason::Unknown => format!("the graphics device was lost: {message}"),
        };
        let text = text.trim().to_owned();
        log::error!("{text}");
        self.reason.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(text);
        self.lost.store(true, Ordering::Release);
    }

    /// Why the device was lost, once it has been: the frame loop must not paint again.
    pub fn lost(&self) -> Option<String> {
        if !self.lost.load(Ordering::Acquire) {
            return None;
        }
        Some(self.reason.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_else(|| "the graphics device was lost".into()))
    }
}

/// Registers the device-lost callback: it flags `watch` and asks `ctx` for a frame, so the
/// shutdown path runs even when nothing else would repaint. The callback only stores and logs;
/// it must not call back into wgpu.
pub fn watch_device(device: &eframe::wgpu::Device, watch: &Arc<DeviceWatch>, ctx: &egui::Context) {
    let (watch, ctx) = (Arc::clone(watch), ctx.clone());
    device.set_device_lost_callback(move |reason, message| {
        watch.report(reason, &message);
        ctx.request_repaint();
    });
}

/// The result of the app's own quit order, for `after_panic`: bit flags, 0 while it has not run.
const RAN: u8 = 1;
const CLEAN: u8 = 2;
const AUTOSAVED: u8 = 4;
static SHUT_DOWN: AtomicU8 = AtomicU8::new(0);

/// Records the result of the app's quit order (run from `on_exit`, the device-lost path, or the
/// app being dropped while a panic unwinds).
pub fn record(result: ShutDown) {
    let flags = RAN | if result.clean { CLEAN } else { 0 } | if result.autosaved { AUTOSAVED } else { 0 };
    SHUT_DOWN.store(flags, Ordering::Release);
}

/// After a panic unwound out of `eframe::run_native`: the app was normally dropped on the way
/// and ran its quit order then; if it was not (the app is unreachable), plug-in hosting is shut
/// down here, which a still-alive player makes unclean. Tells the user and ends the process.
pub fn after_panic(payload: &(dyn std::any::Any + Send)) -> ! {
    let what = payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default();
    let reason =
        if what.is_empty() { "an internal error on the UI thread".to_owned() } else { format!("an internal error on the UI thread ({what})") };
    log::error!("emergency shutdown: {reason}");
    let flags = SHUT_DOWN.load(Ordering::Acquire);
    let result = if flags & RAN != 0 {
        ShutDown { clean: flags & CLEAN != 0, autosaved: flags & AUTOSAVED != 0 }
    } else {
        let clean = transit_engine::emergency::run_bounded(
            "shutdown after a panic",
            transit_engine::emergency::SHUTDOWN_BOUND,
            transit_engine::shutdown_plugin_hosts,
        );
        ShutDown { clean, autosaved: false }
    };
    tell_user(&reason, result);
    transit_engine::emergency::end_process(result.clean)
}

/// How long the closing message stays up before the process ends anyway. Short: the whole
/// emergency path stays within the shutdown watchdog's bound plus this.
// timing: stall-bound a cap on how long a lost device or a panic keeps the process alive
const MESSAGE_WAIT: Duration = Duration::from_secs(5);

/// The closing message: why, and what became of unsaved changes and plug-ins.
fn closing_message(reason: &str, result: ShutDown) -> String {
    let mut text = format!(
        "transit had to close: {reason}.

"
    );
    text.push_str(if result.autosaved {
        "Unsaved changes were kept in the autosave; transit offers them the next time it starts."
    } else {
        "No new autosave was written; the last save (or an earlier autosave) stands."
    });
    if !result.clean {
        text.push_str(" A plug-in did not shut down in time, so transit ended without unloading it.");
    }
    text.push_str(
        "

The log has the details (Help > Debug Logging shows where it is).",
    );
    text
}

/// Shows why transit closed in a native message box, on a thread of its own, for at most
/// [`MESSAGE_WAIT`]; the log line is written in any case. Not in runs without preferences
/// (agents' test runs), and not on macOS, where the box needs the main thread this path holds.
pub fn tell_user(reason: &str, result: ShutDown) {
    let text = closing_message(reason, result);
    log::error!(
        "{}",
        text.replace(
            "

", " "
        )
    );
    if !crate::prefs_enabled() || cfg!(target_os = "macos") {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let shown = std::thread::Builder::new().name("closing-message".into()).spawn(move || {
        rfd::MessageDialog::new()
            .set_title("transit")
            .set_description(text)
            .set_buttons(rfd::MessageButtons::Ok)
            .set_level(rfd::MessageLevel::Error)
            .show();
        let _ = tx.send(());
    });
    if shown.is_ok() {
        let _ = rx.recv_timeout(MESSAGE_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::wgpu::DeviceLostReason;

    /// A frame driver in eframe's order: the app's logic, then egui-wgpu's buffer upload, which
    /// panics on a lost device as egui-wgpu does (`write_buffer_with` returns `None`). The logic
    /// checks the watch the way `App::logic` does and takes the shutdown path instead.
    struct FakeFrames {
        watch: Arc<DeviceWatch>,
        device_ok: bool,
        painted: usize,
        shut_down: Option<String>,
    }

    impl FakeFrames {
        fn frame(&mut self) {
            if let Some(reason) = self.watch.lost() {
                self.shut_down.get_or_insert(reason);
                return;
            }
            assert!(self.device_ok, "egui-wgpu: failed to create a staging buffer (the device is lost)");
            self.painted += 1;
        }
    }

    #[test]
    fn a_lost_device_routes_the_next_frame_to_the_shutdown_path_before_the_paint() {
        let watch = Arc::new(DeviceWatch::default());
        let mut frames = FakeFrames { watch: Arc::clone(&watch), device_ok: true, painted: 0, shut_down: None };
        for _ in 0..3 {
            frames.frame();
        }
        assert_eq!((frames.painted, frames.shut_down.as_deref()), (3, None));
        // The driver loses the device between frames; wgpu calls the callback on detection.
        frames.device_ok = false;
        watch.report(DeviceLostReason::Unknown, "DXGI_ERROR_DEVICE_REMOVED");
        for _ in 0..3 {
            frames.frame();
        }
        assert_eq!(frames.painted, 3, "no paint after the loss");
        assert_eq!(frames.shut_down.as_deref(), Some("the graphics device was lost: DXGI_ERROR_DEVICE_REMOVED"));
        // A later report keeps the first reason.
        watch.report(DeviceLostReason::Destroyed, "");
        assert_eq!(watch.lost().as_deref(), Some("the graphics device was lost: DXGI_ERROR_DEVICE_REMOVED"));
    }

    #[test]
    fn the_closing_message_claims_an_autosave_only_when_one_was_written() {
        let saved = closing_message("x", ShutDown { clean: true, autosaved: true });
        assert!(saved.contains("kept in the autosave") && !saved.contains("without unloading"));
        let not_saved = closing_message("x", ShutDown { clean: false, autosaved: false });
        assert!(!not_saved.contains("kept in the autosave") && not_saved.contains("the last save"), "{not_saved}");
        assert!(not_saved.contains("without unloading"));
    }

    /// The real callback on a real device: Windows' software adapter (WARP), destroyed. wgpu calls
    /// the callback from the next poll; the watch reads lost and the context was asked to repaint.
    #[cfg(windows)]
    #[test]
    fn destroying_a_warp_device_fires_the_callback_and_flags_the_watch() {
        use eframe::wgpu;
        use std::task::{Context, Poll};
        // wgpu's native futures are ready at once; no executor is needed.
        fn ready<T>(future: impl std::future::Future<Output = T>) -> T {
            let mut future = std::pin::pin!(future);
            match future.as_mut().poll(&mut Context::from_waker(std::task::Waker::noop())) {
                Poll::Ready(value) => value,
                Poll::Pending => panic!("a native wgpu future was not ready"),
            }
        }
        // FXC, as the app configures it (`graphics.rs`): without a matching DXIL runtime, the default
        // compiler search loses the device while it is created (issue #185 upstream).
        let mut descriptor = wgpu::InstanceDescriptor { backends: wgpu::Backends::DX12, ..wgpu::InstanceDescriptor::new_without_display_handle() };
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = ready(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            force_fallback_adapter: true,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .expect("Windows has the WARP adapter");
        let (device, _queue) = ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a WARP device");
        let watch = Arc::new(DeviceWatch::default());
        let ctx = egui::Context::default();
        watch_device(&device, &watch, &ctx);
        assert_eq!(watch.lost(), None);
        device.destroy();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        assert_eq!(watch.lost().as_deref(), Some("the graphics device was destroyed"));
        assert!(ctx.has_requested_repaint(), "the callback asks for a frame so the shutdown path runs");
    }
}
