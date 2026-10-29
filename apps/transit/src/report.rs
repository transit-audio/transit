//! `transit --report [path]` (#114): write the diagnostics report to a file and print its path.
//!
//! Runs before the logger, the window and the audio: no display, no stream, and the log it reads is
//! not rotated by this run. Devices are listed, never opened. The text is the one the Copy
//! diagnostics buttons put on the clipboard (`transit_ui_studio::diagnostics`).

use std::path::PathBuf;
use transit_ui_studio::diagnostics::{self, Machine, Redaction, Report};

/// `Some(target)` when `--report` is among the arguments; `target` is the path after it, if the next
/// argument is not another flag.
pub fn parse(args: &[String]) -> Option<Option<PathBuf>> {
    let i = args.iter().position(|a| a == "--report")?;
    Some(args.get(i + 1).filter(|a| !a.starts_with("--")).map(PathBuf::from))
}

/// Builds the report, writes it and returns where. `log_dir` is the log folder (`None` when
/// preferences are off), `settings` the saved audio choices, `profile_mappings` the saved
/// controller profile's MIDI Learn mappings.
pub fn run(
    target: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    settings: transit_playback::AudioConfig,
    profile_mappings: usize,
) -> Result<PathBuf, String> {
    let now = diagnostics::now_secs();
    let path = target.unwrap_or_else(|| PathBuf::from(diagnostics::report_file_name(now)));
    let report = Report {
        generated: now,
        version: env!("CARGO_PKG_VERSION").into(),
        build: env!("TRANSIT_BUILD").into(),
        machine: Machine::probe(),
        systems: diagnostics::machine_systems(),
        settings,
        opened: vec!["Output open: none (report mode opens no stream)".into()],
        telemetry: None,
        midi: midi(profile_mappings),
        open_asio_driver: None,
        strips: Vec::new(),
        last_status: None,
        plugin_host: vec!["No plug-in host runs in report mode.".into()],
        deferred_services: 0,
        unplaced_takes: Vec::new(),
        log_lines: log_dir.as_ref().map(|d| diagnostics::tail_log(&d.join(diagnostics::LOG_FILE), diagnostics::LOG_LINES)).unwrap_or_default(),
        log_dir,
    };
    let text = report.render(&Redaction::from_env());
    std::fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(std::path::absolute(&path).unwrap_or(path))
}

/// The MIDI section of report mode: the ports present (listed, none opened; a running transit is
/// not asked which it has open) and the saved profile's mappings; no session, nothing heard.
fn midi(profile_mappings: usize) -> diagnostics::Midi {
    let (ports, note) = match diagnostics::list_midi_ports(None) {
        Ok(ports) => (ports, "report mode opens no MIDI port, so none is shown open".to_owned()),
        Err(e) => (Vec::new(), format!("the MIDI ports could not be listed: {e}")),
    };
    diagnostics::Midi { ports, note: Some(note), session_mappings: None, profile_mappings, in_force: None, last_heard: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn report_flag_takes_an_optional_path() {
        assert_eq!(parse(&args(&["--demo"])), None);
        assert_eq!(parse(&args(&["--report"])), Some(None));
        assert_eq!(parse(&args(&["--report", "out.txt"])), Some(Some(PathBuf::from("out.txt"))));
        assert_eq!(parse(&args(&["--report", "--verbose"])), Some(None));
    }
}
