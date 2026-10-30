//! PipeWire's clock as Audio Devices reads it (#230). Through PipeWire's ALSA plug-in, transit's
//! buffer size is an ALSA buffer: cpal's ALSA host splits a fixed buffer into [`ALSA_PERIODS`]
//! periods (the pinned fork's `host/alsa/mod.rs`: `set_period_size_near(v / 4)`, then
//! `set_buffer_size(v)`), and the period is what PipeWire schedules transit's node at (what
//! `pw-top` shows), while the graph runs at the server's quantum. Under JACK (and PipeWire's JACK
//! library) buffer, period and quantum are one number.
//!
//! The server's settings come from `pw-metadata -n settings 0` (the `settings` metadata object:
//! `clock.quantum`, `clock.min-quantum`, `clock.max-quantum`, `clock.force-quantum`), run when the
//! devices are listed, never on the audio path. The text it prints is parsed by
//! [`parse_settings`], tested without a server.

use crate::devices::{AudioSystem, DeviceInfo};
use std::sync::{Mutex, PoisonError};

/// The periods cpal's ALSA host splits a fixed buffer into.
pub const ALSA_PERIODS: u32 = 4;

/// The server's clock settings; `None` for a key it did not print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerClock {
    pub rate: Option<u32>,
    /// The default quantum (`clock.quantum`); the graph runs at the smallest quantum its clients
    /// ask for, inside the allowed range, unless one is forced.
    pub quantum: Option<u32>,
    pub min_quantum: Option<u32>,
    pub max_quantum: Option<u32>,
    /// A quantum pinned with `clock.force-quantum` (`None` for 0, nothing pinned).
    pub force_quantum: Option<u32>,
}

impl ServerClock {
    /// The quanta the server allows (`clock.min-quantum` to `clock.max-quantum`).
    pub fn quantum_range(&self) -> Option<(u32, u32)> {
        match (self.min_quantum, self.max_quantum) {
            (Some(min), Some(max)) if min > 0 && min <= max => Some((min, max)),
            _ => None,
        }
    }

    /// The ALSA buffers whose periods the server allows: the quantum range times [`ALSA_PERIODS`].
    pub fn alsa_buffer_range(&self) -> Option<(u32, u32)> {
        self.quantum_range().map(|(min, max)| (min.saturating_mul(ALSA_PERIODS), max.saturating_mul(ALSA_PERIODS)))
    }

    /// The line under a device that PipeWire serves ("<device>: ..."): the quantum, its range, a
    /// forced one, and what a buffer size means there.
    pub fn device_note(&self, device: &str, alsa: bool) -> String {
        let mut line = format!("{device}: PipeWire server");
        if let Some(q) = self.quantum {
            line.push_str(&format!(", default quantum {q}"));
        }
        if let Some((min, max)) = self.quantum_range() {
            line.push_str(&format!(", allowed {min}-{max}"));
        }
        if let Some(f) = self.force_quantum {
            line.push_str(&format!(", forced to {f} (transit cannot change it)"));
        }
        if alsa {
            line.push_str(&format!(
                ". Through ALSA the buffer is split into {ALSA_PERIODS} periods and PipeWire runs transit at the period; the JACK system sets the quantum itself."
            ));
        } else {
            line.push_str(". The buffer size here is the quantum.");
        }
        line
    }
}

/// Parses `pw-metadata -n settings 0` output, lines like
/// `update: id:0 key:'clock.quantum' value:'1024' type:''`. `None` when no clock key is there.
pub fn parse_settings(text: &str) -> Option<ServerClock> {
    let mut clock = ServerClock::default();
    let mut found = false;
    let quoted = |line: &str, field: &str| -> Option<String> {
        let start = line.find(field)? + field.len();
        let rest = &line[start..];
        rest.find('\'').map(|end| rest[..end].to_owned())
    };
    for line in text.lines() {
        let (Some(key), Some(value)) = (quoted(line, "key:'"), quoted(line, "value:'")) else { continue };
        let number = value.trim().parse::<u32>().ok();
        let slot = match key.as_str() {
            "clock.rate" => &mut clock.rate,
            "clock.quantum" => &mut clock.quantum,
            "clock.min-quantum" => &mut clock.min_quantum,
            "clock.max-quantum" => &mut clock.max_quantum,
            "clock.force-quantum" => &mut clock.force_quantum,
            _ => continue,
        };
        found = true;
        *slot = number.filter(|n| *n > 0);
    }
    found.then_some(clock)
}

/// Whether an ALSA device of this name goes through PipeWire's plug-in: the `pipewire` PCM always,
/// `default` only where pipewire-alsa makes it PipeWire's ([`default_is_pipewire`]; a desktop that
/// runs PipeWire beside PulseAudio keeps `default` on PulseAudio). `pulse`, `hw:` and `plughw:`
/// are not counted.
pub fn routed(device: &str) -> bool {
    routed_with(device, default_is_pipewire())
}

/// [`routed`] with the `default` PCM's owner given.
pub fn routed_with(device: &str, default_is_pipewire: bool) -> bool {
    device.eq_ignore_ascii_case("pipewire") || (default_is_pipewire && device.eq_ignore_ascii_case("default"))
}

/// pipewire-alsa's file that makes ALSA's `default` PipeWire's, where the distributions install it.
pub const DEFAULT_CONF: [&str; 2] = ["/etc/alsa/conf.d/99-pipewire-default.conf", "/usr/share/alsa/alsa.conf.d/99-pipewire-default.conf"];

/// Whether ALSA's `default` PCM is PipeWire's: one of [`DEFAULT_CONF`] exists (read once; never on
/// a platform without ALSA).
pub fn default_is_pipewire() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| cfg!(target_os = "linux") && DEFAULT_CONF.iter().any(|p| std::path::Path::new(p).exists()))
}

/// Narrows a device's buffer range to `range` (the server's), keeping what both allow; the
/// server's range alone when they do not overlap.
pub fn clamp_range(device: &mut DeviceInfo, (min, max): (u32, u32)) {
    let lo = device.buffer_min.map_or(min, |m| m.max(min));
    let hi = device.buffer_max.map_or(max, |m| m.min(max));
    (device.buffer_min, device.buffer_max) = if lo <= hi { (Some(lo), Some(hi)) } else { (Some(min), Some(max)) };
}

/// The ALSA system under a PipeWire server: each routed device's buffer list narrowed to the
/// buffers whose periods the server allows, with the server's line under it.
pub fn apply_to_alsa(system: &mut AudioSystem, clock: &ServerClock, default_is_pipewire: bool) {
    let mut notes = Vec::new();
    for device in system.outputs.iter_mut().chain(system.inputs.iter_mut()).filter(|d| routed_with(&d.name, default_is_pipewire)) {
        if let Some(range) = clock.alsa_buffer_range() {
            clamp_range(device, range);
        }
        let note = clock.device_note(&device.name, true);
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    system.diagnostics.extend(notes);
}

/// The clock read at the last listing (`None` before one, or without a PipeWire server).
/// The outer `None`: never read.
static LAST: Mutex<Option<Option<ServerClock>>> = Mutex::new(None);

/// The clock read at the last listing, for the window's per-frame lines (no process is started).
pub fn last_clock() -> Option<ServerClock> {
    LAST.lock().unwrap_or_else(PoisonError::into_inner).flatten()
}

/// The clock read at the last listing, else read now (once): the open path's view.
pub fn clock() -> Option<ServerClock> {
    let known = *LAST.lock().unwrap_or_else(PoisonError::into_inner);
    known.unwrap_or_else(server_clock)
}

/// Reads the server's clock now (Linux, with a PipeWire server's socket present and `pw-metadata`
/// installed; it waits one second at most) and keeps it for [`last_clock`]. A control-thread call.
pub fn server_clock() -> Option<ServerClock> {
    let clock = read_settings().as_deref().and_then(parse_settings);
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(clock);
    clock
}

/// The buffer range the server allows a device of `system` named `device`: the window's list and
/// the open path's clamp both read this (ALSA devices PipeWire serves; `None` for any other).
pub fn alsa_server_range(system: &str, device: &str) -> Option<(u32, u32)> {
    if !system.eq_ignore_ascii_case("ALSA") || !routed(device) {
        return None;
    }
    clock()?.alsa_buffer_range()
}

#[cfg(target_os = "linux")]
fn read_settings() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let runtime = std::env::var_os("PIPEWIRE_RUNTIME_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))?;
    let remote = std::env::var_os("PIPEWIRE_REMOTE").unwrap_or_else(|| "pipewire-0".into());
    if !std::path::Path::new(&runtime).join(remote).exists() {
        return None;
    }
    let mut child =
        Command::new("pw-metadata").args(["-n", "settings", "0"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    Some(text)
}

#[cfg(not(target_os = "linux"))]
fn read_settings() -> Option<String> {
    None
}

/// The Audio Devices line for a stream PipeWire serves through ALSA, when its period is not the
/// buffer that was asked for, or the server pins the quantum: the three numbers (the ALSA
/// `buffer`, the stream `period` transit's callbacks see, the server's quantum) and how to change
/// the quantum. `asked` is the buffer size requested, `period` the frames of the latest callback.
/// `None` before a callback, or when the request was honored as asked.
pub fn period_note(asked: Option<u32>, buffer: Option<u32>, period: Option<u32>, clock: Option<&ServerClock>) -> Option<String> {
    let period = period.filter(|p| *p > 0)?;
    let forced = clock.and_then(|c| c.force_quantum);
    if buffer == Some(period) && asked.is_none_or(|a| a == period) && forced.is_none_or(|f| f == period) {
        return None;
    }
    // A forced quantum is read; without one the graph's quantum is not (no libpipewire): the
    // node's period stands in for it, said as "about".
    let quantum = match forced {
        Some(f) => format!("{f}, forced: transit cannot change it"),
        None => format!("about {period}"),
    };
    // cpal keeps the whole ALSA buffer queued (playback starts at buffer minus one period), so what
    // you hear is about the buffer plus the graph's quantum, not the quantum alone.
    let heard = match buffer {
        Some(b) => format!("You hear about the buffer plus the quantum ({} frames).", b.saturating_add(forced.unwrap_or(period))),
        None => "You hear about the buffer plus the quantum.".to_owned(),
    };
    let buffer = buffer.map_or("default".to_owned(), |b| b.to_string());
    let want = asked.unwrap_or(period);
    Some(format!(
        "Buffer {buffer} · period {period}: period set by the server (PipeWire quantum {quantum}). {heard} To change the quantum: pw-metadata -n settings 0 clock.force-quantum {want} (0 releases it), or use the JACK system."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tester's settings dump after `clock.force-quantum 2048` (#230).
    const FORCED: &str = "Found \"settings\" metadata 32\n\
        update: id:0 key:'log.level' value:'2' type:''\n\
        update: id:0 key:'clock.rate' value:'48000' type:''\n\
        update: id:0 key:'clock.allowed-rates' value:'[ 48000 ]' type:''\n\
        update: id:0 key:'clock.quantum' value:'128' type:''\n\
        update: id:0 key:'clock.min-quantum' value:'64' type:''\n\
        update: id:0 key:'clock.max-quantum' value:'2048' type:''\n\
        update: id:0 key:'clock.force-quantum' value:'2048' type:''\n\
        update: id:0 key:'clock.force-rate' value:'0' type:''\n";

    fn device(name: &str, min: Option<u32>, max: Option<u32>) -> DeviceInfo {
        DeviceInfo { name: name.into(), is_default: false, max_channels: 2, sample_rates: vec![48_000], buffer_min: min, buffer_max: max }
    }

    #[test]
    fn the_settings_dump_gives_the_quantum_its_range_and_a_forced_one() {
        let clock = parse_settings(FORCED).unwrap();
        assert_eq!(
            clock,
            ServerClock { rate: Some(48_000), quantum: Some(128), min_quantum: Some(64), max_quantum: Some(2048), force_quantum: Some(2048) }
        );
        assert_eq!(clock.quantum_range(), Some((64, 2048)));
        assert_eq!(clock.alsa_buffer_range(), Some((256, 8192)));
        let released = parse_settings(&FORCED.replace("force-quantum' value:'2048'", "force-quantum' value:'0'")).unwrap();
        assert_eq!(released.force_quantum, None, "0 pins nothing");
        assert_eq!(parse_settings("Found \"settings\" metadata 32\n"), None);
        assert_eq!(parse_settings("can't connect: Host is down"), None);
    }

    #[test]
    fn a_routed_device_offers_only_buffers_whose_periods_the_server_allows() {
        // The ALSA `default` PCM advertises 1 to 4194304 frames; the server allows 64-2048.
        let clock = parse_settings(FORCED).unwrap();
        let mut system = AudioSystem {
            name: "ALSA".into(),
            is_default: true,
            outputs: vec![device("default", Some(1), Some(4_194_304)), device("hw:CARD=PCH,DEV=0", Some(64), Some(16_384))],
            inputs: vec![device("default", Some(1), Some(4_194_304))],
            diagnostics: Vec::new(),
        };
        apply_to_alsa(&mut system, &clock, true);
        assert_eq!(system.outputs[0].buffer_sizes(), [256, 512, 1024, 2048, 4096, 8192]);
        assert_eq!((system.outputs[1].buffer_min, system.outputs[1].buffer_max), (Some(64), Some(16_384)), "hw: is not PipeWire's");
        assert_eq!(system.inputs[0].buffer_sizes(), [256, 512, 1024, 2048, 4096, 8192]);
        assert_eq!(
            system.diagnostics,
            [
                "default: PipeWire server, default quantum 128, allowed 64-2048, forced to 2048 (transit cannot change it). Through ALSA the buffer is split into 4 periods and PipeWire runs transit at the period; the JACK system sets the quantum itself."
            ],
            "one line per device name"
        );
        // No range known: the standard sizes stay (32 to 8192).
        let mut open = device("default", None, None);
        assert_eq!(open.buffer_sizes().len(), crate::devices::STANDARD_BUFFERS.len());
        clamp_range(&mut open, (64, 2048));
        assert_eq!(open.buffer_sizes(), [64, 128, 256, 512, 1024, 2048], "the JACK system under PipeWire: the quantum range");
    }

    #[test]
    fn default_counts_as_pipewires_only_where_pipewire_alsa_owns_it() {
        assert!(routed_with("pipewire", false) && routed_with("PipeWire", true));
        assert!(routed_with("default", true));
        assert!(!routed_with("default", false), "PipeWire beside PulseAudio: default is PulseAudio's");
        assert!(!routed_with("pulse", true) && !routed_with("hw:CARD=PCH,DEV=0", true));
        let mut system = AudioSystem {
            name: "ALSA".into(),
            is_default: true,
            outputs: vec![device("default", Some(1), Some(4_194_304))],
            inputs: Vec::new(),
            diagnostics: Vec::new(),
        };
        apply_to_alsa(&mut system, &parse_settings(FORCED).unwrap(), false);
        assert_eq!((system.outputs[0].buffer_min, system.diagnostics.len()), (Some(1), 0), "left alone");
    }

    #[test]
    fn the_period_line_names_buffer_period_and_quantum_when_they_differ() {
        let forced = parse_settings(FORCED).unwrap();
        // 128 asked, a 128-frame ALSA buffer, 64-frame callbacks, the graph forced to 2048.
        assert_eq!(
            period_note(Some(128), Some(128), Some(64), Some(&forced)).as_deref(),
            Some(
                "Buffer 128 · period 64: period set by the server (PipeWire quantum 2048, forced: transit cannot change it). You hear about the buffer plus the quantum (2176 frames). To change the quantum: pw-metadata -n settings 0 clock.force-quantum 128 (0 releases it), or use the JACK system."
            )
        );
        let free = ServerClock { force_quantum: None, ..forced };
        assert_eq!(
            period_note(Some(2048), Some(2048), Some(512), Some(&free)).as_deref(),
            Some(
                "Buffer 2048 · period 512: period set by the server (PipeWire quantum about 512). You hear about the buffer plus the quantum (2560 frames). To change the quantum: pw-metadata -n settings 0 clock.force-quantum 2048 (0 releases it), or use the JACK system."
            )
        );
        assert_eq!(period_note(Some(256), Some(256), Some(256), Some(&free)), None, "honored as asked");
        assert_eq!(period_note(Some(256), Some(256), None, Some(&free)), None, "no callback yet");
        assert!(period_note(None, None, Some(1024), None).unwrap().starts_with("Buffer default · period 1024: "));
    }
}
