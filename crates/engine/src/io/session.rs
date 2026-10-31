use super::*;

/// Save the session to `path` (a `.transit` file). Audio that only exists in memory is written
/// as 32-bit float WAV into `Audio Files/` next to it. Returns how many audio files were written.
pub fn save_session(e: &mut Engine, path: &str) -> Result<usize> {
    let (fresh, written, p) = write_session(e.session(), path, false)?;
    // Keep the in-memory document in sync (paths, unsaved flags) without an undo step.
    *e.session_mut() = fresh;
    e.path = Some(p.to_string_lossy().into_owned());
    e.mark_clean();
    Ok(written)
}

/// Write a copy of the session (and any audio it needs) without touching the open document —
/// used for autosave / crash recovery. Audio already written for a source is reused.
pub fn save_copy(e: &Engine, path: &str) -> Result<usize> {
    write_session(e.session(), path, true).map(|(_, n, _)| n)
}

/// File › Save Session Copy In: a normal session folder that does not rename, retarget or
/// clean the session that stays open.
pub fn save_session_copy(e: &Engine, path: &str) -> Result<usize> {
    write_session(e.session(), path, false).map(|(_, n, _)| n)
}

/// Write `session` to `path`; returns the session with updated media paths, the number of audio
/// files written and the final path.
fn write_session(session: &Session, path: &str, overwrite: bool) -> Result<(Session, usize, PathBuf)> {
    use std::collections::{HashMap, HashSet};
    let p = session_file(path);
    // Before any audio is written: a session saved outside the write root changes nothing.
    crate::file_access::check_write(&p).map_err(EngineError::Io)?;
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let audio_dir = dir.join("Audio Files");
    let mut s = session.clone();
    let from = s.folder.clone();
    let same_dir = from.as_deref().is_some_and(|f| same_path(f, &dir));
    let mut written = 0;
    // A file can stay put for all the sources that name it, unless their audio differs.
    let shared_paths: Vec<bool> = {
        let mut audio_by_path: HashMap<&str, HashSet<Option<*const SourceAudio>>> = HashMap::new();
        for src in &s.sources {
            audio_by_path.entry(&src.path).or_default().insert(s.pool.get(src.id).map(Arc::as_ptr));
        }
        s.sources.iter().map(|src| audio_by_path.get(src.path.as_str()).is_some_and(|a| a.len() > 1)).collect()
    };
    // Files written for audio during this save: sources playing the same audio share one.
    let mut files: HashMap<*const SourceAudio, String> = HashMap::new();
    for (i, src) in s.sources.iter_mut().enumerate() {
        let streamed = s.pool.get(src.id).is_some_and(|a| a.is_disk());
        if !same_dir
            && (s.pool.get(src.id).is_none() || streamed)
            && Path::new(&src.path).is_relative()
            && let Some(f) = &from
        {
            src.path = f.join(&src.path).to_string_lossy().into_owned();
        }
        let exists_rel = same_dir && !Path::new(&src.path).is_absolute() && dir.join(&src.path).exists();
        let shared = shared_paths.get(i).copied().unwrap_or(false);
        if exists_rel && !src.unsaved && !shared {
            continue;
        }
        let in_folder = Path::new(&src.path).strip_prefix(&audio_dir).ok().and_then(Path::to_str).map(str::to_string);
        if let Some(f) = in_folder.filter(|_| !src.unsaved && Path::new(&src.path).is_file()) {
            src.path = format!("Audio Files/{f}");
            continue;
        }
        let Some(audio) = s.pool.get(src.id) else { continue };
        // A disk-backed source's file is its audio (ADR 0035): referenced where it is, never rewritten.
        if audio.is_disk() {
            continue;
        }
        if let Some(path) = files.get(&Arc::as_ptr(audio)) {
            src.path.clone_from(path);
            src.unsaved = false;
            src.format = FileFormat::Wav;
            continue;
        }
        make_dir(&audio_dir)?;
        let mut stem = sanitize_name(&src.name);
        if overwrite {
            // One file per source id so autosaves never collide or pile up.
            stem = format!("{stem}_{}", src.id.0);
            if audio_dir.join(format!("{stem}.wav")).exists() {
                src.path = format!("Audio Files/{stem}.wav");
                src.unsaved = false;
                files.insert(Arc::as_ptr(audio), src.path.clone());
                continue;
            }
        }
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: transit_audio_io::BitDepth::Float32, dither: false, bwf: None };
        let bytes = transit_audio_io::encode(&audio.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
        let (name, new) = store_wav(&audio_dir, &stem, &bytes)?;
        src.path = format!("Audio Files/{name}");
        src.unsaved = false;
        src.format = FileFormat::Wav;
        files.insert(Arc::as_ptr(audio), src.path.clone());
        written += usize::from(new);
    }
    if written > 0 {
        sync_dir(&audio_dir);
    }
    s.folder = Some(dir.clone());
    if !overwrite && let Some(stem) = p.file_stem().and_then(|x| x.to_str()) {
        s.name = stem.to_string();
    }
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    make_dir(&dir)?;
    // Atomic-ish write: temp file then rename.
    let tmp = p.with_extension("transit.tmp");
    if !overwrite && read_file(&p).is_ok_and(|old| old != text.as_bytes()) {
        back_up(&p);
    }
    replace_file(&tmp, &p, text.as_bytes())?;
    Ok((s, written, p))
}

/// Write `bytes` to `tmp`, flush it to disk and rename it over `file`, so `file` holds either the
/// old or the new contents, never a mix.
pub fn replace_file(tmp: &Path, file: &Path, bytes: &[u8]) -> Result<()> {
    crate::file_access::check_write(file).map_err(EngineError::Io)?;
    crate::file_access::check_write(tmp).map_err(EngineError::Io)?;
    replace_with(tmp, file, write_synced(tmp, bytes))
}

/// Renames `tmp` over `file` once it is `written`, or removes it when writing or renaming failed.
/// The caller has checked both paths ([`crate::file_access::check_write`]).
pub(super) fn replace_with(tmp: &Path, file: &Path, written: std::io::Result<()>) -> Result<()> {
    if let Err(err) = written.and_then(|()| std::fs::rename(tmp, file)) {
        let _ = std::fs::remove_file(tmp);
        return Err(EngineError::Io(format!("{}: {err}", file.display())));
    }
    sync_dir(file.parent().unwrap_or(Path::new("")));
    Ok(())
}

fn sync_dir(dir: &Path) {
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Copy the session file about to be replaced to `Backups/<name> backup <n>.<ext>`, keeping the
/// newest ten.
fn back_up(file: &Path) {
    let (Some(dir), Some(stem), Some(ext)) = (file.parent(), file.file_stem().and_then(|x| x.to_str()), file.extension().and_then(|x| x.to_str()))
    else {
        return;
    };
    let backups = dir.join("Backups");
    if let Err(err) = crate::file_access::check_write(&backups) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    let prefix = format!("{stem} backup ");
    let suffix = format!(".{ext}");
    let numbers: Vec<u32> = std::fs::read_dir(&backups)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|f| f.file_name().to_str()?.strip_prefix(&prefix)?.strip_suffix(&suffix)?.parse().ok())
        .collect();
    let next = numbers.iter().max().map_or(1, |n| n.saturating_add(1));
    let copied = std::fs::create_dir_all(&backups).and_then(|()| std::fs::copy(file, backups.join(format!("{prefix}{next}{suffix}"))));
    if let Err(err) = copied {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    for n in numbers.into_iter().filter(|n| n.saturating_add(10) <= next) {
        let _ = std::fs::remove_file(backups.join(format!("{prefix}{n}{suffix}")));
    }
}

/// The file a session saved to `path` goes to: `path`, with `.transit` added when it has no
/// extension and isn't an existing file.
pub fn session_file(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    // Only a path the write root allows is probed, so a refused one can't be used to test for files.
    let is_file = crate::file_access::check_write(&p).is_ok() && p.is_file();
    if p.extension().is_none() && !is_file { p.with_extension(transit_model::SESSION_EXTENSION) } else { p }
}

/// True when `command` would write a session over the existing `file`. Save, Save As and Save
/// Copy In may write the open session's own file; other commands never ask.
pub fn replaces(e: &Engine, command: &str, file: &Path) -> bool {
    match command {
        "session.save" | "session.save_as" | "session.save_copy" => {
            file.exists() && !e.path.as_deref().is_some_and(|own| same_path(Path::new(own), file))
        }
        "session.save_template" | "file.export_selected_tracks_as_session" => file.exists(),
        _ => false,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let full = |p: &Path| if p.as_os_str().is_empty() { Path::new(".").canonicalize() } else { p.canonicalize() };
    a == b || matches!((full(a), full(b)), (Ok(x), Ok(y)) if x == y)
}

/// Store `bytes` as the first of `<stem>.wav`, `<stem>_01.wav`, … in `dir` that is free or already
/// holds exactly these bytes, so saving never replaces audio; returns the name and whether it was
/// written.
fn store_wav(dir: &Path, stem: &str, bytes: &[u8]) -> Result<(String, bool)> {
    use std::io::Write;
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(err) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(EngineError::Io(format!("{}: {err}", path.display())));
                }
                return Ok((name, true));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) && std::fs::read(&path).is_ok_and(|b| b == bytes) {
                    return Ok((name, false));
                }
            }
            Err(err) => return Err(EngineError::Io(format!("{}: {err}", path.display()))),
        }
    }
    Err(EngineError::Io(format!("{}: no free file name for {stem}", dir.display())))
}

/// Open a `.transit` (or older `.transit`) session and decode its audio files (missing files are reported, not fatal).
pub fn open_session(e: &mut Engine, path: &str) -> Result<Vec<String>> {
    open_session_linked(e, path, Path::new(path))
}

/// [`open_session`], following the media paths in it as if they were stored in the session file
/// `links` (crash recovery: the session written beside the autosave, its paths from the original).
pub(crate) fn open_session_linked(e: &mut Engine, path: &str, links: &Path) -> Result<Vec<String>> {
    let started = std::time::Instant::now();
    let text = read_text(path)?;
    let (mut s, mut stored) = Session::from_json_as_saved(&text).map_err(|err| EngineError::Io(err.to_string()))?;
    let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut missing = Vec::new();
    let mut files = DecodedFiles::new();
    for src in &s.sources {
        let decoded = stored_path(&src.path, links)
            .map_err(|why| format!("{}: {why}", src.path))
            .and_then(|p| decode_file_streamed(&mut files, &p, s.sample_rate.hz(), src.disk));
        match decoded {
            Ok(audio) => s.pool.insert(src.id, audio),
            Err(err) => missing.push(err),
        }
    }
    // A source too long to decode opens disk-backed; it is marked so a save keeps its file.
    for src in &mut s.sources {
        src.disk |= s.pool.get(src.id).is_some_and(|a| a.is_disk());
    }
    s.folder = Some(dir);
    // The retired Subtractive Synth became a Macro Oscillator on parse (#234): the file still has
    // the old synth, so the opened session reads as modified. The snapshot shares what the open
    // attached to the session (audio, folder), so only the substitution differs.
    if let Some(st) = &mut stored {
        st.pool = s.pool.clone();
        st.sources.clone_from(&s.sources);
        st.folder.clone_from(&s.folder);
    }
    let mut notes = std::mem::take(&mut s.load_notes);
    // Takes an older build could not place (#192) are placed now when their files are there: a
    // change to the session as saved, so it opens modified.
    let mut as_saved = None;
    if !s.unplaced_takes.is_empty() {
        let before = s.clone();
        notes.retain(|n| !n.starts_with(transit_model::UNPLACED_NOTE));
        let (said, placed) = super::disk::place_unplaced(&mut s);
        notes.extend(said);
        notes.extend(s.unplaced_takes.iter().map(|t| format!("{} {t}", transit_model::UNPLACED_NOTE)));
        as_saved = placed.then_some(before);
    }
    log::info!("{}", session_open_line(path, &s, missing.len(), started.elapsed()));
    for line in unknown_param_lines(&s) {
        log::info!("{line}");
    }
    e.replace_session(s);
    if let Some(before) = stored.or(as_saved) {
        e.saved = Arc::new(before);
        e.dirty = true;
        e.revision = e.revision.wrapping_add(1);
    }
    e.path = Some(path.to_string());
    for n in notes {
        e.message(n);
    }
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    crate::takes::migration::on_open(e);
    Ok(missing)
}

/// One INFO line per stored parameter a built-in instrument or effect no longer has (an id renamed
/// or removed, such as the Macro Oscillator's `hardware_tuning`, #195): the session still opens,
/// the value stays in the document (a newer build may know it) and the device ignores it, as
/// `Plugin::set_param` refuses an unknown id. Hosted plug-ins report their own parameters and are
/// not checked.
pub fn unknown_param_lines(s: &Session) -> Vec<String> {
    let mut out = Vec::new();
    for t in &s.tracks {
        for ins in t.instrument.iter().chain(t.mixer.inserts.iter().flatten()) {
            if transit_mix::is_third_party(&ins.plugin) {
                continue;
            }
            let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { continue };
            for id in ins.params.keys().filter(|id| info.param(id).is_none()) {
                out.push(format!("session: track \"{}\": {} ignores the unknown parameter `{id}`", t.name, info.name));
            }
        }
    }
    out
}

/// The INFO line an opened session writes (issue #84): `session opened: path="/x/song.transit"
/// format_version=1 tracks=8 clips=23 duration=205.4 s needs=[clap:..., vst3:...] missing_media=0
/// in 340 ms`. `needs` lists the third-party plug-ins the session loads (`none` when it has no
/// such plug-in).
pub fn session_open_line(path: &str, s: &Session, missing_media: usize, took: std::time::Duration) -> String {
    let clips: usize = s.tracks.iter().filter_map(|t| t.playlist()).map(|p| p.clips.len()).sum();
    let seconds = s.content_end().max(0) as f64 / f64::from(s.sample_rate.hz().max(1));
    // Ids only: `instance_specs` would decode every plug-in's stored state to build its specs.
    let mut needs: Vec<String> = s
        .tracks
        .iter()
        .flat_map(|t| t.mixer.inserts.iter().flatten().chain(t.instrument.iter()))
        .filter(|i| i.hardware.is_none() && transit_mix::is_third_party(&i.plugin))
        .map(|i| i.plugin.clone())
        .collect();
    needs.sort();
    needs.dedup();
    let needs = if needs.is_empty() { "none".to_owned() } else { format!("[{}]", needs.join(", ")) };
    format!(
        "session opened: path=\"{path}\" format_version={} tracks={} clips={clips} duration={seconds:.1} s needs={needs} missing_media={missing_media} in {} ms",
        s.format_version,
        s.tracks.len(),
        took.as_millis()
    )
}

/// Decode one source's audio file at the session rate.
fn decode_source(bytes: &[u8], ext: Option<&str>, rate: u32) -> std::result::Result<Arc<SourceAudio>, String> {
    let (_, buf) = transit_audio_io::decode(bytes, ext).map_err(|err| err.to_string())?;
    let buf = at_rate(buf, rate, MAX_IMPORT_SAMPLES).ok_or("too large to load")?;
    Ok(Arc::new(SourceAudio::new(buf)))
}

/// Audio files decoded so far, by path: a session may name one file thousands of times.
/// The one name of the file at `p` (links followed), so the same file named two ways is read once.
pub(crate) fn file_key(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub(crate) type DecodedFiles = std::collections::HashMap<PathBuf, std::result::Result<Arc<SourceAudio>, String>>;

/// The audio of the file at `p` at `rate`, decoded the first time it is asked for. The error
/// names the file.
/// A file named in a session is hostile input too: a path that leaves the read root is not read.
/// Played from disk (ADR 0035) when `stream` says so or the file is too long to decode at all.
pub(crate) fn decode_file_streamed(files: &mut DecodedFiles, p: &Path, rate: u32, stream: bool) -> std::result::Result<Arc<SourceAudio>, String> {
    crate::file_access::check_read(p)?;
    files
        .entry(file_key(p))
        .or_insert_with(|| {
            if let Some((info, len)) = super::disk::probe(p).filter(|(info, _)| stream || super::disk::over_decode_limit(info)) {
                match super::disk::open_disk_audio(p, len, rate) {
                    Ok(audio) => return Ok(audio),
                    Err(e) if super::disk::over_decode_limit(&info) => return Err(e),
                    Err(e) => log::info!("{e}; it is decoded instead"),
                }
            }
            let bytes = read_file(p).map_err(|_| p.display().to_string())?;
            decode_source(&bytes, p.extension().and_then(|x| x.to_str()), rate).map_err(|err| format!("{}: {err}", p.display()))
        })
        .clone()
}

// ─── Bundles: a session and its audio as one file ─────────────────────────────────────────────

/// Extension of a session bundle: one zip file holding the `.transit` session and its
/// `Audio Files/`, the same layout as a saved session folder.
pub use transit_model::BUNDLE_EXTENSION;

/// Most entries and bytes a bundle may unpack to (bundles are hostile input).
const MAX_BUNDLE_ENTRIES: usize = 10_000;
const MAX_BUNDLE_BYTES: u64 = 1 << 30;

/// Write one output file to `path`, through the checked file access.
pub fn write_file(path: &str, bytes: &[u8]) -> Result<()> {
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{path}: {err}")))
}

/// The session as a bundle: `<name>.transit` plus every source's audio as 32-bit float WAV under
/// `Audio Files/` (stored, not compressed: audio barely compresses). Returns the bytes and the
/// session with its source paths pointing into the bundle.
pub fn session_bundle(session: &Session) -> Result<(Vec<u8>, Session)> {
    use std::io::Write as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let ioerr = |err: std::io::Error| EngineError::Io(format!("bundle: {err}"));
    let mut s = session.clone();
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let mut taken = std::collections::HashSet::new();
    for src in &mut s.sources {
        let Some(audio) = s.pool.get(src.id) else { continue };
        let base = sanitize_name(&src.name);
        let mut target = format!("Audio Files/{base}.wav");
        let mut k = 1;
        while taken.contains(&target) && k <= 9_999 {
            target = format!("Audio Files/{base}_{k:02}.wav");
            k += 1;
        }
        let bytes = match &audio.disk {
            // A disk-backed source goes in as its file, as long as a bundle can hold it.
            Some(d) => {
                let len = std::fs::metadata(d.path()).map_err(ioerr)?.len();
                if len > MAX_BUNDLE_BYTES {
                    return Err(EngineError::Io(format!("bundle: {} is too long to bundle; save the session as a folder", d.path().display())));
                }
                read_file(d.path())?
            }
            None => {
                let opts_wav = EncodeOptions { format: FileFormat::Wav, bit_depth: transit_audio_io::BitDepth::Float32, dither: false, bwf: None };
                transit_audio_io::encode(&audio.buffer, &opts_wav).map_err(|err| EngineError::Io(err.to_string()))?
            }
        };
        w.start_file(target.as_str(), opts).map_err(zerr)?;
        w.write_all(&bytes).map_err(ioerr)?;
        taken.insert(target.clone());
        src.path = target;
        src.unsaved = false;
        src.format = FileFormat::Wav;
    }
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    w.start_file(format!("{}.{}", sanitize_name(&s.name), transit_model::SESSION_EXTENSION), opts).map_err(zerr)?;
    w.write_all(text.as_bytes()).map_err(ioerr)?;
    let bytes = w.finish().map_err(zerr)?.into_inner();
    Ok((bytes, s))
}

/// Open a session from bytes (for hosts that have no paths): a bundle ([`session_bundle`]) or a bare
/// `.transit`, whose audio is then reported missing. Returns the missing media, like
/// [`open_session`].
pub fn open_session_bytes(e: &mut Engine, name: &str, bytes: &[u8]) -> Result<Vec<String>> {
    let (text, mut files) = if bytes.starts_with(b"PK\x03\x04") {
        read_bundle(bytes)?
    } else {
        (String::from_utf8(bytes.to_vec()).map_err(|_| EngineError::Io(format!("{name}: not a transit session")))?, std::collections::HashMap::new())
    };
    let (mut s, mut stored) = Session::from_json_as_saved(&text).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    let mut missing = Vec::new();
    for src in &s.sources {
        match files.remove(&src.path) {
            Some(audio) => match decode_source(&audio, Path::new(&src.path).extension().and_then(|x| x.to_str()), s.sample_rate.hz()) {
                Ok(audio) => s.pool.insert(src.id, audio),
                Err(err) => missing.push(format!("{}: {err}", src.path)),
            },
            None => missing.push(src.path.clone()),
        }
    }
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).filter(|x| !x.is_empty()).unwrap_or("Untitled").to_string();
    let notes = std::mem::take(&mut s.load_notes);
    for line in unknown_param_lines(&s) {
        log::info!("{line}");
    }
    if let Some(st) = &mut stored {
        st.pool = s.pool.clone();
    }
    e.replace_session(s);
    if let Some(before) = stored {
        e.saved = Arc::new(before);
        e.dirty = true;
        e.revision = e.revision.wrapping_add(1);
    }
    for n in notes {
        e.message(n);
    }
    e.path = Some(format!("{}.{BUNDLE_EXTENSION}", sanitize_name(&stem)));
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    crate::takes::migration::on_open(e);
    Ok(missing)
}

/// Unpack a bundle: the one `.transit` at its top level and the files beside it, by relative path.
fn read_bundle(bytes: &[u8]) -> Result<(String, std::collections::HashMap<String, Vec<u8>>)> {
    use std::io::Read as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zerr)?;
    if z.len() > MAX_BUNDLE_ENTRIES {
        return Err(crate::cmd::bad("session.open", "bundle: too many files"));
    }
    let (mut text, mut files, mut total) = (None, std::collections::HashMap::new(), 0u64);
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(zerr)?;
        if f.is_dir() {
            continue;
        }
        let rel = f.name().map_err(zerr)?.to_string();
        let size = f.size();
        total = total.saturating_add(size);
        if total > MAX_BUNDLE_BYTES {
            return Err(crate::cmd::bad("session.open", "bundle: too large"));
        }
        let mut data = Vec::new();
        // The declared size is hostile too: never read past it.
        f.by_ref().take(size).read_to_end(&mut data).map_err(|err| EngineError::Io(format!("bundle: {rel}: {err}")))?;
        let is_session = !rel.contains('/') && transit_model::is_session_file(Path::new(&rel));
        if is_session && text.is_none() {
            text = Some(String::from_utf8(data).map_err(|_| EngineError::Io(format!("bundle: {rel} is not text")))?);
        } else {
            files.insert(rel, data);
        }
    }
    let text = text.ok_or_else(|| crate::cmd::bad("session.open", "bundle: no .transit session inside"))?;
    Ok((text, files))
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn autosave_copy_leaves_document_untouched_and_reuses_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let dir = std::env::temp_dir().join(format!("soundcraft-autosave-{}", std::process::id()));
        let path = dir.join("Midnight Groove.transit");
        let p = path.to_string_lossy().into_owned();
        let n1 = save_copy(&e, &p).unwrap();
        let n2 = save_copy(&e, &p).unwrap();
        assert!(n1 >= 5);
        assert_eq!(n2, 0, "second autosave must not rewrite audio");
        assert!(e.is_dirty() && e.path.is_none());
        let mut r = Engine::default();
        open_session(&mut r, &p).unwrap();
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn with_take(level: f32) -> Engine {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let src = add_source(s, "Audio 1_02", AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; 4_800]] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, "Audio 1_02", src, 0, 0, 4_800));
        e
    }

    fn reopen(path: &str) -> Engine {
        let mut r = Engine::default();
        assert_eq!(open_session(&mut r, path).unwrap(), Vec::<String>::new());
        r
    }

    #[test]
    fn saving_never_replaces_audio_of_another_session() {
        let dir = std::env::temp_dir().join(format!("soundcraft-no-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = |p: &str| dir.join(p).to_string_lossy().into_owned();
        save_session(&mut with_take(0.5), &path("A/A.transit")).unwrap();
        save_session(&mut with_take(-0.25), &path("A/A v2.transit")).unwrap();
        save_session(&mut with_take(0.75), &path("B/B.transit")).unwrap();
        save_session(&mut reopen(&path("B/B.transit")), &path("A/B.transit")).unwrap();
        let levels = ["A/A.transit", "A/A v2.transit", "B/B.transit", "A/B.transit"].map(|p| {
            let r = reopen(&path(p));
            r.session().pool.get(r.session().sources[0].id).unwrap().buffer.channels[0][0]
        });
        assert_eq!(levels, [0.5, -0.25, 0.75, 0.75]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consolidating_twice_keeps_both_renders() {
        let dir = std::env::temp_dir().join(format!("soundcraft-consolidate-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.transit").to_string_lossy().into_owned();
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        let end = e.session().content_end();
        let render = |e: &Engine| transit_mix::render_clips(e.session(), kick, Range::new(0, end));
        e.execute("edit.consolidate", &json!({"tracks": [kick.0], "start": 500_000, "end": 560_000})).unwrap();
        save_session(&mut e, &path).unwrap();
        e.execute("edit.consolidate", &json!({"tracks": [kick.0], "start": 1_400_000, "end": 1_500_000})).unwrap();
        save_session(&mut e, &path).unwrap();
        assert!(render(&reopen(&path)) == render(&e));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_after_undo_reuses_the_saved_audio() {
        let dir = std::env::temp_dir().join(format!("soundcraft-undo-resave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.transit").to_string_lossy().into_owned();
        let mut e = with_take(0.5);
        e.execute("track.new", &json!({})).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 0);
        assert_eq!(std::fs::read_dir(dir.join("Audio Files")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_audio_is_trusted_only_in_the_folder_it_was_saved_in() {
        let dir = std::env::temp_dir().join(format!("soundcraft-own-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let level = |r: &Engine| r.session().pool.get(r.session().sources[0].id).unwrap().buffer.channels[0][0];
        save_session(&mut with_take(-0.25), &path("B/Other.transit")).unwrap();
        let mut e = with_take(0.5);
        save_session(&mut e, &path("A/A.transit")).unwrap();
        e.execute("track.new", &json!({})).unwrap();
        save_session(&mut e, &path("B/B.transit")).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(level(&reopen(&path("B/B.transit"))), 0.5);
        e.execute("session.save_copy", &json!({"path": path("A/Copy.transit")})).unwrap();
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(level(&reopen(&path("B/B.transit"))), 0.5);
        std::fs::remove_file(dir.join("A/Audio Files/Audio 1_02.wav")).unwrap();
        let mut missing = Engine::default();
        open_session(&mut missing, &path("A/A.transit")).unwrap();
        save_session(&mut missing, &path("B/Moved.transit")).unwrap();
        let mut r = Engine::default();
        assert_eq!(
            open_session(&mut r, &path("B/Moved.transit")).unwrap(),
            vec![dir.join("A").join("Audio Files").join("Audio 1_02.wav").display().to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_over_another_session_needs_replace() {
        let dir = std::env::temp_dir().join(format!("soundcraft-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.transit").to_string_lossy().into_owned();
        let mut first = with_take(0.5);
        first.execute("session.save_as", &json!({"path": path})).unwrap();
        first.execute("session.save", &json!({})).unwrap();
        first.execute("session.save_as", &json!({"path": dir.join("Song")})).unwrap();
        assert!(first.execute("session.save_template", &json!({"path": path})).is_err());
        let mut second = with_take(-0.25);
        let track = second.session().tracks[0].id.0;
        for id in ["session.save", "session.save_as", "session.save_copy", "session.save_template", "file.export_selected_tracks_as_session"] {
            let err = second.execute(id, &json!({"path": path, "tracks": [track]})).unwrap_err();
            assert!(err.to_string().contains("`replace: true`"), "{id}: {err}");
        }
        assert_eq!(std::fs::read_dir(dir.join("Audio Files")).unwrap().count(), 1);
        Engine::default().execute("session.save_as", &json!({"path": dir.join("New/New.transit")})).unwrap();
        std::fs::copy(&path, dir.join("Song")).unwrap();
        let mut bare = Engine::default();
        open_session(&mut bare, &dir.join("Song").to_string_lossy()).unwrap();
        bare.execute("session.save", &json!({})).unwrap();
        second.execute("session.save_as", &json!({"path": path, "replace": true})).unwrap();
        let r = reopen(&path);
        assert_eq!(r.session().pool.get(r.session().sources[0].id).unwrap().buffer.channels[0][0], -0.25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_keeps_the_last_ten_versions_as_backups() {
        let dir = std::env::temp_dir().join(format!("soundcraft-backups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Song.transit");
        let mut e = Engine::default();
        let mut versions = Vec::new();
        for i in 0..12 {
            e.execute("track.new", &json!({"name": format!("T{i}")})).unwrap();
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            versions.push(std::fs::read(&path).unwrap());
        }
        let backup = |n: usize| std::fs::read(dir.join(format!("Backups/Song backup {n}.transit"))).ok();
        assert_eq!(std::fs::read_dir(dir.join("Backups")).unwrap().count(), 10);
        assert_eq!((backup(1), backup(2), backup(11)), (None, Some(versions[1].clone()), Some(versions[10].clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_gives_sources_that_share_a_path_their_own_files() {
        let mut s = Session::default();
        let loud = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 64]] };
        let quiet = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 64]] };
        add_source(&mut s, "tone", loud, Some("Audio Files/tone.wav"), FileFormat::Wav);
        add_source(&mut s, "tone", quiet, Some("Audio Files/tone.wav"), FileFormat::Wav);
        let mut e = Engine::new(s);
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Shared.transit");
        save_session(&mut e, &path.to_string_lossy()).unwrap();
        let paths: Vec<_> = e.session().sources.iter().map(|src| src.path.clone()).collect();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1], "{paths:?}");
        let a = std::fs::read(dir.join(&paths[0])).unwrap();
        let b = std::fs::read(dir.join(&paths[1])).unwrap();
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[test]
    fn a_file_named_by_many_sources_is_decoded_once() {
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("k.wav"), transit_audio_io::encode(&audio, &EncodeOptions::default()).unwrap()).unwrap();
        let mut s = Session::default();
        for _ in 0..3 {
            add_source(&mut s, "k", audio.clone(), Some("k.wav"), FileFormat::Wav);
        }
        let path = dir.join("Song.transit").to_string_lossy().into_owned();
        std::fs::write(&path, s.to_json().unwrap()).unwrap();
        let mut e = Engine::default();
        assert!(open_session(&mut e, &path).unwrap().is_empty());
        let (sources, pool) = (&e.session().sources, &e.session().pool);
        let first = pool.get(sources[0].id).unwrap();
        assert!(sources.iter().all(|src| pool.get(src.id).is_some_and(|a| Arc::ptr_eq(a, first))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sources_playing_one_file_share_it_when_saved() {
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Audio Files")).unwrap();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("Audio Files/k.wav"), transit_audio_io::encode(&audio, &EncodeOptions::default()).unwrap()).unwrap();
        let mut s = Session::default();
        for _ in 0..3 {
            add_source(&mut s, "k", audio.clone(), Some("Audio Files/k.wav"), FileFormat::Wav);
        }
        s.sources.iter_mut().for_each(|src| src.unsaved = false);
        let path = dir.join("Song.transit").to_string_lossy().into_owned();
        std::fs::write(&path, s.to_json().unwrap()).unwrap();
        let mut e = Engine::default();
        open_session(&mut e, &path).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 0);
        assert!(e.session().sources.iter().all(|src| src.path == "Audio Files/k.wav"));
        assert_eq!(save_copy(&e, &dir.join("copy/Song.transit").to_string_lossy()).unwrap(), 1);
        assert_eq!(std::fs::read_dir(dir.join("copy/Audio Files")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn stored_media_paths_are_followed_only_on_this_computer() {
        let dir = std::env::temp_dir().join(format!("soundcraft-stored-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("k.wav"), transit_audio_io::encode(&audio, &EncodeOptions::default()).unwrap()).unwrap();
        let mut saved = Session::default();
        let t = saved.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let src = add_source(&mut saved, "k", audio, Some("../k.wav"), FileFormat::Wav);
        let id = saved.new_clip_id();
        crate::edit::place_clip(&mut saved, t, Clip::audio(id, "k", src, 0, 0, 480));
        // In a folder of its own, so nothing beside the session is found by name.
        std::fs::create_dir_all(dir.join("s")).unwrap();
        let path = dir.join("s").join("Song.transit").to_string_lossy().into_owned();
        let mut stored = vec![r"\\192.0.2.1\share\x.wav".to_string(), "file://192.0.2.1/s/x.wav".to_string()];
        // Linux reads `//tmp/…` as the local `/tmp/…`, so only the path check keeps it from being read.
        if cfg!(unix) {
            stored.push(format!("/{}", dir.join("k.wav").display()));
        }
        let refused = |m: &str| m.contains("not opened: the path is on another computer");
        for p in stored {
            let mut s = saved.clone();
            s.sources[0].path.clone_from(&p);
            std::fs::write(&path, s.to_json().unwrap()).unwrap();
            let mut r = Engine::default();
            let missing = open_session(&mut r, &path).unwrap();
            assert_eq!(missing.iter().filter(|m| refused(m)).count(), 1, "{p}: {missing:?}");
            assert!(r.session().pool.get(r.session().sources[0].id).is_none(), "{p}");
            let mut into = Engine::default();
            let imported = into.execute("file.import_session_data", &json!({"path": path})).unwrap();
            assert!(imported["missing"].as_array().unwrap().iter().any(|m| m.as_str().is_some_and(refused)), "{p}: {imported}");
            assert!(into.session().pool.get(into.session().sources[0].id).is_none(), "{p}");
        }
        // A path relative to the session opens.
        std::fs::write(&path, saved.to_json().unwrap()).unwrap();
        let mut r = Engine::default();
        assert!(open_session(&mut r, &path).unwrap().is_empty());
        assert!(r.session().pool.get(r.session().sources[0].id).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    #[test]
    fn bundle_round_trips_session_and_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
        assert!(fresh.sources.iter().all(|s| s.path.starts_with("Audio Files/") && !s.unsaved));
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Midnight Groove.transitz", &bytes).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        assert_eq!(r.session().tracks.len(), e.session().tracks.len());
        for src in &e.session().sources {
            let (a, b) = (e.session().pool.get(src.id).unwrap(), r.session().pool.get(src.id).unwrap());
            assert_eq!(a.buffer.channels, b.buffer.channels, "{}: float WAV keeps every sample", src.name);
        }
        assert!(!r.is_dirty());
        assert_eq!(r.path.as_deref(), Some("Midnight Groove.transitz"));
    }

    #[test]
    fn same_named_sources_get_distinct_files() {
        let mut e = crate::demo::demo_engine();
        let name = e.session().sources[0].name.clone();
        let s = e.session_mut();
        let n = s.sources.len();
        s.sources[1].name = name;
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        let paths: std::collections::HashSet<_> = fresh.sources.iter().map(|s| s.path.clone()).collect();
        assert_eq!(paths.len(), n);
        let mut r = Engine::default();
        assert!(open_session_bytes(&mut r, "x.transitz", &bytes).unwrap().is_empty());
    }

    #[test]
    fn bare_session_opens_with_its_audio_reported_missing() {
        let e = crate::demo::demo_engine();
        let (_, fresh) = session_bundle(e.session()).unwrap();
        let text = fresh.to_json().unwrap();
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Song.transit", text.as_bytes()).unwrap();
        assert_eq!(missing.len(), fresh.sources.len());
        assert_eq!(r.session().tracks.len(), fresh.tracks.len());
    }

    #[test]
    fn hostile_bundles_are_errors_not_panics() {
        let mut r = Engine::default();
        for bad in [&b"PK\x03\x04 not really a zip"[..], b"", b"{ not json", b"\xff\xfe\x00"] {
            assert!(open_session_bytes(&mut r, "bad.transitz", bad).is_err());
        }
        // A valid zip without a session inside.
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("Audio Files/a.wav", zip::write::SimpleFileOptions::default()).unwrap();
        let only_audio = w.finish().unwrap().into_inner();
        assert!(open_session_bytes(&mut r, "a.transitz", &only_audio).is_err());
        assert_eq!(r.session().tracks.len(), 0, "a failed open leaves the document alone");
    }

    #[test]
    fn write_file_writes_to_disk_natively() {
        let p = std::env::temp_dir().join(format!("soundcraft-write-file-{}.txt", std::process::id()));
        write_file(&p.to_string_lossy(), b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(test)]
mod open_line_tests {
    use super::*;

    #[test]
    fn the_session_open_line_names_path_version_counts_length_and_needs() {
        let mut s = Session::new("Song", transit_time::SampleRate::HZ_48000);
        let line = session_open_line("C:/x/song.transit", &s, 0, std::time::Duration::from_millis(340));
        assert_eq!(
            line,
            "session opened: path=\"C:/x/song.transit\" format_version=1 tracks=0 clips=0 duration=0.0 s needs=none missing_media=0 in 340 ms"
        );
        s.sample_rate = transit_time::SampleRate::HZ_48000;
        let line = session_open_line("a.transit", &s, 2, std::time::Duration::from_millis(5));
        assert!(line.contains("missing_media=2") && line.ends_with("in 5 ms"), "{line}");
    }

    #[test]
    fn a_session_with_tracks_and_clips_counts_them() {
        let e = crate::demo::demo_engine();
        let s = e.session();
        let line = session_open_line("demo.transit", s, 0, std::time::Duration::ZERO);
        let tracks = s.tracks.len();
        assert!(tracks > 0, "the demo has tracks");
        assert!(line.contains(&format!("tracks={tracks} ")), "{line}");
        assert!(!line.contains("clips=0 "), "{line}");
        assert!(!line.contains("duration=0.0 s"), "{line}");
    }

    #[test]
    fn needs_lists_third_party_ids_once_without_reading_their_state() {
        let mut s = crate::demo::demo_engine().session().clone();
        let prefix = transit_clap_host::ID_PREFIX;
        let mut ins = transit_model::Insert::new(format!("{prefix}one"));
        ins.state = Some("!!! not base64 !!!".into());
        let track = s.tracks.first_mut().expect("a track");
        track.mixer.inserts.push(Some(ins.clone()));
        track.mixer.inserts.push(Some(ins));
        let line = session_open_line("a.transit", &s, 0, std::time::Duration::ZERO);
        assert!(line.contains(&format!("needs=[{prefix}one]")), "{line}");
    }
}
