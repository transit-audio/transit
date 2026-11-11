//! The bench's waveform from the source's existing peaks (the pyramid built when the audio was
//! admitted, or a disk-backed source's peaks once its reader has built them, ADR 0035). Nothing is
//! decoded here: a source whose audio or peaks are not there yet draws nothing and reads
//! `Analyzing…`, and the next frame after they arrive draws them.

use transit_model::SourceAudio;

/// One column's (min, max) over every channel (overlaid, never summed), `None` outside the data.
pub type Span = Option<(f32, f32)>;

/// `cols` columns over the frames `from..to` of `audio`, or `None` while its peaks are not there.
pub fn columns(audio: Option<&SourceAudio>, from: f64, to: f64, cols: usize) -> Option<Vec<Span>> {
    let audio = audio?;
    let step = (to - from) / cols.max(1) as f64;
    let merge = |acc: &mut Vec<Span>, spans: Vec<Span>| {
        for (a, s) in acc.iter_mut().zip(spans) {
            *a = match (*a, s) {
                (Some((lo, hi)), Some((l, h))) => Some((lo.min(l), hi.max(h))),
                (x, None) => x,
                (None, y) => y,
            };
        }
    };
    let mut out: Vec<Span> = vec![None; cols];
    if let Some(disk) = &audio.disk {
        let peaks = disk.peaks()?;
        for ch in 0..peaks.channels.len() {
            let spans = (0..cols).map(|c| peaks.minmax(ch, from + step * c as f64, from + step * (c + 1) as f64)).collect();
            merge(&mut out, spans);
        }
        return Some(out);
    }
    if audio.peaks.is_empty() {
        return None;
    }
    for (ch, peaks) in audio.peaks.iter().enumerate() {
        let Some(samples) = audio.channel(ch) else { continue };
        merge(&mut out, peaks.spans(samples, from, step, 0.0, cols));
    }
    Some(out)
}
