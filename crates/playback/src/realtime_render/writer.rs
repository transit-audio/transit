//! The real-time render's writer thread (ADR 0027 decision 3): drains the tap's ring off the audio
//! thread and places each file's frames. Main's frame for session sample `start + i` is the one
//! rendered at render frame `start + i + A`; a return's is the one the plan read at
//! `start + i + C`, which is where slice 1 puts it for monitoring (ADR 0020 decisions 2 and 8).

use super::{RenderTap, TapState};
use soundcraft_model::TrackId;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Frames moved from the ring per pass.
const CHUNK: usize = 4096;
/// How long the writer waits for more frames when the ring is empty. Not a timing assertion: the
/// ring holds seconds, so the writer only has to come back well within that.
const POLL: std::time::Duration = std::time::Duration::from_millis(5);

/// One External Instrument's return in a real-time render.
#[derive(Debug, Clone, PartialEq)]
pub struct StemSpec {
    pub track: TrackId,
    /// The device input channels of its return.
    pub channels: Vec<u16>,
    /// C in plan frames (0 when the render was accepted uncompensated).
    pub latency: u32,
    /// The device's input gain, as a factor.
    pub gain: f32,
}

/// What a real-time render plays and keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeSpec {
    /// The first session sample of the range and its length.
    pub start: i64,
    pub len: u64,
    /// Main's channels to keep.
    pub main_channels: u16,
    /// Played before `start`: at least the largest C and one block.
    pub preroll: i64,
    pub stems: Vec<StemSpec>,
}

/// Where the frames go: the files of a render, or a test's memory.
pub trait RenderSink: Send {
    /// Append `frames` frames of the mix (one `Vec` per channel).
    fn main(&mut self, block: &[Vec<f32>], frames: usize) -> Result<(), String>;
    /// Append `frames` frames of stem `index` (the spec's order).
    fn stem(&mut self, index: usize, block: &[Vec<f32>], frames: usize) -> Result<(), String>;
    /// Every frame is in: complete the files.
    fn finish(&mut self) -> Result<(), String>;
    /// The render failed or was cancelled: remove what was written.
    fn abort(&mut self);
}

/// A finished real-time render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rendered {
    pub frames: u64,
    /// A, the plan's delay compensation the mix was taken behind.
    pub latency: u64,
    /// The mix's sample peak.
    pub peak: f32,
}

/// Why a real-time render wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeError {
    Cancelled,
    DeviceLost,
    /// The writer fell behind and the audio thread dropped `blocks` blocks.
    Overrun {
        blocks: u64,
    },
    Jumped,
    Stopped,
    LatencyChanged,
    MissedStart,
    /// The files could not be written.
    Sink(String),
}

impl std::fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Render cancelled. Nothing was written."),
            Self::DeviceLost => write!(f, "Render stopped: the audio device went away. Nothing was written."),
            Self::Overrun { blocks } => write!(
                f,
                "Render failed: the file writer fell behind and {blocks} block{} of audio {} dropped. Nothing was written: render again.",
                if *blocks == 1 { "" } else { "s" },
                if *blocks == 1 { "was" } else { "were" }
            ),
            Self::Jumped => write!(f, "Render failed: the playhead moved during the render. Nothing was written: render again."),
            Self::Stopped => write!(f, "Render failed: playback stopped before the end. Nothing was written: render again."),
            Self::LatencyChanged => {
                write!(f, "Render failed: the mix's delay compensation changed during the render. Nothing was written: render again.")
            }
            Self::MissedStart => write!(f, "Render failed: playback started past the range. Nothing was written: render again."),
            Self::Sink(e) => write!(f, "Render failed: {e}"),
        }
    }
}

impl std::error::Error for RealtimeError {}

/// The layout the writer places frames by.
struct Layout {
    len: u64,
    mains: usize,
    /// Per stem: its first sample in a captured frame, its width, C and gain.
    stems: Vec<(usize, usize, u64, f32)>,
}

/// Start the writer thread over `tap`, already armed for `spec`. `done` counts the mix frames
/// written; `cancel` stops it (the sink is aborted).
pub(crate) fn spawn(
    tap: Arc<RenderTap>,
    spec: &RealtimeSpec,
    sink: Box<dyn RenderSink>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
) -> std::io::Result<std::thread::JoinHandle<Result<Rendered, RealtimeError>>> {
    let mains = usize::from(spec.main_channels);
    let mut at = mains;
    let stems = spec
        .stems
        .iter()
        .map(|s| {
            let first = at;
            at += s.channels.len();
            (first, s.channels.len(), u64::from(s.latency), s.gain)
        })
        .collect();
    let layout = Layout { len: spec.len, mains, stems };
    std::thread::Builder::new().name("transit-render-writer".into()).spawn(move || run(&tap, &layout, sink, &cancel, &done))
}

fn run(tap: &RenderTap, layout: &Layout, mut sink: Box<dyn RenderSink>, cancel: &AtomicBool, done: &AtomicU64) -> Result<Rendered, RealtimeError> {
    let result = write_all(tap, layout, sink.as_mut(), cancel, done);
    if result.is_err() {
        sink.abort();
    }
    result
}

fn write_all(tap: &RenderTap, layout: &Layout, sink: &mut dyn RenderSink, cancel: &AtomicBool, done: &AtomicU64) -> Result<Rendered, RealtimeError> {
    let stride = layout.mains + layout.stems.iter().map(|s| s.1).sum::<usize>();
    let mut frames = Vec::with_capacity(CHUNK * stride);
    let mut main: Vec<Vec<f32>> = vec![Vec::with_capacity(CHUNK); layout.mains];
    let mut stems: Vec<Vec<Vec<f32>>> = layout.stems.iter().map(|s| vec![Vec::with_capacity(CHUNK); s.1]).collect();
    // `k`: render frames taken since the start; `main_done` and `stem_done`: frames written.
    let (mut k, mut main_done, mut peak) = (0u64, 0u64, 0f32);
    let mut stem_done = vec![0u64; layout.stems.len()];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(RealtimeError::Cancelled);
        }
        // Read the state before draining: once Captured, the drain below sees every frame.
        let state = tap.state();
        let got = tap.drain(&mut frames, CHUNK);
        if got > 0 {
            let a = tap.latency();
            main.iter_mut().for_each(Vec::clear);
            stems.iter_mut().flatten().for_each(Vec::clear);
            for (i, frame) in frames.chunks_exact(stride).enumerate() {
                let r = k + i as u64;
                if r >= a && r - a < layout.len {
                    for (c, ch) in main.iter_mut().enumerate() {
                        peak = peak.max(frame[c].abs());
                        ch.push(frame[c]);
                    }
                }
                for (j, &(first, width, c, gain)) in layout.stems.iter().enumerate() {
                    if r >= c && r - c < layout.len {
                        for (n, ch) in stems[j].iter_mut().enumerate().take(width) {
                            ch.push(frame[first + n] * gain);
                        }
                    }
                }
            }
            k += got as u64;
            let n = main.first().map_or(0, Vec::len);
            if n > 0 {
                sink.main(&main, n).map_err(RealtimeError::Sink)?;
                main_done += n as u64;
                done.store(main_done, Ordering::Relaxed);
            }
            for (j, block) in stems.iter().enumerate() {
                let n = block.first().map_or(0, Vec::len);
                if n > 0 {
                    sink.stem(j, block, n).map_err(RealtimeError::Sink)?;
                    stem_done[j] += n as u64;
                }
            }
            continue;
        }
        match state {
            TapState::Captured if main_done == layout.len && stem_done.iter().all(|d| *d == layout.len) => {
                sink.finish().map_err(RealtimeError::Sink)?;
                return Ok(Rendered { frames: main_done, latency: tap.latency(), peak });
            }
            TapState::Captured => return Err(RealtimeError::Sink("the capture ended short of the range".into())),
            TapState::Failed(why) => {
                return Err(match why {
                    super::Failure::Overrun => RealtimeError::Overrun { blocks: tap.overruns().max(1) },
                    super::Failure::Jumped => RealtimeError::Jumped,
                    super::Failure::Stopped => RealtimeError::Stopped,
                    super::Failure::LatencyChanged => RealtimeError::LatencyChanged,
                    super::Failure::MissedStart => RealtimeError::MissedStart,
                });
            }
            TapState::Idle => return Err(RealtimeError::Cancelled),
            TapState::Armed | TapState::Capturing => std::thread::park_timeout(POLL),
        }
    }
}

/// One file of a render: written to `<name>.part`, renamed over `path` when complete.
struct Part {
    enc: Option<soundcraft_audio_io::FileEncoder<std::io::BufWriter<std::fs::File>>>,
    part: PathBuf,
    path: PathBuf,
}

impl Part {
    fn create(
        path: &Path,
        channels: usize,
        opts: &soundcraft_audio_io::EncodeOptions,
        rate: u32,
        mask: Option<u32>,
        frames: u64,
    ) -> Result<Self, String> {
        let name = path.file_name().map_or_else(|| "render".into(), |n| n.to_string_lossy().into_owned());
        let part = path.with_file_name(format!("{name}.part"));
        let enc = (|| -> Result<_, String> {
            let file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
            let enc = soundcraft_audio_io::FileEncoder::new(std::io::BufWriter::with_capacity(1 << 20, file), opts, channels, rate, mask)
                .map_err(|e| e.to_string())?;
            enc.check_room(frames).map_err(|e| e.to_string())?;
            Ok(enc)
        })()
        .map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("{}: {e}", path.display())
        })?;
        Ok(Self { enc: Some(enc), part, path: path.to_path_buf() })
    }

    fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<(), String> {
        match &mut self.enc {
            Some(enc) => enc.write(block, frames).map_err(|e| format!("{}: {e}", self.path.display())),
            None => Ok(()),
        }
    }

    fn finish(&mut self) -> Result<(), String> {
        let Some(enc) = self.enc.take() else { return Ok(()) };
        let what = |e: &dyn std::fmt::Display| format!("{}: {e}", self.path.display());
        let mut out = enc.finish().map_err(|e| what(&e))?;
        out.flush().map_err(|e| what(&e))?;
        let file = out.into_inner().map_err(|e| what(&e.into_error()))?;
        file.sync_all().map_err(|e| what(&e))?;
        drop(file);
        std::fs::rename(&self.part, &self.path).map_err(|e| what(&e))
    }

    fn abort(&mut self) {
        drop(self.enc.take());
        let _ = std::fs::remove_file(&self.part);
    }
}

impl Drop for Part {
    /// Removes the part file unless the file completed (which renamed it).
    fn drop(&mut self) {
        self.abort();
    }
}

/// The files of a real-time render: the mix, and one stem per External Instrument when asked.
pub struct FileSink {
    main: Part,
    stems: Vec<Option<Part>>,
}

impl FileSink {
    /// Part files for the mix at `main` (`channels` wide, WAV speaker `mask`) and for each stem
    /// path given (`None`: that return is not kept as a file), each with room for `frames`.
    pub fn create(
        main: &Path,
        channels: usize,
        mask: Option<u32>,
        stems: &[Option<(PathBuf, usize)>],
        opts: &soundcraft_audio_io::EncodeOptions,
        rate: u32,
        frames: u64,
    ) -> Result<Self, String> {
        let main = Part::create(main, channels, opts, rate, mask, frames)?;
        let stems =
            stems.iter().map(|s| s.as_ref().map(|(p, ch)| Part::create(p, *ch, opts, rate, None, frames)).transpose()).collect::<Result<_, _>>()?;
        Ok(Self { main, stems })
    }
}

impl RenderSink for FileSink {
    fn main(&mut self, block: &[Vec<f32>], frames: usize) -> Result<(), String> {
        self.main.write(block, frames)
    }

    fn stem(&mut self, index: usize, block: &[Vec<f32>], frames: usize) -> Result<(), String> {
        match self.stems.get_mut(index) {
            Some(Some(part)) => part.write(block, frames),
            _ => Ok(()),
        }
    }

    fn finish(&mut self) -> Result<(), String> {
        self.main.finish()?;
        self.stems.iter_mut().flatten().try_for_each(Part::finish)
    }

    fn abort(&mut self) {
        self.main.abort();
        self.stems.iter_mut().flatten().for_each(Part::abort);
    }
}
