//! The Sampler's kit preset files (ADR 0041, amendment 2026-11-11 "kit presets", #467), listed,
//! read, written and deleted through the checked file access, and the samples a kit names resolved
//! against the kit file. The commands are `mix.sampler_kit_save` and `mix.sampler_kit_load`
//! (`cmd/sampler_kit_presets.rs`); the UI lists, names and deletes kits here.
//!
//! - **Where:** `<config dir>/presets/sampler/<name>.tkit` ([`folder`], [`path`]): the `sampler`
//!   plug-in's user preset folder. The caller passes the settings folder (the existing
//!   configuration seam); nothing here knows where it is.
//! - **The file:** `{"format": "transit-sampler-kit", "version": 1, "kit": ..}` (ADR 0049 tags,
//!   checked before anything else is read), at most 8 MiB.
//! - **Writing** goes to a temporary file beside the target and then a rename, so a failed write
//!   never leaves half a kit; an existing file whose tag, version or kit schema this build does not
//!   read is never replaced.

use crate::file_tags::{Refusal, Tags};
use serde_json::Value;
use std::path::{Path, PathBuf};
use transit_model::sampler_portable::{self as portable, EXTENSION, FORMAT, ReadRefusal};

/// Largest kit file read.
pub const MAX_FILE_BYTES: u64 = 8 << 20;
/// The plug-in id whose user preset folder holds the kits.
pub const PLUGIN: &str = "sampler";

const TAGS: Tags = Tags { current: FORMAT, accepted_legacy: &[], refused_pre_0_7: &[], what: "Sampler kit" };

/// The folder of the user's kits under the settings folder `root`.
#[must_use]
pub fn folder(root: &Path) -> PathBuf {
    crate::presets::folder(root, PLUGIN)
}

/// The file of user kit `name` under `root` (the name cleaned as preset names are, `.tkit`
/// appended); `None` for a name that cleans to nothing.
#[must_use]
pub fn path(root: &Path, name: &str) -> Option<PathBuf> {
    let name = crate::presets::file_name(name);
    (!name.is_empty()).then(|| portable::kit_file_path(&folder(root).join(name)))
}

fn is_kit_file(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION))
}

/// The user's kits under `root`, by name (the file stems), sorted without regard to case. Empty
/// when there are none or the folder may not be read.
#[must_use]
pub fn list(root: &Path) -> Vec<String> {
    let dir = folder(root);
    if crate::file_access::check_read(&dir).is_err() {
        return Vec::new();
    }
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ok = is_kit_file(&p) && e.file_type().is_ok_and(|t| t.is_file());
            ok.then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())).flatten()
        })
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

/// The file's name for a message.
fn shown(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The kit file at `path`, its tag and version checked: the whole file's JSON.
///
/// # Errors
/// The file cannot be read (or is outside the automation read root), is not a regular file, is
/// larger than 8 MiB, is not JSON, or its tag or version is not one this build reads.
pub fn read(path: &Path) -> Result<Value, String> {
    let name = shown(path);
    crate::file_access::check_read(path)?;
    let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name}: not a regular file"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{name}: larger than the 8 MiB a kit file holds"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| format!("{name}: not a transit Sampler kit"))?;
    match TAGS.check(&v) {
        Ok(_) => Ok(v),
        Err(Refusal::Message(m)) => Err(format!("{name}: {m}")),
        Err(Refusal::Structural) => Err(format!("{name}: not a transit Sampler kit")),
    }
}

/// Why the existing file at `path` may not be replaced: its tag, version or kit schema is not one
/// this build reads. `None` when there is no file, or it is a kit this build reads.
fn kept(path: &Path) -> Option<String> {
    if std::fs::symlink_metadata(path).is_err() {
        return None;
    }
    let name = shown(path);
    let newer = format!("\"{name}\" was saved by a newer transit: Save As under another name");
    let other = format!("\"{name}\" is not a transit Sampler kit this version reads: Save As under another name");
    let Ok(v) = read(path) else {
        // The tag or version this build refuses: a newer version is named as such.
        let bytes = std::fs::read(path).ok();
        let version = bytes.and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.get("version").and_then(Value::as_u64));
        return Some(if version.is_some_and(|n| n > crate::file_tags::VERSION) { newer } else { other });
    };
    match portable::from_portable(v.get("kit").cloned().unwrap_or(Value::Null)) {
        Err(ReadRefusal::NewerSchema(_)) => Some(newer),
        _ => None,
    }
}

/// Writes the kit file JSON `file` to `path`, creating the folder: to a temporary file beside it,
/// then moved into place.
///
/// # Errors
/// An existing file this build does not read (named), a path outside the automation write root,
/// or a failed write.
pub fn write(path: &Path, file: &Value) -> Result<(), String> {
    crate::file_access::check_write(path)?;
    if let Some(why) = kept(path) {
        return Err(why);
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        crate::file_access::check_write(dir)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    crate::file_access::check_write(&tmp)?;
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Deletes a kit file; anything that is not a `.tkit` file is refused.
///
/// # Errors
/// Not a kit file, outside the automation write root, or the delete failed.
pub fn delete(path: &Path) -> Result<(), String> {
    if !is_kit_file(path) {
        return Err("only .tkit files are deleted".into());
    }
    crate::file_access::check_write(path)?;
    std::fs::remove_file(path).map_err(|e| format!("cannot delete {}: {e}", path.display()))
}

/// The sample a kit stored as `stored` names, resolved against the kit file `kit` (a relative path
/// from the kit's folder; another computer, a device or a URL refused) and checked: inside the
/// automation read root, and a regular file with links followed.
///
/// # Errors
/// Why it is not opened: the path check, the read root, no such file, or not a regular file.
pub(crate) fn sample(stored: &str, kit: &Path) -> Result<PathBuf, (Option<PathBuf>, String)> {
    let path = super::stored_path(stored, kit).map_err(|why| (None, why))?;
    let found = Some(path.clone());
    crate::file_access::check_read(&path).map_err(|why| (found.clone(), why))?;
    match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => Ok(path),
        Ok(_) => Err((found, "not a regular file".into())),
        Err(e) => Err((found, e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("transit-kits-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn kits_are_listed_written_read_and_deleted_in_a_scratch_settings_folder() {
        let root = scratch("list");
        let kit = portable::file(json!({"schema": 1}));
        for name in ["b kit", "A Kit", "808 v1.5"] {
            write(&path(&root, name).unwrap(), &kit).unwrap();
        }
        // A retired one-sample preset beside them is not a kit.
        std::fs::write(folder(&root).join("Old.json"), b"{}").unwrap();
        assert_eq!(list(&root), ["808 v1.5", "A Kit", "b kit"], "stems, sorted without case, kits only");
        assert!(folder(&root).join("808 v1.5.tkit").is_file(), "appended, not replaced");
        assert_eq!(read(&path(&root, "A Kit").unwrap()).unwrap(), kit);
        assert!(delete(&folder(&root).join("Old.json")).is_err(), "not a kit");
        assert!(folder(&root).join("Old.json").is_file());
        delete(&path(&root, "A Kit").unwrap()).unwrap();
        assert_eq!(list(&root), ["808 v1.5", "b kit"]);
        assert!(path(&root, "  ").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_this_build_does_not_read_is_never_replaced() {
        let root = scratch("newer");
        let dir = folder(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let kit = portable::file(json!({"schema": 1}));
        for (name, text) in [
            ("version.tkit", r#"{"format": "transit-sampler-kit", "version": 2, "kit": {}}"#),
            ("schema.tkit", r#"{"format": "transit-sampler-kit", "version": 1, "kit": {"schema": 2}}"#),
            ("other.tkit", r#"{"format": "transit-track-preset", "version": 1}"#),
            ("text.tkit", "not json"),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            let err = write(&p, &kit).unwrap_err();
            assert!(err.contains(name) && err.contains("Save As under another name"), "{err}");
            assert_eq!(err.contains("newer transit"), name == "version.tkit" || name == "schema.tkit", "{err}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), text, "byte-identical");
        }
        // A kit this build reads is replaced.
        let p = dir.join("mine.tkit");
        write(&p, &portable::file(json!({"schema": 1, "seed": 5}))).unwrap();
        write(&p, &kit).unwrap();
        assert_eq!(read(&p).unwrap(), kit);
        assert!(!dir.join("mine.tkit.tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_checks_the_tag_and_version_first() {
        let root = scratch("read");
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("x.tkit");
        std::fs::write(&p, r#"{"format": "transit-sampler-kit", "version": 2}"#).unwrap();
        assert!(read(&p).unwrap_err().contains("newer transit (format version 2)"));
        std::fs::write(&p, r#"{"format": "transit-track-preset", "version": 1}"#).unwrap();
        assert!(read(&p).unwrap_err().contains("not a transit Sampler kit"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
