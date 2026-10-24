//! The INFO line a stream open writes, for bug reports (issue #84): the backend and host, the
//! device, what was asked for against what the system gave, and the latency it reports. A pure
//! formatting function, so the exact text is unit tested with fake values. Control threads only
//! (a device opens on the control thread, never in an audio callback).

use std::time::Duration;
use transit_audio_backend as ab;

/// What one stream open asked for and got.
pub(crate) struct OpenReport<'a> {
    /// `"output"` or `"input"`.
    pub direction: &'static str,
    /// `"native"` (WASAPI or ASIO driven directly) or `"cpal"`.
    pub backend: &'static str,
    /// The audio system (host): `WASAPI`, `ASIO`, `ALSA`, `CoreAudio`, ...
    pub host: &'a str,
    pub device: &'a str,
    pub asked: ab::OpenRequest,
    pub got: ab::Achieved,
}

fn ms(frames: u32, rate: u32) -> String {
    if rate == 0 { "?".to_owned() } else { format!("{:.1}", f64::from(frames) * 1000.0 / f64::from(rate)) }
}

fn latency(latency: Option<Duration>) -> String {
    latency.map_or_else(|| "not reported".to_owned(), |d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
}

/// `audio output opened: backend=native host=WASAPI device="Speakers" mode=exclusive rate=48000 Hz
/// channels=2 sample=F32 buffer=480 frames (10.0 ms, asked 256) latency=12.0 ms`. A value that came
/// out as asked is printed once; one that differs says what was asked.
pub(crate) fn describe(r: &OpenReport<'_>) -> String {
    let (asked, got) = (&r.asked, &r.got);
    let mode = match asked.mode {
        ab::Mode::Shared => "shared",
        _ => "exclusive",
    };
    let rate = if asked.format.rate == got.format.rate {
        format!("{} Hz", got.format.rate)
    } else {
        format!("{} Hz (asked {} Hz)", got.format.rate, asked.format.rate)
    };
    let channels = if asked.format.channels == got.format.channels {
        got.format.channels.to_string()
    } else {
        format!("{} (asked {})", got.format.channels, asked.format.channels)
    };
    let asked_period = asked.period.frames().map_or_else(|| "device default".to_owned(), |f| format!("{f} frames"));
    let buffer = match got.period.frames() {
        Some(f) if asked.period.frames() == Some(f) => format!("{f} frames ({} ms)", ms(f, got.format.rate)),
        Some(f) => format!("{f} frames ({} ms, asked {asked_period})", ms(f, got.format.rate)),
        None => format!("unknown (asked {asked_period})"),
    };
    format!(
        "audio {} opened: backend={} host={} device=\"{}\" mode={mode} rate={rate} channels={channels} sample={:?} buffer={buffer} latency={}",
        r.direction,
        r.backend,
        r.host,
        r.device,
        got.format.sample,
        latency(got.latency)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(rate: u32, channels: u16, period: ab::Period, mode: ab::Mode) -> ab::OpenRequest {
        ab::OpenRequest {
            format: ab::Format::new(rate, channels, ab::SampleFormat::F32),
            period,
            mode,
            origin: ab::HostOrigin::default(),
            calibration: None,
        }
    }

    #[test]
    fn a_device_that_opened_as_asked_prints_each_value_once() {
        let asked = request(48_000, 2, ab::Period::Frames(256), ab::Mode::Exclusive);
        let mut got = ab::Achieved::requested(asked.format, ab::Period::Frames(256));
        got.latency = Some(Duration::from_micros(11_500));
        let line = describe(&OpenReport { direction: "output", backend: "native", host: "WASAPI", device: "Speakers (Realtek)", asked, got });
        assert_eq!(
            line,
            "audio output opened: backend=native host=WASAPI device=\"Speakers (Realtek)\" mode=exclusive rate=48000 Hz channels=2 sample=F32 \
             buffer=256 frames (5.3 ms) latency=11.5 ms"
        );
    }

    #[test]
    fn a_device_that_changed_the_request_says_what_was_asked() {
        let asked = request(44_100, 2, ab::Period::Default, ab::Mode::Shared);
        let mut got = ab::Achieved::requested(ab::Format::new(48_000, 8, ab::SampleFormat::I32), ab::Period::Frames(480));
        got.format.channels = 8;
        let line = describe(&OpenReport { direction: "input", backend: "cpal", host: "ASIO", device: "Fireface", asked, got });
        assert_eq!(
            line,
            "audio input opened: backend=cpal host=ASIO device=\"Fireface\" mode=shared rate=48000 Hz (asked 44100 Hz) channels=8 (asked 2) \
             sample=I32 buffer=480 frames (10.0 ms, asked device default) latency=not reported"
        );
    }

    #[test]
    fn an_unreported_buffer_is_unknown_not_zero() {
        let asked = request(48_000, 2, ab::Period::Frames(128), ab::Mode::Shared);
        let got = ab::Achieved::requested(asked.format, ab::Period::Default);
        let line = describe(&OpenReport { direction: "output", backend: "cpal", host: "ALSA", device: "default", asked, got });
        assert!(line.contains("buffer=unknown (asked 128 frames)"), "{line}");
    }
}
