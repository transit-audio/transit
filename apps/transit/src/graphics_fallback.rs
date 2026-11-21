//! Startup when the graphics backend cannot create a surface or an adapter (#762): never a silent exit.
//!
//! The decision logic is here and is pure: which backends each attempt asks for, the log line, the
//! notice shown once the window is up, and the text of the error shown when nothing works. `main`
//! supplies the one effectful step (running the window) as a closure, so tests fake the backend.

use eframe::wgpu::Backends;
use std::path::Path;

/// The backends the platform uses when nothing asks for others: DirectX 12 on Windows (see
/// `graphics.rs`), eframe's own choice (the primary APIs and OpenGL) elsewhere.
pub fn platform_default() -> Backends {
    if cfg!(windows) { Backends::DX12 } else { Backends::PRIMARY | Backends::GL }
}

/// Ask for exactly `backends` when the wgpu instance is created. (Windows goes through
/// `graphics::configure`, which also picks the shader compiler.)
#[cfg(not(windows))]
pub fn set_backends(options: &mut eframe::NativeOptions, backends: Backends) {
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.instance_descriptor.backends = backends;
    }
}

/// What the first attempt asks for and what a retry falls back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// `WGPU_BACKEND`, when set: the one backend choice that is retried.
    pub requested: Option<Backends>,
    pub default: Backends,
}

impl Plan {
    pub fn new(requested: Option<Backends>) -> Plan {
        Plan { requested, default: platform_default() }
    }
}

/// The first attempt failed and the platform default is in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub failed: Backends,
    pub used: Backends,
    /// The reason wgpu gave.
    pub reason: String,
}

impl Fallback {
    /// One line for the status line once the window is up.
    pub fn notice(&self) -> String {
        format!(
            "Graphics: {} could not start, so {} is in use. The WGPU_BACKEND variable asked for {}; the log has the reason.",
            describe(self.failed),
            describe(self.used),
            describe(self.failed)
        )
    }
}

/// How one attempt to run the window ended, as `main` classifies it.
pub enum Failure<E> {
    /// No surface, adapter or device: the window never got as far as creating the app.
    Graphics { reason: String, error: E },
    /// Anything else: not ours to retry.
    Other(E),
}

/// The run ended in an error; `message` is set when the user needs to be told (graphics failed).
pub struct Stop<E> {
    pub error: E,
    pub message: Option<String>,
}

/// Run `start`, retrying once with the platform default when an override failed on graphics.
/// `start` gets the backend choice to apply (`None`: the platform's own way) and the fallback to
/// announce when it is the retry. `log_path` is named in the error text.
pub fn run<E: std::fmt::Display>(
    plan: &Plan,
    log_path: Option<&Path>,
    mut start: impl FnMut(Option<Backends>, Option<&Fallback>) -> Result<(), Failure<E>>,
) -> Result<(), Stop<E>> {
    let (reason, error) = match start(plan.requested, None) {
        Ok(()) => return Ok(()),
        Err(Failure::Other(error)) => return Err(Stop { error, message: None }),
        Err(Failure::Graphics { reason, error }) => (reason, error),
    };
    let first = plan.requested.unwrap_or(plan.default);
    log::error!("graphics: {} could not start: {reason}", describe(first));
    if plan.requested.is_none() || first == plan.default {
        let message = failure_text(&[(first, &reason)], plan.requested.is_some(), log_path);
        return Err(Stop { error, message: Some(message) });
    }
    let fallback = Fallback { failed: first, used: plan.default, reason: reason.clone() };
    log::warn!("graphics: {} (from WGPU_BACKEND) could not start; retrying with {}: {reason}", describe(fallback.failed), describe(fallback.used));
    match start(Some(plan.default), Some(&fallback)) {
        Ok(()) => Ok(()),
        // After a graphics failure any error ends in the text, so the user is never left with nothing.
        Err(Failure::Other(error)) => {
            let second = error.to_string();
            log::error!("graphics: the retry with {} failed: {second}", describe(plan.default));
            let message = failure_text(&[(first, &reason), (plan.default, &second)], true, log_path);
            Err(Stop { error, message: Some(message) })
        }
        Err(Failure::Graphics { reason: second, error }) => {
            log::error!("graphics: {} could not start either: {second}", describe(plan.default));
            let message = failure_text(&[(first, &reason), (plan.default, &second)], true, log_path);
            Err(Stop { error, message: Some(message) })
        }
    }
}

/// The text of the dialog (Windows) or standard error: what failed, and where the log is.
pub fn failure_text(attempts: &[(Backends, &str)], by_env: bool, log_path: Option<&Path>) -> String {
    let mut text = String::from("transit cannot start: the graphics backend could not create a window surface.\n");
    for (i, (backends, reason)) in attempts.iter().enumerate() {
        let origin = if i == 0 && by_env { " (asked for by WGPU_BACKEND)" } else { "" };
        text.push_str(&format!("\n{}{origin}: {reason}", describe(*backends)));
    }
    text.push_str(
        "\n\nA graphics driver update can leave a driver registered where it no longer exists; installing the driver again usually fixes that. ",
    );
    text.push_str("If WGPU_BACKEND is set, remove it and start transit again.");
    match log_path {
        Some(path) => text.push_str(&format!("\n\nLog file: {}", path.display())),
        None => text.push_str("\n\nThere is no log file in this run."),
    }
    text
}

/// A backend set in words, for the log, the notice and the error.
pub fn describe(backends: Backends) -> String {
    let names: Vec<&str> = backends
        .iter_names()
        .map(|(name, _)| match name {
            "VULKAN" => "Vulkan",
            "DX12" => "DirectX 12",
            "METAL" => "Metal",
            "GL" => "OpenGL",
            "BROWSER_WEBGPU" => "WebGPU",
            other => other,
        })
        .collect();
    if names.is_empty() { "no backend".into() } else { names.join(" + ") }
}

/// Tell the user when nothing could start. Windows has no console for a shortcut launch, so a
/// message box; elsewhere standard error.
pub fn show_failure(text: &str) {
    #[cfg(windows)]
    {
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("transit").set_description(text).show();
    }
    #[cfg(not(windows))]
    {
        eprintln!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plan(requested: Option<Backends>) -> Plan {
        Plan { requested, default: Backends::DX12 }
    }

    /// Fakes the window: `working` says which backend choices start. Records every call.
    fn fake<'a>(
        working: &'static [Backends],
        calls: &'a mut Vec<(Option<Backends>, Option<Fallback>)>,
    ) -> impl FnMut(Option<Backends>, Option<&Fallback>) -> Result<(), Failure<&'static str>> + 'a {
        move |asked, fallback| {
            calls.push((asked, fallback.cloned()));
            match asked {
                Some(b) if working.contains(&b) => Ok(()),
                _ => Err(Failure::Graphics { reason: "no surface".into(), error: "wgpu" }),
            }
        }
    }

    #[test]
    fn override_fails_then_default_works_retries_once_with_a_notice() {
        let mut calls = Vec::new();
        let result = run(&plan(Some(Backends::VULKAN)), None, fake(&[Backends::DX12], &mut calls));
        assert!(result.is_ok());
        assert_eq!(calls.len(), 2, "one retry");
        assert_eq!(calls[0], (Some(Backends::VULKAN), None));
        let fallback = calls[1].1.clone().expect("the retry is announced");
        assert_eq!(calls[1].0, Some(Backends::DX12));
        assert_eq!((fallback.failed, fallback.used), (Backends::VULKAN, Backends::DX12));
        let notice = fallback.notice();
        assert!(notice.contains("Vulkan") && notice.contains("DirectX 12") && notice.contains("WGPU_BACKEND"), "{notice}");
    }

    #[test]
    fn override_fails_and_default_fails_stops_with_both_in_the_message() {
        let mut calls = Vec::new();
        let log = PathBuf::from("logs/transit.log");
        let Err(stop) = run(&plan(Some(Backends::VULKAN)), Some(&log), fake(&[], &mut calls)) else { panic!("nothing works") };
        assert_eq!(calls.len(), 2);
        let message = stop.message.expect("the user is told");
        assert!(message.contains("Vulkan (asked for by WGPU_BACKEND)") && message.contains("DirectX 12: no surface"), "{message}");
        assert!(message.contains("logs/transit.log"), "{message}");
    }

    #[test]
    fn no_override_and_default_fails_does_not_retry() {
        let mut calls = Vec::new();
        let Err(stop) = run(&plan(None), None, fake(&[], &mut calls)) else { panic!("nothing works") };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (None, None));
        let message = stop.message.expect("the user is told");
        assert!(message.contains("DirectX 12: no surface") && !message.contains("WGPU_BACKEND)"), "{message}");
        assert!(message.contains("no log file"), "{message}");
    }

    #[test]
    fn success_the_first_time_has_no_notice_and_no_retry() {
        let mut calls = Vec::new();
        assert!(run(&plan(Some(Backends::VULKAN)), None, fake(&[Backends::VULKAN], &mut calls)).is_ok());
        assert_eq!(calls, vec![(Some(Backends::VULKAN), None)]);
    }

    #[test]
    fn an_override_equal_to_the_default_is_not_retried() {
        let mut calls = Vec::new();
        assert!(run(&plan(Some(Backends::DX12)), None, fake(&[], &mut calls)).is_err());
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn a_failure_that_is_not_graphics_is_passed_through_without_a_retry() {
        let mut calls = 0;
        let Err(stop) = run(&plan(Some(Backends::VULKAN)), None, |_, _| {
            calls += 1;
            Err(Failure::Other("app creation"))
        }) else {
            panic!("the error passes through")
        };
        assert_eq!((calls, stop.error, stop.message), (1, "app creation", None));
    }

    #[test]
    fn override_fails_then_the_retry_returns_a_non_graphics_error_still_tells_the_user() {
        let mut calls = 0;
        let log = PathBuf::from("logs/transit.log");
        let Err(stop) = run(&plan(Some(Backends::VULKAN)), Some(&log), |_, _| {
            calls += 1;
            if calls == 1 {
                Err(Failure::Graphics { reason: "no surface".into(), error: "first" })
            } else {
                Err(Failure::Other("event loop recreated"))
            }
        }) else {
            panic!("both attempts failed")
        };
        assert_eq!((calls, stop.error), (2, "event loop recreated"));
        let message = stop.message.expect("the user is told");
        assert!(message.contains("Vulkan (asked for by WGPU_BACKEND): no surface"), "{message}");
        assert!(message.contains("DirectX 12: event loop recreated") && message.contains("logs/transit.log"), "{message}");
    }

    #[test]
    fn backends_are_described_in_words() {
        assert_eq!(describe(Backends::VULKAN | Backends::DX12), "Vulkan + DirectX 12");
        assert_eq!(describe(Backends::empty()), "no backend");
    }
}
