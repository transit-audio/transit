//! Recording file planning and take placement.

use super::*;

/// The record-enabled audio tracks, each with the input channels it records (those its input
/// route names, "In 3" → 2, wrapped to the device's `input_channels`) and a new take file
/// `<track>_<NN>.wav` in the `Audio Files` of the session's folder (`unsaved_dir` until it is saved).
pub fn plan_takes(e: &Engine, unsaved_dir: &Path, input_channels: usize) -> Vec<(TrackId, Vec<usize>, PathBuf)> {
    let dir = e.path.as_deref().and_then(|p| Path::new(p).parent()).unwrap_or(unsaved_dir);
    let s = e.session();
    let take_no = s.sources.len() + 1;
    s.tracks
        .iter()
        .filter(|t| t.mixer.record_arm && t.kind == TrackKind::Audio && !t.inactive)
        .map(|t| {
            // The same resolution as the mix's input monitoring: a path by its name in this
            // machine's table, else the number in the label.
            let inputs = (0..t.channels())
                .map(|k| {
                    let channel = match &t.mixer.input {
                        transit_model::Route::Hardware(h) => transit_model::input_channel(Some(e.io_map()), h, k),
                        _ => k,
                    };
                    channel % input_channels.max(1)
                })
                .collect();
            let file = |n: usize| dir.join("Audio Files").join(format!("{}_{n:02}.wav", sanitize_name(&t.name)));
            let path = (take_no..take_no + 1_000).map(file).find(|p| !p.exists()).unwrap_or_else(|| file(take_no));
            (t.id, inputs, path)
        })
        .collect()
}

/// A recording's plan, made when it starts: each record-enabled track's take file, where the
/// takes start on the timeline, the loop length they split into when loop recording, and the
/// punch range they're trimmed to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub start: Samples,
    pub pass: Option<Samples>,
    pub punch: Option<Range>,
    /// The input's sample rate, which the take files are written at.
    pub rate: u32,
    pub takes: Vec<Take>,
    /// The MIDI take journal of the pass, if MIDI is being recorded (crash recovery rebuilds the
    /// clip from it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_journal: Option<PathBuf>,
    /// The pass's MIDI clips are already in the session this plan was saved with, so recovery
    /// discards the journal instead of rebuilding them (set once Stop has placed the clips).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub midi_placed: bool,
}

/// One track's take file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Take {
    pub track: TrackId,
    pub path: PathBuf,
    pub channels: usize,
}

/// Place recorded take files as `plan` says, as one undo step ("Record").
pub fn place_recording(e: &mut Engine, plan: &Recording, takes: Vec<(TrackId, PathBuf)>) -> Vec<transit_model::ClipId> {
    let ids = add_recording(e, plan.start, takes, plan.pass);
    if let Some(p) = plan.punch {
        let clips: Vec<u64> = ids.iter().map(|c| c.0).collect();
        let _ = e.execute_merged("edit.trim_to_fill_selection", &json!({"clips": clips, "start": p.start, "end": p.end}), "record");
    }
    ids
}

/// `place_recording` that is always exactly one undo step labeled "Record", whatever it does:
/// with `new_take`, a track that already has clips under the take (within the punch range)
/// first gets a duplicate of its playlist, so the old clips stay as the previous take; the punch
/// trim is folded in too. A loop recording over clips duplicates too, as MIDI does: the old
/// clips stay as take 1 (#25).
pub fn place_recording_take(e: &mut Engine, plan: &Recording, takes: Vec<(TrackId, PathBuf)>, new_take: bool) -> Vec<transit_model::ClipId> {
    let ids = add_recording_take(e, plan.start, takes, plan.pass, new_take, plan.punch);
    let n0 = e.undo_pushes();
    if !ids.is_empty() {
        if let Some(p) = plan.punch {
            let clips: Vec<u64> = ids.iter().map(|c| c.0).collect();
            let _ = e.execute("edit.trim_to_fill_selection", &json!({"clips": clips, "start": p.start, "end": p.end}));
        }
        // The add above is already one "Record" step; fold in the trim pushed since.
        let added = usize::try_from(e.undo_pushes() - n0).unwrap_or(0);
        if added > 0 {
            let _ = e.squash_undo(added + 1, "Record");
        }
    }
    ids
}

/// Turn recorded take files into clips at `start`, as one undo step ("Record"); each file is the
/// source of its clips. With `pass` (loop recording), a take becomes one clip per pass (at most
/// 1000), each pass after the first on a new playlist.
pub fn add_recording(e: &mut Engine, start: Samples, takes: Vec<(TrackId, PathBuf)>, pass: Option<Samples>) -> Vec<transit_model::ClipId> {
    add_recording_take(e, start, takes, pass, false, None)
}

/// `add_recording` with `new_take`: a track with clips under the take (clipped to `punch`) gets a
/// duplicate of its active playlist first, inside the same "Record" step.
pub fn add_recording_take(
    e: &mut Engine,
    start: Samples,
    takes: Vec<(TrackId, PathBuf)>,
    pass: Option<Samples>,
    new_take: bool,
    punch: Option<Range>,
) -> Vec<transit_model::ClipId> {
    let before = e.session_arc();
    let n0 = e.undo_pushes();
    let mut out = Vec::new();
    let mut unplaced = Vec::new();
    for (tid, path) in takes {
        let got = match take_audio(e, &path) {
            Ok(got) => got,
            Err(entry) => {
                unplaced.extend(entry);
                continue;
            }
        };
        if new_take {
            // A loop's passes all lie over its first lap.
            let frames = i64::try_from(got.frames()).unwrap_or(0);
            let frames = pass.map_or(frames, |p| frames.min(p.max(1)));
            let from = punch.map_or(start, |p| p.start.max(start));
            let to = punch.map_or(start + frames, |p| p.end.min(start + frames));
            let over = to > from && e.session().track(tid).is_some_and(|t| t.clips().iter().any(|c| c.start < to && c.end() > from));
            let previous = e.session().track(tid).map_or(0, |t| t.active_playlist);
            if over && let Err(err) = e.execute("track.playlist_duplicate", &json!({"track": tid.0})) {
                e.message(format!("Take kept at {} but not placed: {err}", path.display()));
                continue;
            }
            if over {
                crate::takes::number_covered(e.session_mut(), tid, Range::new(from, to), previous);
            }
        }
        let base = e.session().track(tid).map_or(0, |t| t.active_playlist);
        let first = out.len();
        let s = e.session_mut();
        if s.track(tid).is_none() {
            continue;
        }
        let name = path.file_stem().and_then(|x| x.to_str()).unwrap_or("Take");
        let src = match got {
            Got::Memory(buf) => {
                let resampled = buf.sample_rate != s.sample_rate.hz();
                let src = add_source(s, name, buf, Some(&path.to_string_lossy()), FileFormat::Wav);
                if let Some(x) = s.sources.last_mut() {
                    x.unsaved = resampled;
                }
                src
            }
            // A long take is one clip on one source played from its file (ADR 0035).
            Got::Disk(audio) => super::disk::add_disk_source(s, name, audio, &path.to_string_lossy()),
        };
        let Some(x) = s.source(src) else { continue };
        let len = i64::try_from(x.frames).unwrap_or(0);
        let pass = pass.unwrap_or(len).max(1);
        for offset in (0..len).step_by(usize::try_from(pass).unwrap_or(usize::MAX)).take(1_000) {
            let n = pass.min(len - offset);
            if offset > 0 && n < pass / 8 {
                break;
            }
            if offset > 0
                && let Some(t) = s.track_mut(tid)
            {
                t.playlists.push(transit_model::Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
                t.active_playlist = t.playlists.len() - 1;
            }
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, tid, Clip::audio(cid, name, src, offset, start, n));
            out.push(cid);
        }
        // The track plays what it played before the passes, with the newest pass on top (#25).
        if let Some(new) = out.get(first..) {
            crate::takes::settle_new_takes(s, tid, base, new);
        }
    }
    if !out.is_empty() {
        e.push_undo("Record", before);
        let added = usize::try_from(e.undo_pushes() - n0).unwrap_or(0);
        if new_take && added > 1 {
            let _ = e.squash_undo(added, "Record");
        }
    }
    // After the undo step, so undoing the recording keeps the reference to the file.
    if !unplaced.is_empty() {
        let s = e.session_mut();
        for entry in unplaced {
            if !s.unplaced_takes.contains(&entry) {
                s.unplaced_takes.push(entry);
            }
        }
    }
    out
}

/// A take's audio: decoded, or played from its file.
enum Got {
    Memory(AudioBuffer),
    Disk(Arc<SourceAudio>),
}

impl Got {
    fn frames(&self) -> usize {
        match self {
            Got::Memory(buf) => buf.frames(),
            Got::Disk(audio) => audio.frames(),
        }
    }
}

/// The audio of the take at `path`: played from the file when it is long (over the setting or the
/// decode limit, ADR 0035), else decoded. `Err(Some(entry))` for a take that can be neither (the
/// entry the session keeps for it, said loudly), `Err(None)` for one that could not be read (said).
fn take_audio(e: &mut Engine, path: &Path) -> std::result::Result<Got, Option<String>> {
    let rate = e.session().sample_rate.hz();
    if let Some((info, len)) = super::disk::probe(path).filter(|(info, _)| super::disk::streams(e.session(), info)) {
        match super::disk::open_disk_audio(path, len, rate) {
            Ok(audio) => return Ok(Got::Disk(audio)),
            Err(why) if super::disk::over_decode_limit(&info) => {
                let held = Held::Sized { frames: info.frames, channels: info.channels, rate: info.sample_rate };
                return Err(Some(refuse_take(e, path, &held, Some(&why))));
            }
            // Short enough to hold: it is decoded instead.
            Err(why) => log::info!("{why}; the take is held in memory"),
        }
    }
    let bytes = read_file(path).map_err(|err| {
        e.message(err.to_string());
        None
    })?;
    match transit_audio_io::decode(&bytes, None) {
        Ok((_, buf)) => Ok(Got::Memory(buf)),
        Err(err @ transit_audio_io::AudioError::TooLarge(_)) => Err(Some(refuse_take(e, path, &Held::Unsized(err.to_string()), None))),
        Err(err) => {
            e.message(format!("{}: {err}", path.display()));
            Err(None)
        }
    }
}

/// What a refused take holds: its length and layout (read from the header), or only the decoder's words.
enum Held {
    Sized { frames: u64, channels: u16, rate: u32 },
    Unsized(String),
}

/// A rate in kHz: 48000 as "48", 44100 as "44.1".
fn khz(rate: u32) -> String {
    let k = f64::from(rate) / 1000.0;
    if rate.is_multiple_of(1000) { format!("{k:.0}") } else { format!("{k:.1}") }
}

fn layout(channels: u16) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        n => format!("{n}-channel"),
    }
}

/// Hours of `channels`-channel audio at `rate` that fit the decode limit.
fn limit_hours(channels: u16, rate: u32) -> f64 {
    transit_audio_io::MAX_DECODE_SAMPLES as f64 / f64::from(channels.max(1)) / f64::from(rate.max(1)) / 3600.0
}

/// Say, loudly, that the take at `path` was not placed and where the file is; returns the entry the
/// session keeps for it. `why` is why it could not be played from disk either.
fn refuse_take(e: &mut Engine, path: &Path, held: &Held, why: Option<&str>) -> String {
    let shown = path.display();
    let (what, said) = match held {
        Held::Sized { frames, channels, rate } => {
            let hours = *frames as f64 / f64::from((*rate).max(1)) / 3600.0;
            let what = format!("{hours:.2} h of {} at {} kHz", layout(*channels), khz(*rate));
            let said = format!("{what} ({frames} frames); the limit is {:.2} h", limit_hours(*channels, *rate));
            (what, said)
        }
        Held::Unsized(why) => (why.clone(), format!("refused by the decoder ({why})")),
    };
    let prefix = format!("{shown}: ");
    let disk = why.map_or_else(String::new, |w| format!(", and it cannot be played from disk ({})", w.strip_prefix(prefix.as_str()).unwrap_or(w)));
    e.warn(format!(
        "The take {shown} was not placed: it is {said}, more than a session can hold in memory{disk}. The file is kept at {shown}; import it in pieces or open it in another editor."
    ));
    format!("{shown} ({what})")
}

/// Where a take recording from `start` shows while it holds `len` samples, as it will be placed:
/// just the pass under way when loop recording in passes of `pass`, within the `punch` range.
/// Returns the timeline position of its first sample and the span it covers.
pub fn live_span(start: Samples, pass: Option<Samples>, punch: Option<Range>, len: Samples) -> Option<(Samples, Range)> {
    let origin = start.saturating_sub(pass.filter(|p| *p > 0).map_or(0, |p| len / p * p));
    let span = Range::new(start, origin.saturating_add(len));
    Some((origin, span.intersect(&punch.unwrap_or(span))?))
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_failed_playlist_duplicate_keeps_the_file_and_old_clips_issue_10() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let source =
            add_source(e.session_mut(), "Original", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 480]] }, None, FileFormat::Wav);
        let clip = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), t, Clip::audio(clip, "Original", source, 0, 0, 480));
        let tr = e.session_mut().track_mut(t).unwrap();
        tr.freeze = Some(transit_model::Frozen { source, start: 0, length: 480, hash: tr.freeze_hash(), revision: 1, latency: 0 });
        let before = e.session_arc();
        let path = std::env::temp_dir().join(format!("failed-playlist-{}.wav", std::process::id()));
        write_take(&path, 480);
        assert!(add_recording_take(&mut e, 0, vec![(t, path.clone())], None, true, None).is_empty());
        assert_eq!(e.session(), before.as_ref());
        assert!(path.is_file());
        assert!(e.messages.last().is_some_and(|m| m.contains("not placed") && m.contains(path.to_string_lossy().as_ref())));
        assert!(e.undo_history().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    /// Removes the directory on drop, so a failed assertion leaks nothing.
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    use super::super::disk::FAKE_LEN;

    /// Holds the faked file length; clears it on drop, even when the test panics.
    struct FakeLenGuard;
    impl Drop for FakeLenGuard {
        fn drop(&mut self) {
            FAKE_LEN.with(|l| l.set(None));
        }
    }

    /// A header for `frames` mono float frames at `rate`, the file's length faked to hold them.
    fn header_only(dir: &TempDir, name: &str, frames: u64, rate: u32) -> (PathBuf, FakeLenGuard) {
        let path = dir.0.join(name);
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(&path, transit_audio_io::wav_float_header(1, rate, frames).unwrap()).unwrap();
        FAKE_LEN.with(|l| l.set(Some(std::fs::metadata(&path).unwrap().len() + frames * 4)));
        (path, FakeLenGuard)
    }

    #[test]
    fn a_take_over_the_decode_limit_is_one_disk_backed_clip_that_undo_and_redo_keep_issue_187() {
        let dir = TempDir(std::env::temp_dir().join(format!("disk-take-undo-{}", std::process::id())));
        let frames = transit_audio_io::MAX_DECODE_SAMPLES + 1_000;
        let (path, _fake_len) = header_only(&dir, "Long_01.wav", frames, 48_000);
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let ids = add_recording_take(&mut e, 0, vec![(t, path.clone())], None, false, None);
        assert_eq!(ids.len(), 1, "one clip: {:?}", e.messages);
        assert!(e.session().unplaced_takes.is_empty());
        let src = e.session().sources.last().unwrap().clone();
        assert!(src.disk && !src.unsaved, "{src:?}");
        assert_eq!((src.frames, src.path.as_str()), (frames, path.to_string_lossy().as_ref()));
        let audio = e.session().pool.get(src.id).unwrap();
        assert!(audio.is_disk() && audio.buffer.channels.iter().all(Vec::is_empty), "nothing decoded");
        let clip = e.session().find_clip(ids[0]).unwrap().1.clone();
        assert_eq!((clip.start, clip.length, clip.source()), (0, frames as i64, Some(src.id)));
        assert_eq!(e.undo_history().last().map(String::as_str), Some("Record"));
        assert!(e.undo());
        assert!(e.session().find_clip(ids[0]).is_none());
        assert!(e.redo());
        assert!(e.session().find_clip(ids[0]).is_some());
    }

    #[test]
    fn a_long_take_that_cannot_stream_is_refused_and_survives_undo_and_redo_issue_192() {
        let dir = TempDir(std::env::temp_dir().join(format!("refused-undo-{}", std::process::id())));
        // At 44.1 kHz in a 48 kHz session: not streamed in slice 1 of ADR 0035, too long to decode.
        let (path, _fake_len) = header_only(&dir, "Long_01.wav", transit_audio_io::MAX_DECODE_SAMPLES + 1_000, 44_100);
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
        assert!(add_recording_take(&mut e, 0, vec![(t, path.clone())], None, false, None).is_empty());
        let listed = e.session().unplaced_takes.clone();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(listed[0].contains("Long_01.wav") && listed[0].contains("6.76 h of mono at 44.1 kHz"), "{listed:?}");
        let said = e.messages.last().unwrap();
        assert!(
            said.contains("the limit is 6.76 h") && said.contains("cannot be played from disk") && said.contains("The file is kept at"),
            "{said}"
        );
        assert_eq!(said.matches(path.to_string_lossy().as_ref()).count(), 2, "{said}");
        assert!(e.undo());
        assert_eq!(e.session().unplaced_takes, listed, "undo of an earlier edit keeps the reference");
        assert!(e.redo());
        assert_eq!(e.session().unplaced_takes, listed, "redo keeps it");
    }

    #[test]
    fn a_take_over_the_setting_streams_and_a_shorter_one_is_decoded_issue_187() {
        let dir = TempDir(std::env::temp_dir().join(format!("disk-setting-{}", std::process::id())));
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        // One second at 48 kHz against a setting of half a second.
        e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.5 / 60.0);
        if std::env::var(transit_model::disk::STREAM_MINUTES_ENV).is_ok() {
            return;
        }
        let long = dir.0.join("Long_01.wav");
        write_take(&long, 48_000);
        let short = dir.0.join("Short_01.wav");
        write_take(&short, 4_800);
        add_recording_take(&mut e, 0, vec![(t, long), (t, short)], None, false, None);
        let disk: Vec<bool> = e.session().sources.iter().map(|s| s.disk).collect();
        assert_eq!(disk, [true, false]);
    }

    #[test]
    fn the_decoders_own_refusal_reads_once_and_names_no_path_twice_issue_192() {
        let mut e = Engine::default();
        let entry =
            refuse_take(&mut e, Path::new("/rec/A.wav"), &Held::Unsized("audio too large: 5 samples exceeds the 4-sample decode limit".into()), None);
        assert_eq!(entry, "/rec/A.wav (audio too large: 5 samples exceeds the 4-sample decode limit)");
        assert_eq!(e.messages.last().unwrap().matches("/rec/A.wav").count(), 2, "once as the take, once as where it is kept");
    }

    fn armed(e: &mut Engine, name: &str, format: &str, input: &str) -> TrackId {
        let id = e.execute("track.new", &json!({"name": name, "format": format})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn write_take(path: &Path, frames: usize) {
        let mut bytes = transit_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
        bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn an_input_path_by_name_records_its_channel_and_old_labels_keep_their_meaning() {
        use transit_model::{ChannelFormat, IoDirection, IoMap, IoPath};
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-path-{}", std::process::id()));
        let mut e = Engine::default();
        let mut io = IoMap::for_device("ASIO", "x", 8, 2);
        io.add_path(IoPath { name: "Guitar".into(), direction: IoDirection::Input, channels: vec![4], format: ChannelFormat::Mono }).unwrap();
        io.add_path(IoPath { name: "Drum pair".into(), direction: IoDirection::Input, channels: vec![6, 7], format: ChannelFormat::Stereo }).unwrap();
        e.set_io_map(io);
        let gtr = armed(&mut e, "Gtr", "mono", "Guitar");
        let drums = armed(&mut e, "Drums", "stereo", "Drum pair");
        let old = armed(&mut e, "Old", "stereo", "In 3-4");
        let plan = plan_takes(&e, &dir, 8);
        let channels = |t: TrackId| plan.iter().find(|p| p.0 == t).map(|p| p.1.clone()).unwrap();
        assert_eq!(channels(gtr), [4], "Guitar is input channel 5");
        assert_eq!(channels(drums), [6, 7]);
        assert_eq!(channels(old), [2, 3]);
        // The same names, resolved by the mix: one function (`transit_model::input_channel`).
        assert_eq!(transit_model::input_channel(Some(e.io_map()), "Guitar", 0), 4);
    }

    #[test]
    fn armed_tracks_get_new_take_files_used_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 2");
        let gtr = armed(&mut e, "Gtr", "stereo", "In 3-4");
        write_take(&dir.join("Audio Files/Vox_01.wav"), 10);
        let plan = plan_takes(&e, &dir, 4);
        assert_eq!(plan, vec![(vox, vec![1], dir.join("Audio Files/Vox_02.wav")), (gtr, vec![2, 3], dir.join("Audio Files/Gtr_01.wav"))]);
        assert_eq!(plan_takes(&e, &dir, 2)[1].1, vec![0, 1]);

        let session = dir.join("Song.transit").to_string_lossy().into_owned();
        e.execute("session.save_as", &json!({"path": session})).unwrap();
        write_take(&plan[0].2, 4_800);
        let clips = add_recording(&mut e, 96_000, vec![(vox, plan[0].2.clone())], None);
        let clip = e.session().find_clip(clips[0]).unwrap().1.clone();
        assert_eq!((clip.start, clip.length), (96_000, 4_800));
        assert_eq!(save_session(&mut e, &session).unwrap(), 0);
        let src = e.session().sources.last().unwrap().clone();
        assert_eq!((src.name.as_str(), src.path.as_str(), src.unsaved, src.frames), ("Vox_02", "Audio Files/Vox_02.wav", false, 4_800));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_loop_take_becomes_one_clip_per_pass_and_autosaves_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-loop-take-{}", std::process::id()));
        let path = dir.join("Audio Files/Vox_01.wav");
        write_take(&path, 1_050);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        assert_eq!(add_recording(&mut e, 1_000, vec![(vox, path.clone())], Some(400)).len(), 3);
        assert_eq!(save_copy(&e, &dir.join("Untitled.transit").to_string_lossy()).unwrap(), 0);
        let t = e.session().track(vox).unwrap();
        let passes: Vec<_> = t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
        // The track plays its first playlist again: the newest (partial) pass over its range and
        // the rest of the pass before it, as Live's main lane does (#25). Pass 1 and the start of
        // pass 2 are the other takes.
        assert_eq!(passes, vec![(800, 1_000, 250), (650, 1_250, 150), (0, 1_000, 400), (400, 1_000, 250)]);
        assert_eq!((t.active_playlist, t.playlists[2].name.as_str()), (0, "Vox.03"));
        assert_eq!(t.take_numbers(), vec![3, 2, 1]);
        assert_eq!(t.take_stacks()[0].active(), Some(3));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn three_laps_of_cycle_recording_are_three_audio_takes_each_one_lap_long() {
        let dir = std::env::temp_dir().join(format!("soundcraft-cycle-take-{}", std::process::id()));
        let path = dir.join("Audio Files/Gtr_01.wav");
        write_take(&path, 1_200);
        let mut e = Engine::default();
        let gtr = armed(&mut e, "Gtr", "mono", "In 1");
        assert_eq!(add_recording(&mut e, 1_000, vec![(gtr, path)], Some(400)).len(), 3);
        let t = e.session().track(gtr).unwrap();
        assert_eq!(t.playlists.len(), 3, "a take per lap, stacked");
        let laps: Vec<_> = t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
        // The last lap plays (on the playlist the track played before); laps 1 and 2 are takes.
        assert_eq!(laps, vec![(800, 1_000, 400), (0, 1_000, 400), (400, 1_000, 400)]);
        let stacks = t.take_stacks();
        assert_eq!((stacks.len(), stacks[0].takes.len(), stacks[0].active(), t.active_playlist), (1, 3, Some(3), 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #25: the passes of a cycle recording used to leave the track playing the last pass's
    /// playlist, which held only that pass: every other clip on the track went silent, and a
    /// clip under the loop was cut away. Now the track keeps playing its own playlist, the newest
    /// pass on it, and a clip that was under the loop is take 1.
    #[test]
    fn cycle_recording_keeps_the_rest_of_the_track_playing_and_the_clip_under_the_loop_as_take_1() {
        let dir = TempDir(std::env::temp_dir().join(format!("transit-cycle-rest-{}", std::process::id())));
        let path = dir.0.join("Audio Files/Gtr_01.wav");
        write_take(&path, 1_200);
        let mut e = Engine::default();
        let gtr = armed(&mut e, "Gtr", "mono", "In 1");
        let s = e.session_mut();
        let source = add_source(s, "Old", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 4_000]] }, None, FileFormat::Wav);
        let (under, later) = (s.new_clip_id(), s.new_clip_id());
        crate::edit::place_clip(s, gtr, Clip::audio(under, "Under", source, 0, 900, 600));
        crate::edit::place_clip(s, gtr, Clip::audio(later, "Later", source, 0, 3_000, 500));
        let undo = e.undo_history().len();
        let plan = Recording { start: 1_000, pass: Some(400), punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false };
        let ids = place_recording_take(&mut e, &plan, vec![(gtr, path)], true);
        assert_eq!(ids.len(), 3);
        assert_eq!(e.undo_history().len(), undo + 1, "one Record step");
        let t = e.session().track(gtr).unwrap();
        let playing: Vec<(transit_model::ClipId, i64, i64, Option<u32>)> = t.clips().iter().map(|c| (c.id, c.start, c.length, c.take)).collect();
        assert_eq!(playing.first().map(|c| (c.1, c.2, c.3)), Some((900, 100, Some(1))), "the old clip before the loop still plays, as take 1");
        assert!(playing.iter().any(|c| c.0 == ids[2] && c.3 == Some(4)), "the newest pass plays: {playing:?}");
        assert!(playing.iter().any(|c| c.1 == 1_400 && c.2 == 100 && c.3 == Some(1)), "and the old clip after it: {playing:?}");
        // (The duplicated playlist's copy of it: Duplicate Playlist gives each copy a new id.)
        assert!(playing.iter().any(|c| (c.1, c.2) == (3_000, 500)), "the clip later on the track keeps playing");
        assert!(t.playlists[0].clip(later).is_some());
        let stacks = t.take_stacks();
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].takes.iter().map(|k| k.number).collect::<Vec<_>>(), vec![1, 2, 3, 4], "the old clip, then a take per pass");
        assert_eq!(stacks[0].active(), Some(4));
        assert!(t.playlists.iter().any(|p| p.clip(under).is_some_and(|c| c.length == 600)), "the whole old clip is kept as take 1");
        e.execute("edit.undo", &json!({})).unwrap();
        let t = e.session().track(gtr).unwrap();
        assert_eq!((t.playlists.len(), t.clips().len()), (1, 2));
        assert!(t.clips().iter().all(|c| c.take.is_none()), "undo leaves no take numbers behind");
    }

    fn over_a_clip(tag: &str) -> (Engine, TrackId, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("soundcraft-newtake-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (first, second) = (dir.join("Audio Files/Vox_01.wav"), dir.join("Audio Files/Vox_02.wav"));
        write_take(&first, 4_800);
        write_take(&second, 4_800);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        add_recording(&mut e, 1_000, vec![(vox, first.clone())], None);
        (e, vox, first, second)
    }

    #[test]
    fn new_take_recording_over_a_clip_is_one_record_step() {
        let (mut e, vox, _, second) = over_a_clip("one");
        let before = e.undo_history();
        let plan = Recording { start: 2_000, pass: None, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false };
        let ids = place_recording_take(&mut e, &plan, vec![(vox, second)], true);
        assert_eq!(ids.len(), 1);
        let h = e.undo_history();
        assert_eq!((h.len(), h.last().map(String::as_str)), (before.len() + 1, Some("Record")));
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 2);
        e.execute("edit.undo", &json!({})).unwrap();
        let t = e.session().track(vox).unwrap();
        assert_eq!((t.playlists.len(), t.clips().len(), t.clips()[0].start), (1, 1, 1_000));
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 2);
    }

    #[test]
    fn new_take_with_punch_folds_the_trim_into_the_record_step() {
        let (mut e, vox, _, second) = over_a_clip("punch");
        let n = e.undo_history().len();
        let plan = Recording {
            start: 1_000,
            pass: None,
            punch: Some(Range::new(2_000, 3_000)),
            rate: 48_000,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
        };
        let ids = place_recording_take(&mut e, &plan, vec![(vox, second)], true);
        let h = e.undo_history();
        assert_eq!((h.len(), h.last().map(String::as_str)), (n + 1, Some("Record")));
        let c = e.session().find_clip(ids[0]).unwrap().1.clone();
        assert_eq!((c.start, c.length), (2_000, 1_000));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
    }

    #[test]
    fn new_take_fold_still_works_when_the_history_is_full() {
        let (mut e, vox, _, second) = over_a_clip("full");
        e.history_limit = e.undo_history().len();
        let last = e.undo_history().pop();
        let plan = Recording { start: 2_000, pass: None, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false };
        place_recording_take(&mut e, &plan, vec![(vox, second)], true);
        let h = e.undo_history();
        // One new entry on top of the previous newest; the duplicate step is folded, not left behind.
        assert_eq!(h[h.len() - 2..], [last.unwrap(), "Record".to_string()]);
        assert!(!h.iter().any(|l| l == "Duplicate Playlist"));
        e.execute("edit.undo", &json!({})).unwrap();
        let t = e.session().track(vox).unwrap();
        assert_eq!((t.playlists.len(), t.clips().len(), t.clips()[0].start), (1, 1, 1_000));
    }

    #[test]
    fn new_take_across_two_tracks_with_clips_is_one_step() {
        let (mut e, vox, _, second) = over_a_clip("two");
        let gtr = armed(&mut e, "Gtr", "mono", "In 2");
        add_recording(&mut e, 1_000, vec![(gtr, second.clone())], None);
        let n = e.undo_history().len();
        let plan = Recording { start: 2_000, pass: None, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false };
        place_recording_take(&mut e, &plan, vec![(vox, second.clone()), (gtr, second)], true);
        let h = e.undo_history();
        assert_eq!((h.len(), h.last().map(String::as_str)), (n + 1, Some("Record")));
        assert_eq!([vox, gtr].map(|t| e.session().track(t).unwrap().playlists.len()), [2, 2]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!([vox, gtr].map(|t| e.session().track(t).unwrap().playlists.len()), [1, 1]);
    }

    #[test]
    fn new_take_into_empty_space_does_not_duplicate_and_is_one_step() {
        let (mut e, vox, _, second) = over_a_clip("empty");
        let n = e.undo_history().len();
        let plan = Recording { start: 20_000, pass: None, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false };
        place_recording_take(&mut e, &plan, vec![(vox, second)], true);
        let h = e.undo_history();
        assert_eq!((h.len(), h.last().map(String::as_str)), (n + 1, Some("Record")));
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
    }

    #[test]
    fn recording_without_new_take_keeps_the_old_steps() {
        let (mut e, vox, _, second) = over_a_clip("old");
        let n = e.undo_history().len();
        let plan = Recording {
            start: 1_000,
            pass: None,
            punch: Some(Range::new(2_000, 3_000)),
            rate: 48_000,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
        };
        place_recording(&mut e, &plan, vec![(vox, second)]);
        assert_eq!(e.undo_history().len(), n + 2);
    }

    #[test]
    fn a_take_shows_the_pass_under_way_within_the_punch_while_it_records() {
        assert_eq!(live_span(1_000, None, None, 0), None);
        assert_eq!(live_span(1_000, None, None, 500), Some((1_000, Range::new(1_000, 1_500))));
        assert_eq!(live_span(1_000, Some(400), None, 900), Some((200, Range::new(1_000, 1_100))));
        assert_eq!(live_span(1_000, Some(400), None, 800), None);
        let punch = Some(Range::new(1_200, 1_400));
        assert_eq!(live_span(1_000, None, punch, 150), None);
        assert_eq!(live_span(1_000, None, punch, 500), Some((1_000, Range::new(1_200, 1_400))));
    }
}
