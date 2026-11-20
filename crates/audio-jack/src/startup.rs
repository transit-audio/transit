//! What the watcher reads once after the start (ADR 0021, amendment for #760, rules 3 and 6), and
//! the Audio Devices lines it makes. Display only: nothing here feeds a stamp, `Achieved.latency`,
//! `Stream::latency`, Path latency or compensation.
//!
//! - **transit's own links.** At the watcher's first tick after a direction's connection attempts,
//!   each pair it asked for is looked for among its port's links (`jack_port_get_all_connections`
//!   on the port transit owns). A pair not seen is one line, historical and one-shot: a link that
//!   appears later does not update it, and nothing is retried. Nothing is asked, and nothing read,
//!   with auto-connect off.
//! - **The graph and the reported latency.** The graph's rate and buffer size as the active client
//!   reported them (the buffer "observed" once a process cycle ran with it), and the latency the
//!   server reports for the device ports each side stands for, read once per session at that same
//!   tick, labeled as reported, never as measured. Under PipeWire a fixed sentence follows.
//!
//! The watcher's tick runs these reads on its own thread under the session lock, with no wait: one
//! read per asked pair and per device port.

use crate::api::{JackClient, Latency};

/// Under PipeWire, after the graph and latency lines (rule 6), verbatim.
pub const PIPEWIRE_STAGES: &str = "PipeWire may perform rate conversion and add buffering. JACK does not identify those stages separately or establish which delays are included in these reported ranges.";

/// One link a direction's start asked for, and what `jack_connect` said (`None`: success).
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub(crate) source: String,
    pub(crate) destination: String,
    pub(crate) returned: Option<String>,
}

/// The lines for the asked pairs of one direction not seen among its ports' links. `input`: the
/// direction's ports are transit's inputs (the pair's destination), else its outputs (the source);
/// `names` are those ports' full names, by index.
pub(crate) fn link_lines(client: &dyn JackClient, input: bool, names: &[String], asked: &[Asked], pipewire: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for pair in asked {
        let (ours, theirs) = if input { (&pair.destination, &pair.source) } else { (&pair.source, &pair.destination) };
        let seen =
            names.iter().position(|n| n == ours).is_some_and(|index| client.links(input, index).iter().any(|p| p.as_deref() == Ok(theirs.as_str())));
        if !seen {
            lines.push(link_line(pair, pipewire));
        }
    }
    lines
}

/// "Link from <source> to <destination> was not observed at the startup check.", with what
/// `jack_connect` returned when it was not 0 or 17, and the PipeWire setting that can refuse it.
pub(crate) fn link_line(pair: &Asked, pipewire: bool) -> String {
    let mut line = format!("Link from {} to {} was not observed at the startup check.", pair.source, pair.destination);
    if let Some(code) = pair.returned.as_deref().and_then(returned_code) {
        line.push_str(&format!(" jack_connect returned {code}."));
    }
    if pipewire {
        line.push_str(" A PipeWire setting such as jack.self-connect-mode can refuse links to transit's ports.");
    }
    line
}

/// The code in a failed connect's text ("jack_connect returned 1", the binding's and the fake's).
fn returned_code(text: &str) -> Option<i32> {
    text.strip_prefix("jack_connect returned ")?.split_whitespace().next()?.parse().ok()
}

/// "Graph: <rate> Hz, buffer <n> (reported by transit's active client)", or once a process cycle
/// ran with that buffer, "Graph: <rate> Hz (reported), buffer <n> (observed)".
pub(crate) fn graph_line(rate: u32, buffer: u32, observed: bool) -> String {
    if observed {
        format!("Graph: {rate} Hz (reported), buffer {buffer} (observed)")
    } else {
        format!("Graph: {rate} Hz, buffer {buffer} (reported by transit's active client)")
    }
}

/// "Latency reported at startup: output <...>, input <...>; not measured, Measure measures the
/// loop", each side read from its device ports (`playback` ports for the output, capture ports for
/// the input). `None` when transit has neither direction.
pub(crate) fn latency_line(client: &dyn JackClient, outputs: Option<&[String]>, inputs: Option<&[String]>, rate: u32) -> Option<String> {
    let mut sides = Vec::new();
    if let Some(ports) = outputs {
        sides.push(side_text("output", &read(client, ports, true), rate));
    }
    if let Some(ports) = inputs {
        sides.push(side_text("input", &read(client, ports, false), rate));
    }
    (!sides.is_empty()).then(|| format!("Latency reported at startup: {}; not measured, Measure measures the loop", sides.join(", ")))
}

fn read(client: &dyn JackClient, ports: &[String], playback: bool) -> Vec<Latency> {
    ports.iter().filter(|p| !p.is_empty()).map(|port| client.latency_range(port, playback)).collect()
}

/// One side: the envelope over its ports, with their count when the ends differ or there are
/// several; the ports not reported; "not reported" when none was.
pub(crate) fn side_text(label: &str, reads: &[Latency], rate: u32) -> String {
    let ranges: Vec<(u32, u32)> = reads.iter().filter_map(|r| if let Latency::Range(min, max) = r { Some((*min, *max)) } else { None }).collect();
    let (Some(min), Some(max)) = (ranges.iter().map(|r| r.0).min(), ranges.iter().map(|r| r.1).max()) else {
        return format!("{label} not reported");
    };
    let ms = |frames: u32| format!("{:.1}", f64::from(frames) * 1000.0 / f64::from(rate.max(1)));
    let total = reads.len();
    let mut text = if total == 1 && min == max {
        format!("{label} {min} frames ({} ms)", ms(min))
    } else {
        let ports = if ranges.len() == 1 { "1 port".to_owned() } else { format!("{} ports", ranges.len()) };
        format!("{label} {min}–{max} frames across {ports} ({}–{} ms)", ms(min), ms(max))
    };
    if ranges.len() < total {
        text.push_str(&format!(", {} of {total} ports not reported", total - ranges.len()));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_side_is_an_envelope_with_its_count_and_what_was_not_reported() {
        let r = Latency::Range;
        assert_eq!(side_text("output", &[r(256, 256)], 48_000), "output 256 frames (5.3 ms)");
        assert_eq!(side_text("output", &[r(256, 256), r(256, 256)], 48_000), "output 256–256 frames across 2 ports (5.3–5.3 ms)");
        assert_eq!(side_text("input", &[r(128, 192), r(64, 256)], 48_000), "input 64–256 frames across 2 ports (1.3–5.3 ms)");
        assert_eq!(
            side_text("input", &[r(128, 128), Latency::NoPort], 48_000),
            "input 128–128 frames across 1 port (2.7–2.7 ms), 1 of 2 ports not reported"
        );
        assert_eq!(side_text("input", &[Latency::NoPort, Latency::NoPort], 48_000), "input not reported");
        assert_eq!(side_text("output", &[Latency::NoFunction], 48_000), "output not reported", "the library lacks the call");
        assert_eq!(side_text("output", &[], 48_000), "output not reported", "no device port");
    }

    #[test]
    fn a_link_line_says_what_connect_returned_and_the_pipewire_setting() {
        let pair = |returned: Option<&str>| Asked {
            source: "transit:out_1".into(),
            destination: "system:playback_1".into(),
            returned: returned.map(str::to_owned),
        };
        assert_eq!(link_line(&pair(None), false), "Link from transit:out_1 to system:playback_1 was not observed at the startup check.");
        assert_eq!(
            link_line(&pair(Some("jack_connect returned 1")), true),
            "Link from transit:out_1 to system:playback_1 was not observed at the startup check. jack_connect returned 1. A PipeWire setting such as jack.self-connect-mode can refuse links to transit's ports."
        );
        assert_eq!(graph_line(48_000, 256, false), "Graph: 48000 Hz, buffer 256 (reported by transit's active client)");
        assert_eq!(graph_line(48_000, 256, true), "Graph: 48000 Hz (reported), buffer 256 (observed)");
    }
}
