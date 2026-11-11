//! The portable kit form (ADR 0041, amendment 2026-11-11 "kit presets", #467): the session's
//! Sampler patch serialization with each source named by its file instead of a session source id.
//! Pure: no file is read or written here; the engine resolves, admits and writes.
//!
//! - **One format.** [`to_portable`] serializes the patch with the session's own serde code and
//!   replaces each `sources` entry's `source` id (and its `ownership`) with a `file` object: the
//!   sample's `path` and the facts the session's [`crate::Source`] records (`name`, `frames`,
//!   `channels`, `sample_rate`, `format`). An opaque patch has its raw `sources` rewritten the same
//!   way, so it still carries no session id. [`from_portable`] takes the `file` objects out again;
//!   the engine gives each entry its new session id and reads the value with
//!   [`SamplerPatch::from_value`] ([`with_sources`]), so schema handling, migration, validation and
//!   kept unknown fields are the session's.
//! - **Paths** ([`sample_path`], [`relative`]): the Windows extended-length prefix (`\\?\`,
//!   `\\?\UNC\`) is removed first; a sample in or below the preset file's folder is written relative
//!   with `/` (compared by whole components, without regard to case for drive and share paths);
//!   anything else is written absolute, as the session holds it. Reading goes through the engine's
//!   stored-path check against the preset file.
//! - **The file** ([`file`]): `{"format": "transit-sampler-kit", "version": 1, "kit": ..}`, with the
//!   extension `.tkit` appended, never replacing another extension ([`kit_file_path`]).

use crate::sampler_patch::{MAX_LABEL_BYTES, MAX_SOURCES, SCHEMA, SamplerPatch};
use crate::{Session, Source, SourceId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use transit_audio_io::FileFormat;

/// The kit file's format tag (ADR 0049).
pub const FORMAT: &str = "transit-sampler-kit";
/// The kit file's format version.
pub const VERSION: u64 = 1;
/// The kit file's extension.
pub const EXTENSION: &str = "tkit";

/// A sample a portable source entry names: its path and what the saving session recorded about it.
/// Every field has a default (the new-field rule): a hand-written entry without its facts records
/// 0 frames at 0 Hz, which no file matches, so it loads in the missing state; one without a path
/// is not found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileRef {
    pub path: String,
    pub name: String,
    pub frames: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub format: FileFormat,
}

impl Default for FileRef {
    fn default() -> Self {
        FileRef { path: String::new(), name: String::new(), frames: 0, channels: 0, sample_rate: 0, format: FileFormat::Other }
    }
}

/// One source entry of a portable kit, by index.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A file reference (the amendment's form).
    File(FileRef),
    /// A session source id alone: a track preset written before the amendment.
    Session(SourceId),
}

/// One sample a saved kit names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// The index in the patch's `sources`.
    pub index: usize,
    pub source: SourceId,
    /// The path as written.
    pub path: String,
    /// Written relative to the preset file's folder.
    pub relative: bool,
    /// The session's own audio (a path relative to the session folder): the kit depends on that
    /// session folder.
    pub managed: bool,
    /// Listed by the session with no audio in the pool (saved with its stored path and facts).
    pub missing: bool,
    /// On a network share other than the preset file's: it will be missing on recall.
    pub other_share: bool,
}

/// A patch in the portable form.
#[derive(Debug, Clone, PartialEq)]
pub struct Portable {
    /// The portable patch (the file's `kit`).
    pub kit: Value,
    pub files: Vec<Saved>,
}

/// Who uses a source, for a save refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum User {
    Pad(u8),
    Zone(u16),
    Loop,
    Unknown,
}

/// Why a patch cannot be saved as a kit (nothing is written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveRefusal {
    /// A source with no file of its own: unsaved managed audio, or no session record.
    NoFile { user: User, name: String },
    /// A path longer than [`MAX_LABEL_BYTES`].
    TooLong { user: User, name: String },
}

impl SaveRefusal {
    /// The refusal in the user's words.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            SaveRefusal::NoFile { user, name } => match user {
                User::Pad(p) => format!(
                    "Pad {}'s sample \"{name}\" has no file yet: save the session, or load the sample onto the pad from a file.",
                    crate::pad_notes::pad_label(*p)
                ),
                User::Zone(z) => {
                    format!("Zone {}'s sample \"{name}\" has no file yet: save the session, or load the sample onto the zone from a file.", z + 1)
                }
                User::Loop => format!("The slice loop's sample \"{name}\" has no file yet: save the session, or load the sample from a file."),
                User::Unknown => format!("The sample \"{name}\" has no file yet: save the session, or load the sample from a file."),
            },
            SaveRefusal::TooLong { name, .. } => format!("The path of the sample \"{name}\" is longer than {MAX_LABEL_BYTES} bytes."),
        }
    }
}

/// Why a portable kit is not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRefusal {
    /// A kit schema above this build's: "saved by a newer transit".
    NewerSchema(u64),
    /// Not a kit this build reads, with the reason.
    Invalid(String),
}

impl ReadRefusal {
    /// The refusal in the user's words.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            ReadRefusal::NewerSchema(n) => format!("saved by a newer transit (Sampler kit schema {n})"),
            ReadRefusal::Invalid(why) => format!("not a valid Sampler kit ({why})"),
        }
    }
}

// ─── Paths ────────────────────────────────────────────────────────────────────────────────────

/// The Windows extended-length prefix removed: `\\?\C:\x` is `C:\x`, `\\?\UNC\host\share\x` is
/// `\\host\share\x`. Other paths are returned as they are.
#[must_use]
pub fn strip_verbatim(path: &str) -> String {
    let b = path.as_bytes();
    if b.len() >= 8 && path.starts_with(r"\\?\") && b[4..7].eq_ignore_ascii_case(b"UNC") && b[7] == b'\\' {
        return format!(r"\\{}", &path[8..]);
    }
    path.strip_prefix(r"\\?\").map_or_else(|| path.to_string(), str::to_string)
}

/// The root of an absolute path, in either platform's form.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Root {
    /// `/`.
    Unix,
    /// `C:\`, the letter upper case.
    Drive(u8),
    /// `\\host\share`.
    Share(String, String),
}

impl Root {
    fn same(&self, other: &Root) -> bool {
        match (self, other) {
            (Root::Share(h, s), Root::Share(i, t)) => h.eq_ignore_ascii_case(i) && s.eq_ignore_ascii_case(t),
            _ => self == other,
        }
    }
    /// Drive and share paths are Windows paths: compared without regard to case.
    fn folds_case(&self) -> bool {
        !matches!(self, Root::Unix)
    }
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// An absolute path (after [`strip_verbatim`]) split into its root and components, `.` and `..`
/// folded lexically; `None` for a relative path, a device path or one that climbs above its root.
fn parse(path: &str) -> Option<(Root, Vec<String>)> {
    let b = path.as_bytes();
    let (root, rest) = if b.len() >= 2 && is_sep(char::from(b[0])) && is_sep(char::from(b[1])) {
        let mut it = path[2..].splitn(3, is_sep);
        let host = it.next().filter(|h| !h.is_empty() && *h != "?" && *h != ".")?;
        let share = it.next().filter(|s| !s.is_empty())?;
        (Root::Share(host.to_string(), share.to_string()), it.next().unwrap_or(""))
    } else if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && is_sep(char::from(b[2])) {
        (Root::Drive(b[0].to_ascii_uppercase()), &path[3..])
    } else if b.first() == Some(&b'/') {
        (Root::Unix, &path[1..])
    } else {
        return None;
    };
    let mut parts: Vec<String> = Vec::new();
    for c in rest.split(is_sep) {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c.to_string()),
        }
    }
    Some((root, parts))
}

/// Whether `path` is absolute in either platform's form (`/x`, `C:\x`, `\\host\share\x`, with or
/// without the extended-length prefix).
#[must_use]
pub fn is_absolute(path: &str) -> bool {
    parse(&strip_verbatim(path)).is_some()
}

/// `sample` relative to the folder `folder` when it is in or below it, with `/` separators (never
/// `..`, a drive or a root); `None` otherwise. Both are absolute paths; the extended-length prefix
/// is removed first and whole components are compared, drive and share paths without regard to
/// case.
#[must_use]
pub fn relative(sample: &str, folder: &str) -> Option<String> {
    let (sr, sp) = parse(&strip_verbatim(sample))?;
    let (fr, fp) = parse(&strip_verbatim(folder))?;
    if !sr.same(&fr) || sp.len() <= fp.len() {
        return None;
    }
    let fold = sr.folds_case();
    let same = |a: &String, b: &String| if fold { a == b || a.to_lowercase() == b.to_lowercase() } else { a == b };
    fp.iter().zip(&sp).all(|(a, b)| same(a, b)).then(|| sp[fp.len()..].join("/"))
}

/// Whether the absolute paths `a` and `b` name the same file as [`relative`] compares them:
/// prefix removed, whole components, drive and share paths without regard to case.
#[must_use]
pub fn same_path(a: &str, b: &str) -> bool {
    match (parse(&strip_verbatim(a)), parse(&strip_verbatim(b))) {
        (Some((ra, pa)), Some((rb, pb))) => {
            let fold = ra.folds_case();
            ra.same(&rb) && pa.len() == pb.len() && pa.iter().zip(&pb).all(|(x, y)| x == y || (fold && x.to_lowercase() == y.to_lowercase()))
        }
        _ => false,
    }
}

/// Whether `sample` is on a network share other than the one `folder` is on.
#[must_use]
pub fn other_share(sample: &str, folder: &str) -> bool {
    match (parse(&strip_verbatim(sample)), parse(&strip_verbatim(folder))) {
        (Some((s @ Root::Share(..), _)), Some((f, _))) => !s.same(&f),
        (Some((Root::Share(..), _)), None) => true,
        _ => false,
    }
}

/// The absolute path of `source`'s file, prefix removed: an absolute stored path as stored, a
/// relative one (the session's own `Audio Files/` audio) joined to the session folder in that
/// folder's form. `None` when the path is relative and the session has no folder.
#[must_use]
pub fn sample_path(source: &Source, session_folder: Option<&Path>) -> Option<String> {
    let stored = strip_verbatim(&source.path);
    if is_absolute(&stored) {
        return Some(stored);
    }
    if stored.is_empty() {
        return None;
    }
    join(&session_folder?.to_string_lossy(), &stored)
}

/// The relative path `rel` joined as text to the absolute folder `folder`, in that folder's form
/// (`\` for drive and share paths, else `/`), the folder's prefix removed; `None` when `folder` is
/// not absolute.
#[must_use]
pub fn join(folder: &str, rel: &str) -> Option<String> {
    let folder = strip_verbatim(folder);
    let sep = match parse(&folder) {
        Some((root, _)) if root.folds_case() => '\\',
        Some(_) => '/',
        None => return None,
    };
    let rel: String = rel.chars().map(|c| if is_sep(c) { sep } else { c }).collect();
    Some(format!("{}{sep}{}", folder.trim_end_matches(is_sep), rel.trim_start_matches(sep)))
}

/// `given` with the kit extension: kept when it already ends in `.tkit` (any case), else `.tkit`
/// appended, never replacing another extension (`808 v1.5` is `808 v1.5.tkit`).
#[must_use]
pub fn kit_file_path(given: &Path) -> PathBuf {
    if given.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION)) {
        return given.to_path_buf();
    }
    let mut s = given.as_os_str().to_os_string();
    s.push(".");
    s.push(EXTENSION);
    PathBuf::from(s)
}

// ─── The portable form ────────────────────────────────────────────────────────────────────────

/// The first user of source index `index` in a ready patch: its pad, else its zone (lowest id),
/// else the slice loop.
fn user_of(patch: &SamplerPatch, index: usize) -> User {
    let Some(p) = patch.ready() else { return User::Unknown };
    let uses = |z: u16| p.zone(z).is_some_and(|z| usize::from(z.source) == index);
    if let Some(pad) = p.pads.iter().find(|q| q.zone.is_some_and(uses)) {
        return User::Pad(pad.index);
    }
    if let Some(z) = p.zones.iter().filter(|z| usize::from(z.source) == index).map(|z| z.id).min() {
        return User::Zone(z);
    }
    if p.slicing.as_ref().is_some_and(|l| usize::from(l.source) == index) {
        return User::Loop;
    }
    User::Unknown
}

/// The display name of a source's file: its file name, else the session's name.
fn file_label(source: &Source) -> String {
    let stored = source.path.rsplit(is_sep).next().unwrap_or("");
    if stored.is_empty() { source.name.clone() } else { stored.to_string() }
}

/// `patch` (ready or opaque) in the portable form, each source by its file. `preset_folder` is the
/// folder the kit file is written to (absolute); `session_folder` is where the session's own
/// relative paths point.
///
/// # Errors
/// [`SaveRefusal`]: a source with no file of its own, or a path too long.
pub fn to_portable(patch: &SamplerPatch, session: &Session, session_folder: Option<&Path>, preset_folder: &Path) -> Result<Portable, SaveRefusal> {
    let mut kit = serde_json::to_value(patch).unwrap_or(Value::Null);
    let folder = preset_folder.to_string_lossy().into_owned();
    let mut files = Vec::new();
    if let Some(sources) = kit.get_mut("sources").and_then(Value::as_array_mut) {
        for (index, entry) in sources.iter_mut().enumerate().take(MAX_SOURCES) {
            let Some(o) = entry.as_object_mut() else { continue };
            let Some(sid) = o.get("source").and_then(Value::as_u64).map(SourceId) else { continue };
            let user = user_of(patch, index);
            let Some(src) = session.source(sid) else {
                return Err(SaveRefusal::NoFile { user, name: format!("source {}", sid.0) });
            };
            let name = file_label(src);
            let path = sample_path(src, session_folder).filter(|_| !src.unsaved);
            let Some(path) = path else { return Err(SaveRefusal::NoFile { user, name }) };
            let rel = relative(&path, &folder);
            let written = rel.clone().unwrap_or_else(|| path.clone());
            if written.len() > MAX_LABEL_BYTES {
                return Err(SaveRefusal::TooLong { user, name });
            }
            let file = FileRef {
                path: written.clone(),
                name: src.name.clone(),
                frames: src.frames,
                channels: src.channels,
                sample_rate: src.sample_rate,
                format: src.format,
            };
            o.remove("source");
            o.remove("ownership");
            o.insert("file".into(), serde_json::to_value(&file).unwrap_or(Value::Null));
            files.push(Saved {
                index,
                source: sid,
                path: written,
                relative: rel.is_some(),
                managed: !is_absolute(&src.path),
                missing: !session.pool.contains(sid),
                other_share: other_share(&path, &folder),
            });
        }
    }
    Ok(Portable { kit, files })
}

/// The portable kit `kit` with its `file` objects taken out, and the entries by source index.
///
/// # Errors
/// [`ReadRefusal::NewerSchema`] for a kit schema above this build's; [`ReadRefusal::Invalid`] for
/// anything else this build does not read (not an object, a bad `sources` list, an entry with
/// neither or both of `file` and `source`, a malformed `file`, a path over 4096 bytes).
pub fn from_portable(mut kit: Value) -> Result<(Value, Vec<Entry>), ReadRefusal> {
    let invalid = |why: String| ReadRefusal::Invalid(why);
    let o = kit.as_object_mut().ok_or_else(|| invalid("the kit is not an object".into()))?;
    if let Some(v) = o.get("schema") {
        let n = v.as_u64().ok_or_else(|| invalid("the kit's schema is not a number".into()))?;
        if n > u64::from(SCHEMA) {
            return Err(ReadRefusal::NewerSchema(n));
        }
        if n != u64::from(SCHEMA) {
            return Err(invalid(format!("kit schema {n} is not {SCHEMA}")));
        }
    }
    let mut entries = Vec::new();
    let Some(sources) = o.get_mut("sources") else { return Ok((kit, entries)) };
    let sources = sources.as_array_mut().ok_or_else(|| invalid("`sources` is not a list".into()))?;
    if sources.len() > MAX_SOURCES {
        return Err(invalid(format!("{} sources (at most {MAX_SOURCES})", sources.len())));
    }
    for (i, entry) in sources.iter_mut().enumerate() {
        let n = i + 1;
        let e: &mut Map<String, Value> = entry.as_object_mut().ok_or_else(|| invalid(format!("source {n} is not an object")))?;
        match (e.remove("file"), e.get("source")) {
            (Some(_), Some(_)) => return Err(invalid(format!("source {n} names both a file and a session source"))),
            (None, None) => return Err(invalid(format!("source {n} names no file"))),
            (None, Some(id)) => {
                entries.push(Entry::Session(SourceId(id.as_u64().ok_or_else(|| invalid(format!("source {n}: the id is not a number")))?)));
            }
            (Some(file), None) => {
                let file: FileRef = serde_json::from_value(file).map_err(|err| invalid(format!("source {n}: {err}")))?;
                if file.path.len() > MAX_LABEL_BYTES || file.name.len() > MAX_LABEL_BYTES {
                    return Err(invalid(format!("source {n}: the path or name is longer than {MAX_LABEL_BYTES} bytes")));
                }
                entries.push(Entry::File(file));
            }
        }
    }
    Ok((kit, entries))
}

/// The patch of the portable value `kit` (from [`from_portable`]) with source `i` named by session
/// source `ids[i]`, referenced where it is, read as a session reads a patch.
#[must_use]
pub fn with_sources(mut kit: Value, ids: &[SourceId]) -> SamplerPatch {
    if let Some(sources) = kit.get_mut("sources").and_then(Value::as_array_mut) {
        for (entry, id) in sources.iter_mut().zip(ids) {
            if let Some(e) = entry.as_object_mut() {
                e.insert("source".into(), json!(id.0));
                e.insert("ownership".into(), json!("referenced"));
            }
        }
    }
    SamplerPatch::from_value(kit)
}

/// The kit file's JSON for the portable patch `kit`.
#[must_use]
pub fn file(kit: Value) -> Value {
    json!({"format": FORMAT, "version": VERSION, "kit": kit})
}

#[cfg(test)]
#[path = "sampler_portable_tests.rs"]
mod tests;
