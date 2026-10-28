//! Import, export, save/load, rendering helpers (bounce, consolidate, commit) and analysis.

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use transit_audio_io::{AudioBuffer, EncodeOptions, FileFormat};
use transit_model::{BusId, ChannelFormat, Clip, ClipContent, Session, Source, SourceAudio, SourceId, TrackId, TrackKind};
use transit_time::{Range, Samples};

/// Largest import we accept (samples per channel × channels), ~4 GiB of f32.
pub(crate) const MAX_IMPORT_SAMPLES: usize = 1 << 30;

/// `buf` converted to `rate`, unless it would then hold more than `limit` samples: a file with a
/// tiny sample rate grows by orders of magnitude.
pub(crate) fn at_rate(mut buf: AudioBuffer, rate: u32, limit: usize) -> Option<AudioBuffer> {
    let from = buf.sample_rate.max(1);
    if buf.frames() as f64 * buf.num_channels() as f64 * f64::from(rate) / f64::from(from) > limit as f64 {
        return None;
    }
    if from != rate {
        buf.channels = transit_dsp::offline::resample(&buf.channels, from, rate);
        buf.sample_rate = rate;
    }
    Some(buf)
}

// ─── Checked file access ──────────────────────────────────────────────────────────────────────
//
// Every file a command names by path is read and written through the helpers below, so the
// automation roots (`crate::file_access`, `transit-cli mcp --automation-read-root` and
// `--automation-write-root`) are checked in one place. A test in `file_access` keeps the rest of
// the engine from calling `std::fs` itself. Audio is decoded into the session's pool the moment it
// enters it (import, open, sampler load), so playback and bounce read nothing from disk and
// need no check of their own.

/// The bytes of the file at `path`, which must be inside the automation read root when there is
/// one. Only a regular file qualifies: a device or a pipe named by a session or a command would
/// never end or never answer. Errors name the file.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let err = |err: std::io::Error| EngineError::Io(format!("{}: {err}", path.display()));
    if !std::fs::metadata(path).map_err(err)?.is_file() {
        return Err(EngineError::Io(format!("{}: not a regular file", path.display())));
    }
    std::fs::read(path).map_err(err)
}

/// [`read_file`] in 1 MB pieces, calling `progress(read, total)` after each; when it returns
/// `false` the read stops with an error (a canceled import).
pub fn read_file_with_progress(path: impl AsRef<Path>, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<Vec<u8>> {
    use std::io::Read;
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let err = |err: std::io::Error| EngineError::Io(format!("{}: {err}", path.display()));
    let meta = std::fs::metadata(path).map_err(err)?;
    if !meta.is_file() {
        return Err(EngineError::Io(format!("{}: not a regular file", path.display())));
    }
    let total = meta.len();
    let mut file = std::fs::File::open(path).map_err(err)?;
    let mut out = Vec::with_capacity(usize::try_from(total).unwrap_or(0).min(MAX_IMPORT_SAMPLES));
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut chunk).map_err(err)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n]);
        if !progress(out.len() as u64, total) {
            return Err(EngineError::Io(format!("{}: canceled", path.display())));
        }
    }
    Ok(out)
}

/// [`read_file`] as UTF-8 text.
pub fn read_text(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    String::from_utf8(read_file(path)?).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// The file a path stored in the session file `session` refers to, or why it is not opened.
///
/// Every path a session names (audio sources) goes through here before it reaches the
/// file system. A relative path is taken from the session's folder, and only files on this
/// computer or on the network share the session itself is on are opened (`craft_docpath`).
/// Paths the person picks in this session (imports) are not stored paths and skip this.
pub fn stored_path(stored: &str, session: &Path) -> std::result::Result<PathBuf, String> {
    let session = std::path::absolute(session).unwrap_or_else(|_| session.to_path_buf());
    let path = craft_docpath::resolve(stored, Some(&session)).map_err(|why| format!("not opened: {why}"))?;
    // A path that is still relative would be taken from the working directory, which the session
    // does not choose.
    if path.is_relative() {
        return Err("not opened: the path is not absolute".into());
    }
    Ok(path)
}

/// Create `dir` and its parents, inside the automation write root when there is one.
pub(crate) fn make_dir(dir: impl AsRef<Path>) -> Result<()> {
    let dir = dir.as_ref();
    crate::file_access::check_write(dir).map_err(EngineError::Io)?;
    std::fs::create_dir_all(dir).map_err(|err| EngineError::Io(format!("{}: {err}", dir.display())))
}

/// Write `bytes` to the file at `path`, inside the automation write root when there is one.
pub(crate) fn write_path(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// A relative `Audio Files/` path no other source in `s` already uses.
fn unique_audio_rel(s: &Session, stem: &str) -> String {
    let stem = sanitize_name(stem);
    let (plain, prefix) = (format!("Audio Files/{stem}.wav"), format!("Audio Files/{stem}-"));
    let used: std::collections::HashSet<u64> = s
        .sources
        .iter()
        .filter_map(|src| {
            if src.path == plain {
                return Some(1);
            }
            let n = src.path.strip_prefix(&prefix)?.strip_suffix(".wav")?;
            n.parse().ok().filter(|i: &u64| *i > 1 && i.to_string() == n)
        })
        .collect();
    match (1..).find(|i| !used.contains(i)) {
        Some(n) if n > 1 => format!("{prefix}{n}.wav"),
        _ => plain,
    }
}

/// Register decoded audio as a new source. Resamples to the session rate if needed.
pub fn add_source(s: &mut Session, name: &str, mut buf: AudioBuffer, path: Option<&str>, format: FileFormat) -> SourceId {
    let sr = s.sample_rate.hz();
    if buf.sample_rate != sr && buf.sample_rate > 0 {
        buf.channels = transit_dsp::offline::resample(&buf.channels, buf.sample_rate, sr);
        buf.sample_rate = sr;
    }
    add_source_audio(s, name, Arc::new(SourceAudio::new(buf)), path, format)
}

/// Register audio already at the session rate, with its peaks built (see [`prepare_audio_bytes`]).
pub fn add_source_audio(s: &mut Session, name: &str, audio: Arc<SourceAudio>, path: Option<&str>, format: FileFormat) -> SourceId {
    let sr = s.sample_rate.hz();
    let buf = &audio.buffer;
    let id = SourceId(s.alloc());
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or(name).to_string();
    let stored = match path {
        Some(p) => p.to_string(),
        None => unique_audio_rel(s, &stem),
    };
    s.sources.push(Source {
        id,
        name: stem.clone(),
        path: stored,
        channels: u16::try_from(buf.num_channels()).unwrap_or(u16::MAX),
        frames: buf.frames() as u64,
        sample_rate: sr,
        format,
        time_reference: 0,
        unsaved: true,
    });
    s.pool.insert(id, audio);
    id
}

/// A BWF time stamp (samples at the file's rate) at the session's rate, as the audio is resampled.
fn stamp_at_rate(stamp: u64, file_rate: u32, session_rate: u32) -> u64 {
    if file_rate == 0 || file_rate == session_rate {
        return stamp;
    }
    let (from, to) = (u128::from(file_rate), u128::from(session_rate));
    u64::try_from((u128::from(stamp) * to + from / 2) / from).unwrap_or(u64::MAX)
}

/// An audio file decoded, resampled to the session rate and with its peaks built, ready to place:
/// the slow part of an import, done by [`prepare_audio_bytes`] on any thread.
pub struct PreparedAudio {
    /// The file name, `Kick 01.wav`.
    pub name: String,
    pub path: Option<String>,
    pub audio: Arc<SourceAudio>,
    pub format: FileFormat,
    /// The file's own rate, and its BWF time stamp at that rate.
    pub file_rate: u32,
    pub time_reference: Option<u64>,
}

impl PreparedAudio {
    pub fn frames(&self) -> usize {
        self.audio.frames()
    }
    pub fn channels(&self) -> usize {
        self.audio.buffer.num_channels()
    }
    pub fn rate(&self) -> u32 {
        self.audio.buffer.sample_rate
    }
}

/// Decode `bytes` and resample them to `rate`, building the waveform peaks: everything an import
/// does but touch the session. `progress(fraction)` (0 to 1 for this step) runs between stages
/// and after each resampled channel; when it returns `false` the work stops with an error.
pub fn prepare_audio_bytes(name: &str, bytes: &[u8], path: Option<&str>, rate: u32, progress: &mut dyn FnMut(f32) -> bool) -> Result<PreparedAudio> {
    let canceled = || EngineError::Io(format!("{name}: canceled"));
    let too_large = || crate::cmd::bad("file.import_audio", format!("{name}: file is too large to import"));
    let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (info, mut buf) = transit_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    if !progress(0.4) {
        return Err(canceled());
    }
    let from = buf.sample_rate.max(1);
    let n = buf.num_channels();
    // The same limit as `at_rate`: a file with a tiny sample rate grows by orders of magnitude.
    if buf.frames() as f64 * n as f64 * f64::from(rate) / f64::from(from) > MAX_IMPORT_SAMPLES as f64 {
        return Err(too_large());
    }
    if from != rate {
        // One channel at a time (as `at_rate` does), so the work can report and stop in between.
        let mut out = Vec::with_capacity(n);
        for (i, ch) in std::mem::take(&mut buf.channels).into_iter().enumerate() {
            out.extend(transit_dsp::offline::resample(&[ch], from, rate));
            if !progress(0.4 + 0.45 * (i + 1) as f32 / n as f32) {
                return Err(canceled());
            }
        }
        buf.channels = out;
        buf.sample_rate = rate;
    }
    if buf.frames() == 0 {
        return Err(crate::cmd::bad("file.import_audio", format!("{name}: no audio")));
    }
    let audio = Arc::new(SourceAudio::new(buf));
    if !progress(1.0) {
        return Err(canceled());
    }
    Ok(PreparedAudio {
        name: name.to_string(),
        path: path.map(str::to_string),
        audio,
        format: info.format,
        file_rate: info.sample_rate,
        time_reference: info.bwf.as_ref().map(|b| b.time_reference),
    })
}

/// Place prepared audio: a new source, and a clip at `at` on `target` (or on a new track of
/// matching width). `name` replaces the file name as the source, clip and new track name. Audio
/// prepared at another rate than the session's is resampled here.
pub fn place_prepared(e: &mut Engine, prepared: PreparedAudio, target: Option<TrackId>, at: Samples, name: Option<&str>) -> Result<Value> {
    let PreparedAudio { name: file, path, audio, format, file_rate, time_reference } = prepared;
    let label = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&file).to_string();
    let channels = audio.buffer.num_channels();
    let s = e.session_mut();
    let session_rate = s.sample_rate.hz();
    let src = if audio.buffer.sample_rate == session_rate {
        add_source_audio(s, &label, audio, path.as_deref(), format)
    } else {
        let buf = Arc::try_unwrap(audio).map_or_else(|a| a.buffer.clone(), |a| a.buffer);
        add_source(s, &label, buf, path.as_deref(), format)
    };
    if let (Some(stamp), Some(sx)) = (time_reference, s.sources.last_mut()) {
        sx.time_reference = stamp_at_rate(stamp, file_rate, session_rate);
    }
    let frames = s.source(src).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
    let stem = s.source(src).map(|x| x.name.clone()).unwrap_or_default();
    let track = match target {
        Some(t) => t,
        None => s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), Some(if stem.is_empty() { "Audio" } else { &stem })),
    };
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, track, Clip::audio(cid, stem, src, 0, at, frames));
    Ok(json!({"track": track, "clip": cid, "source": src, "frames": frames, "channels": channels}))
}

/// Decode and place an audio file. With no target track, a new track of matching width is made.
/// The synchronous import: [`prepare_audio_bytes`], then [`place_prepared`].
pub fn import_audio_bytes(e: &mut Engine, name: &str, bytes: &[u8], path: Option<&str>, target: Option<TrackId>, at: Samples) -> Result<Value> {
    let prepared = prepare_audio_bytes(name, bytes, path, e.session().sample_rate.hz(), &mut |_| true)?;
    place_prepared(e, prepared, target, at, None)
}

/// Loads an audio file's bytes into an instrument track's Sampler as one undo step ("Load
/// Sampler Sample"): for files that have no path, such as one dropped onto the window. The same
/// checks as `mix.sampler_load`; not journaled, since the bytes can be large.
pub fn load_sampler_bytes(e: &mut Engine, track: TrackId, name: &str, bytes: &[u8]) -> Result<Value> {
    let before = e.session_arc();
    let r = crate::cmd::sampler::attach(e, track, name, bytes, None, Vec::new())?;
    e.push_undo("Load Sampler Sample", before);
    Ok(r)
}

mod midi;
pub use midi::{export_midi, import_midi_bytes};

mod session_data;
pub(crate) use session_data::import_session_data;

mod paths;
pub use paths::{PathBounce, PathBounced, PathNormalize, PathStem, bounce_paths, is_routed, routed_paths};

mod bounce;
pub use bounce::{Bounce, BounceOptions, Bounced, bounce_stems, bounce_stems_with};

/// Hosted plug-ins' user preset files (issue #68), through the checked file access.
pub mod presets;
pub(crate) use bounce::{bounce_layout, check_bounce_range};

/// Where a bounce is written: its part file.
enum Out {
    File(std::io::BufWriter<std::fs::File>),
}

impl Out {
    fn create(part: &Path) -> std::io::Result<Out> {
        Ok(Out::File(std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(part)?)))
    }

    /// Puts the finished file at `path` and returns its size: the part file is synced and renamed
    /// over it.
    fn complete(self, part: &Path, path: &Path) -> Result<u64> {
        match self {
            Out::File(out) => {
                let mut len = 0;
                let synced = out.into_inner().map_err(std::io::IntoInnerError::into_error).and_then(|f| {
                    f.sync_all()?;
                    len = f.metadata()?.len();
                    Ok(())
                });
                replace_with(part, path, synced).map(|()| len)
            }
        }
    }
}

impl std::io::Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File(f) => f.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(f) => f.flush(),
        }
    }
}

impl std::io::Seek for Out {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(f) => f.seek(pos),
        }
    }
}

/// Limit a range used for rendering to the track's material (plus a second), so a huge
/// selection never renders hours of silence.
pub fn bounded_to_track(s: &Session, t: TrackId, r: Range) -> Range {
    let Some(tr) = s.track(t) else { return Range::point(r.start) };
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0).saturating_add(s.sample_rate.samples(1.0));
    let start = r.start.max(0).min(end);
    Range::new(start, r.end.min(end).max(start))
}

/// Transient positions (absolute) in a track's clips within `r`.
pub fn transients_in(s: &Session, t: TrackId, r: Range, sens: f32) -> Vec<Samples> {
    let r = if r.is_empty() { Range::new(0, s.content_end()) } else { r };
    let r = bounded_to_track(s, t, r);
    let _offline = offline();
    let audio = transit_mix::render_clips(s, t, r);
    transit_dsp::offline::detect_transients(&audio, s.sample_rate.as_f64() as f32, sens).into_iter().map(|i| r.start + i as i64).collect()
}

/// Strip Silence: keep only the non-silent ranges of clips inside `r`.
pub fn strip_silence(s: &mut Session, t: TrackId, r: Range, thr: f32, min_len: Samples, pre: Samples, post: Samples) -> Result<usize> {
    let r = bounded_to_track(s, t, r);
    let keep = non_silent(s, t, r, SilenceOptions { threshold: thr, min_len, pre, post })?;
    let mut cut_points = vec![r.start];
    for (a, b) in &keep {
        cut_points.push(*a);
        cut_points.push(*b);
    }
    cut_points.push(r.end);
    for p in &cut_points {
        crate::edit::separate_at(s, t, *p);
    }
    // Remove pieces that fall in the gaps.
    let mut gaps = Vec::new();
    let mut prev = r.start;
    for (a, b) in &keep {
        if *a > prev {
            gaps.push(Range::new(prev, *a));
        }
        prev = *b;
    }
    if prev < r.end {
        gaps.push(Range::new(prev, r.end));
    }
    if let Some(pl) = s.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips.retain(|c| !gaps.iter().any(|g| c.start >= g.start && c.end() <= g.end));
    }
    Ok(keep.len())
}

/// Most samples (over all channels) Strip Silence renders at once.
const STRIP_WINDOW_SAMPLES: usize = 1 << 24;

struct SilenceOptions {
    threshold: f32,
    min_len: Samples,
    pre: Samples,
    post: Samples,
}

/// The non-silent ranges of `t`'s clips in `r`. The audio is rendered a window at a time where
/// audio clips play, so all of `r` is scanned however long it is (one render stops at
/// `MAX_RENDER_SAMPLES`); at most a day of clips, Pro Tools' longest session.
fn non_silent(s: &Session, t: TrackId, r: Range, options: SilenceOptions) -> Result<Vec<(Samples, Samples)>> {
    let SilenceOptions { threshold: thr, min_len, pre, post } = options;
    let Some(tr) = s.track(t) else { return Ok(Vec::new()) };
    let mut clips: Vec<Range> = tr.clips().iter().filter(|c| c.is_audio() && !c.muted).filter_map(|c| c.range().intersect(&r)).collect();
    clips.sort_by_key(|x| x.start);
    let mut spans: Vec<Range> = Vec::new();
    for x in clips {
        match spans.last_mut() {
            Some(last) if x.start <= last.end => last.end = last.end.max(x.end),
            _ => spans.push(x),
        }
    }
    if spans.iter().map(Range::len).fold(0, i64::saturating_add) > s.sample_rate.samples(86_400.0) {
        return Err(EngineError::BadParams("edit.strip_silence".into(), format!("`{}` has more than a day of clips to scan", tr.name)));
    }
    let window = i64::try_from(STRIP_WINDOW_SAMPLES / tr.channels().max(1)).unwrap_or(i64::MAX);
    let len = |n: Samples| u64::try_from(n).unwrap_or(0);
    let mut scan = transit_dsp::offline::SilenceScan::new(thr, len(min_len));
    let mut at = r.start;
    for x in spans {
        scan.skip(len(x.start - at));
        at = x.start;
        while at < x.end {
            let end = at.saturating_add(window).min(x.end);
            let _offline = offline();
            scan.push(&transit_mix::render_clips(s, t, Range::new(at, end)));
            at = end;
        }
    }
    scan.skip(len(r.end - at));
    Ok(scan.finish(len(pre), len(post)).into_iter().map(|(a, b)| (r.start.saturating_add_unsigned(a), r.start.saturating_add_unsigned(b))).collect())
}

/// Consolidate `r` on a track into one new clip backed by a rendered source. The clip covers the
/// whole range, silence included; a range too long to render is refused rather than shortened.
pub fn consolidate(e: &mut Engine, cmd: &str, t: TrackId, r: Range) -> Result<bool> {
    let s = e.session();
    let Some(tr) = s.track(t).filter(|tr| tr.kind.has_playlist() && !tr.kind.is_midi()) else { return Ok(false) };
    let start = r.start.max(0);
    let len = r.end.saturating_sub(start);
    if len <= 0 {
        return Ok(false);
    }
    let max = transit_mix::MAX_RENDER_SAMPLES / tr.channels().max(1);
    if usize::try_from(len).map_or(true, |n| n > max) {
        let secs = max as f64 / s.sample_rate.as_f64();
        return Err(crate::cmd::bad(cmd, format!("selection is too long to consolidate on `{}` (at most {secs:.0} s)", tr.name)));
    }
    let r = Range::new(start, start + len);
    let _offline = offline();
    let audio = transit_mix::render_clips(s, t, r);
    if audio.iter().any(|ch| ch.len() as i64 != len) {
        return Err(EngineError::Internal(cmd.to_string(), "consolidate rendered the wrong length".into()));
    }
    let name = format!("{}_consolidated", tr.name);
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    crate::edit::clear_range(s, t, r, false);
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(cid, name, src, 0, r.start, r.len()));
    Ok(true)
}

/// The most frames of `channels`-channel audio one offline render holds.
pub fn render_limit(channels: usize) -> usize {
    transit_mix::MAX_RENDER_SAMPLES / channels.max(1)
}

/// Refuses `len` frames of `name` past `max`, where a render would cut the audio short.
/// Offline mode for hosted plugins while the guard lives: every plugin made for an offline render
/// samples its parameter curves at creation (see `transit_vst3_host::offline_scope`).
pub(crate) fn offline() -> transit_vst3_host::OfflineScope {
    transit_vst3_host::offline_scope()
}

pub fn refuse_longer(s: &Session, cmd: &str, name: &str, verb: &str, len: Samples, max: usize) -> Result<()> {
    if len <= Samples::try_from(max).unwrap_or(Samples::MAX) {
        return Ok(());
    }
    let secs = max as f64 / s.sample_rate.as_f64();
    Err(crate::cmd::bad(cmd, format!("`{name}` is too long to {verb} (at most {secs:.0} s)")))
}

/// Render clip gain into a new source and reset the clip's gain.
pub fn render_clip_gain(e: &mut Engine, cmd: &str, id: transit_model::ClipId) -> Result<bool> {
    let s = e.session();
    let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Ok(false) };
    if !c.is_audio() {
        return Ok(false);
    }
    refuse_longer(s, cmd, &c.name, "render", c.length, render_limit(s.track(t).map_or(1, |x| x.channels())))?;
    let mut c2 = c.clone();
    c2.fade_in = Default::default();
    c2.fade_out = Default::default();
    let mut tmp = s.clone();
    if let Some(pl) = tmp.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips = vec![c2];
    }
    let _offline = offline();
    let audio = transit_mix::render_clips(&tmp, t, c.range());
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &format!("{}_gain", c.name), buf, None, FileFormat::Wav);
    if let Some(cl) = s.find_clip_mut(id) {
        cl.content = ClipContent::Audio { source: src, offset: 0 };
        // The rendered file holds every cycle (ADR 0019).
        cl.looping = None;
        cl.gain_db = 0.0;
        cl.gain_env.clear();
        // The new source is the clip as it already sounds, stretch included.
        // Keeping the old ratio would play that audio through the stretch again.
        cl.stretch = 1.0;
    }
    Ok(true)
}

/// Commit a track's inserts into a new audio track (Track › Commit).
pub fn commit_track(e: &mut Engine, t: TrackId) -> Result<Option<TrackId>> {
    let s = e.session();
    let Some(tr) = s.track(t) else { return Ok(None) };
    if !tr.kind.is_audio_path() || tr.kind == TrackKind::Master {
        return Ok(None);
    }
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0);
    if end <= 0 {
        return Ok(None);
    }
    let range = Range::new(0, end + s.sample_rate.samples(2.0));
    // The track renders through a main mix of at least two channels.
    refuse_longer(s, "track.commit", &tr.name, "commit", range.len(), render_limit(tr.channels().max(2)))?;
    let _offline = offline();
    let audio = transit_mix::render_track_pre_fader(s, t, range);
    let name = format!("{}.cm", tr.name);
    let fmt = ChannelFormat::for_channels(audio.len().clamp(1, 2));
    let mixer = tr.mixer.clone();
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    let nt = s.add_track(TrackKind::Audio, fmt, Some(&name));
    if let (Some(idx), Some(new_idx)) = (s.track_index(t), s.track_index(nt)) {
        let tr = s.tracks.remove(new_idx);
        s.tracks.insert((idx + 1).min(s.tracks.len()), tr);
    }
    let cid = s.new_clip_id();
    if let Some(ntr) = s.track_mut(nt) {
        ntr.mixer.volume_db = mixer.volume_db;
        ntr.mixer.output = mixer.output.clone();
        ntr.mixer.sends = mixer.sends.clone();
        if let Some(pl) = ntr.playlist_mut() {
            pl.clips.push(Clip::audio(cid, name, src, 0, 0, range.len()));
        }
    }
    if let Some(old) = s.track_mut(t) {
        old.inactive = true;
        old.hidden = true;
    }
    Ok(Some(nt))
}

pub fn export_clips(e: &Engine, ids: &[transit_model::ClipId], dir: &str) -> Result<usize> {
    let s = e.session();
    make_dir(dir)?;
    let mut n = 0;
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let _offline = offline();
        let audio = transit_mix::render_clips(s, t, c.range());
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
        let bytes = transit_audio_io::encode(&buf, &EncodeOptions::default()).map_err(|err| EngineError::Io(err.to_string()))?;
        let stem = sanitize_name(&c.name);
        let mut path = Path::new(dir).join(format!("{stem}.wav"));
        let mut k = 2u32;
        while path.exists() {
            path = Path::new(dir).join(format!("{stem}-{k}.wav"));
            k = k.saturating_add(1);
            if k > 10_000 {
                return Err(EngineError::Io(format!("{dir}: too many files named {stem}")));
            }
        }
        write_path(&path, bytes)?;
        n += 1;
    }
    Ok(n)
}

pub fn sanitize_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "untitled".into() } else { s.chars().take(100).collect() }
}

mod session;
use session::replace_with;
pub use session::{
    BUNDLE_EXTENSION, open_session, open_session_bytes, replace_file, replaces, save_copy, save_session, save_session_copy, session_bundle,
    session_file, session_open_line, write_file,
};
pub(crate) use session::{DecodedFiles, decode_file, file_key, open_session_linked};

mod recording;
pub use recording::{Recording, Take, add_recording, add_recording_take, live_span, place_recording, place_recording_take, plan_takes};

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn a_tiny_sample_rate_is_refused_before_it_is_converted() {
        let buf = |rate| AudioBuffer { sample_rate: rate, channels: vec![vec![0.5; 100]; 2] };
        assert!(at_rate(buf(1), 48_000, 1_000_000).is_none());
        let ok = at_rate(buf(8_000), 48_000, 1_000_000).unwrap();
        assert_eq!((ok.sample_rate, ok.frames()), (48_000, 600));
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn a_midi_track_name_is_cut_to_a_length_that_can_be_drawn() {
        let note = transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = transit_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = transit_midi::Smf { tracks: vec![transit_midi::SmfTrack { name: "n".repeat(1_000_000), sequence }], ..Default::default() };
        let mut e = Engine::default();
        let ids = import_midi_bytes(&mut e, &transit_midi::write_smf(&smf).unwrap(), 0, false).unwrap();
        let track = e.session().track(ids[0]).unwrap();
        assert_eq!((track.name.len(), track.clips()[0].name.len()), (transit_model::MAX_NAME_CHARS, transit_model::MAX_NAME_CHARS));
    }

    #[test]
    fn midi_longer_than_a_day_is_refused() {
        let held = transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: (1 << 28) - 1 };
        let sequence = transit_midi::Sequence { notes: vec![held], ..Default::default() };
        let smf = transit_midi::Smf { tracks: vec![transit_midi::SmfTrack { name: String::new(), sequence }], ..Default::default() };
        let err = import_midi_bytes(&mut Engine::default(), &transit_midi::write_smf(&smf).unwrap(), 0, false).unwrap_err();
        assert!(err.to_string().contains("longer than 24 hours"), "{err}");
    }

    #[test]
    fn midi_with_more_meter_changes_than_scores_have_is_refused() {
        let note = transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = transit_midi::Sequence { notes: vec![note], ..Default::default() };
        let meters = (0..=midi::MAX_MIDI_METERS as i64).map(|bar| (bar * 3840, 4, 4)).collect();
        let smf = transit_midi::Smf { tracks: vec![transit_midi::SmfTrack { name: String::new(), sequence }], meters, ..Default::default() };
        let err = import_midi_bytes(&mut Engine::default(), &transit_midi::write_smf(&smf).unwrap(), 0, true).unwrap_err();
        assert!(err.to_string().contains("meter changes"), "{err}");
        assert!(matches!(&err, EngineError::BadParams(command, _) if command == "file.import_midi"));
    }

    #[test]
    fn new_sources_take_the_first_free_file_name() {
        let mut s = Session::default();
        let buf = || AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 4]] };
        add_source(&mut s, "k", buf(), Some("Audio Files/k-02.wav"), FileFormat::Wav);
        let mut path = || {
            let id = add_source(&mut s, "k", buf(), None, FileFormat::Wav);
            s.source(id).map(|x| x.path.clone()).unwrap_or_default()
        };
        assert_eq!([path(), path(), path()], ["Audio Files/k.wav", "Audio Files/k-2.wav", "Audio Files/k-3.wav"]);
    }
}

#[cfg(all(test, unix))]
mod pipe_tests {
    use super::*;

    #[test]
    fn a_pipe_named_as_media_is_refused_not_waited_on() {
        let dir = std::env::temp_dir().join(format!("soundcraft-pipe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe.wav");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let mut s = Session::default();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 10]] };
        add_source(&mut s, "pipe", audio, Some(&fifo.to_string_lossy()), FileFormat::Wav);
        let path = dir.join("Song.transit").to_string_lossy().into_owned();
        std::fs::write(&path, s.to_json().unwrap()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let pipe = fifo.to_string_lossy().into_owned();
        std::thread::spawn(move || {
            let mut e = Engine::default();
            let imported = e.execute("file.import_audio", &json!({"path": pipe})).is_ok();
            let _ = tx.send((imported, open_session(&mut e, &path).map_err(|err| err.to_string())));
        });
        let (imported, opened) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(!imported);
        assert_eq!(opened.unwrap(), [fifo.display().to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod stem_tests {
    #[test]
    fn stems_one_file_per_track() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("soundcraft-stems-{}", std::process::id()));
        let r = e
            .execute("file.bounce_stems", &serde_json::json!({"dir": dir.to_string_lossy(), "start": {"seconds": 10.0}, "end": {"seconds": 11.0}}))
            .unwrap();
        let n = r["files"].as_array().map_or(0, Vec::len);
        assert_eq!(n, 7, "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod strip_tests {
    use super::*;

    /// A stereo track with `length`-long clips at `starts`, each playing a second of steady tone.
    fn tones(starts: &[Samples], length: Samples) -> (Engine, TrackId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id;
        let s = e.session_mut();
        let tone = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]; 2] };
        let src = add_source(s, "tone", tone, None, FileFormat::Wav);
        for at in starts {
            let id = s.new_clip_id();
            crate::edit::place_clip(s, t, Clip::audio(id, "tone", src, 0, *at, length));
        }
        (e, t)
    }

    #[test]
    fn clips_past_what_one_render_holds_are_kept() {
        // 100 minutes in: a stereo render stops at 2^28 frames, about 93 minutes.
        let far = 48_000 * 6000;
        let (mut e, t) = tones(&[0, far], 48_000);
        let r = e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": far + 96_000})).unwrap();
        assert_eq!(r["clips"], 2);
        assert!(e.session().track(t).unwrap().clips().iter().any(|c| c.start >= far));
    }

    #[test]
    fn more_than_a_day_of_clips_is_refused() {
        let (mut e, t) = tones(&[0], 48_000 * 86_401);
        assert!(e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": 48_000i64 * 86_402})).is_err());
        assert_eq!(e.session().track(t).unwrap().clips().len(), 1);
    }
}
