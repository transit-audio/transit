//! Consolidated per-track stems from session start, with editable MIDI beside them (#261 slice 3).

use super::{Bounce, BounceOptions, EncodeOptions, EngineError, Path, PathBuf, Range, Result, Session, TrackId, TrackKind};
use std::sync::Arc;

const CMD: &str = "file.bounce_consolidated";

/// A finished consolidated export: full-length stems, per-track SMFs and a conductor-only SMF.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidatedBounced {
    pub files: Vec<PathBuf>,
    pub midi_files: Vec<PathBuf>,
    pub tempo_file: PathBuf,
}

struct TrackExport {
    id: TrackId,
    stem: Option<PathBuf>,
    midi: Option<PathBuf>,
}

/// An incremental consolidated export. Each track uses the ordinary stem renderer; each SMF uses
/// the ordinary arrangement-accurate MIDI exporter, including tempo, meter and markers.
pub struct ConsolidatedBounce {
    session: Arc<Session>,
    dir: PathBuf,
    range: Range,
    opts: EncodeOptions,
    fold_stereo: bool,
    tracks: Vec<TrackExport>,
    tempo: PathBuf,
    next: usize,
    current: Option<Bounce>,
    done: ConsolidatedBounced,
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

fn same_output(a: &Option<PathBuf>, b: &Option<PathBuf>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same_path(a, b),
        _ => false,
    }
}

impl ConsolidatedBounce {
    /// Prepare one stem per audio, instrument and MIDI track from sample zero to the content end,
    /// one SMF per MIDI track, and a tempo-only SMF. External Instrument audio is skipped, while
    /// its MIDI remains. Muted and inactive tracks are rendered in isolation so their material is
    /// not lost from the interchange package.
    pub fn new(s: Arc<Session>, dir: &Path, opts: &EncodeOptions, fold_stereo: bool) -> Result<Self> {
        let range = Range::new(0, transit_mix::playback_content_end(&s));
        super::check_bounce_range(&s, range, CMD)?;
        super::make_dir(dir)?;
        let ext = transit_audio_io::encode_extension(opts.format);
        let tempo = dir.join(format!("{} - tempo.mid", super::sanitize_name(&s.name)));
        let mut tracks = Vec::new();
        for track in &s.tracks {
            let external = track.external_instrument().is_some() && track.freeze.is_none();
            let audible = matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi) && !external;
            let has_midi = track.kind.is_midi();
            if !audible && !has_midi {
                continue;
            }
            let mut number = 1;
            let (stem, midi) = loop {
                let suffix = if number == 1 { String::new() } else { format!(" {number}") };
                let base = format!("{} - {}{suffix}", super::sanitize_name(&s.name), super::sanitize_name(&track.name));
                let stem = audible.then(|| dir.join(format!("{base}.{ext}")));
                let midi = has_midi.then(|| dir.join(format!("{base}.mid")));
                let collision = tracks.iter().any(|t: &TrackExport| same_output(&t.stem, &stem) || same_output(&t.midi, &midi))
                    || midi.as_ref().is_some_and(|path| same_path(path, &tempo));
                if !collision {
                    break (stem, midi);
                }
                number += 1;
            };
            tracks.push(TrackExport { id: track.id, stem, midi });
        }
        if tracks.is_empty() {
            return Err(EngineError::BadParams(CMD.into(), "no audio, instrument or MIDI tracks to export".into()));
        }
        for path in tracks.iter().flat_map(|t| [t.stem.as_ref(), t.midi.as_ref()].into_iter().flatten()).chain(std::iter::once(&tempo)) {
            crate::file_access::check_write(path).map_err(EngineError::Io)?;
            let name = path.file_name().map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
            crate::file_access::check_write(path.with_file_name(format!("{name}.part"))).map_err(EngineError::Io)?;
        }
        Ok(Self {
            session: s,
            dir: dir.to_path_buf(),
            range,
            opts: opts.clone(),
            fold_stereo,
            tracks,
            tempo,
            next: 0,
            current: None,
            done: ConsolidatedBounced::default(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// Every destination, including MIDI, for replacement confirmation before rendering starts.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self.tracks.iter().filter_map(|t| t.stem.clone()).collect();
        files.extend(self.tracks.iter().filter_map(|t| t.midi.clone()));
        files.push(self.tempo.clone());
        files
    }

    /// Files completed so far, including MIDI and the tempo map.
    pub fn written(&self) -> Vec<PathBuf> {
        let mut files = self.done.files.clone();
        files.extend(self.done.midi_files.clone());
        if !self.done.tempo_file.as_os_str().is_empty() {
            files.push(self.done.tempo_file.clone());
        }
        files
    }

    /// A stop/error report that names every file already completed.
    pub fn report_stopped(&self, reason: &str) -> String {
        let written = self.written();
        if written.is_empty() {
            reason.to_string()
        } else {
            format!("{reason}; completed files: {}", written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))
        }
    }

    pub fn progress(&self) -> f32 {
        if self.next == self.tracks.len() {
            return 1.0;
        }
        let current = self.current.as_ref().map_or(0.0, Bounce::progress);
        (self.next as f32 + current) / self.tracks.len().max(1) as f32
    }

    /// Render until `more` returns false, at least one block per call. MIDI for a track is written
    /// after that track's stem; the conductor-only SMF is written after every track completes.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> Result<Option<ConsolidatedBounced>> {
        loop {
            if self.next == self.tracks.len() {
                let bytes = super::midi::export_tempo_map(&self.session)?;
                let name = self.tempo.file_name().map_or_else(|| "tempo.mid".into(), |n| n.to_string_lossy().into_owned());
                super::session::replace_file(&self.tempo.with_file_name(format!("{name}.part")), &self.tempo, &bytes)?;
                self.done.tempo_file = self.tempo.clone();
                return Ok(Some(std::mem::take(&mut self.done)));
            }
            if self.current.is_none()
                && let Some(path) = self.tracks[self.next].stem.clone()
            {
                let id = self.tracks[self.next].id;
                let downstream = super::dawproject_export::downstream_tracks(&self.session, id);
                let mut solo = Arc::new((*self.session).clone());
                for track in &mut Arc::make_mut(&mut solo).tracks {
                    track.mixer.solo = track.id == id || downstream.contains(&track.id);
                    track.mixer.solo_safe = false;
                    track.mixer.mute = false;
                    track.inactive = false;
                }
                self.current = Some(Bounce::with_options(
                    solo,
                    &path,
                    self.range,
                    &self.opts,
                    BounceOptions { fold_stereo: self.fold_stereo, ..BounceOptions::default() },
                )?);
            }
            let mut finished_stem = false;
            if let Some(job) = &mut self.current {
                match job.step(&mut more)? {
                    Some(_) => {
                        self.done.files.push(self.tracks[self.next].stem.clone().unwrap_or_default());
                        self.current = None;
                        finished_stem = true;
                    }
                    None => return Ok(None),
                }
            }
            if self.current.is_none() && (finished_stem || self.tracks[self.next].stem.is_none()) {
                if let Some(path) = self.tracks[self.next].midi.clone() {
                    let bytes = super::midi::export_consolidated_midi_track(&self.session, self.tracks[self.next].id)?;
                    let name = path.file_name().map_or_else(|| "export.mid".into(), |n| n.to_string_lossy().into_owned());
                    super::session::replace_file(&path.with_file_name(format!("{name}.part")), &path, &bytes)?;
                    self.done.midi_files.push(path);
                }
                self.next += 1;
            }
            if !more() {
                return Ok(None);
            }
        }
    }

    /// Render the complete export (see [`Self::step`]).
    pub fn run(mut self) -> Result<ConsolidatedBounced> {
        loop {
            match self.step(|| true) {
                Ok(Some(done)) => return Ok(done),
                Ok(None) => {}
                Err(e) => return Err(EngineError::Io(self.report_stopped(&e.to_string()))),
            }
        }
    }
}

/// Render a consolidated export to completion.
pub fn bounce_consolidated(s: Arc<Session>, dir: &Path, opts: &EncodeOptions, fold_stereo: bool) -> Result<ConsolidatedBounced> {
    ConsolidatedBounce::new(s, dir, opts, fold_stereo)?.run()
}
