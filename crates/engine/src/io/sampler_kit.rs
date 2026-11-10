//! Checked, immediate-file enumeration and header-only kit inspection.

use super::*;

pub(crate) struct Folder {
    pub files: Vec<PathBuf>,
    pub ignored: Vec<String>,
}

pub(crate) fn folder(path: &Path) -> Result<Folder> {
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let fail = |err| EngineError::Io(format!("{}: {err}", path.display()));
    let root = std::fs::canonicalize(path).map_err(fail)?;
    let mut out = Folder { files: Vec::new(), ignored: Vec::new() };
    for entry in std::fs::read_dir(&root).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| EngineError::Io("Load kit: a file name is not valid UTF-8".into()))?;
        let kind = entry.file_type().map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
        if name.starts_with('.') || !kind.is_file() || !matches!(ext.as_str(), "wav" | "wave" | "aif" | "aiff" | "aifc" | "flac") {
            out.ignored.push(name);
        } else {
            out.files.push(path);
        }
    }
    sort_files(&mut out.files);
    out.ignored.sort_by_key(|n| (n.to_lowercase(), n.clone()));
    Ok(out)
}

pub(crate) fn sort_files(files: &mut [PathBuf]) {
    files.sort_by_cached_key(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        (name.to_lowercase(), name)
    });
}

/// Recheck before each open: links (including a replaced entry) never become kit inputs.
pub(crate) fn regular(path: &Path) -> Result<()> {
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let meta = std::fs::symlink_metadata(path).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))?;
    if !meta.file_type().is_file() {
        return Err(EngineError::Io(format!("{}: not a plain file", path.display())));
    }
    Ok(())
}

pub(crate) fn probe(path: &Path) -> Result<transit_audio_io::AudioInfo> {
    regular(path)?;
    let mut input = open_file(path)?;
    transit_audio_io::probe_metadata(&mut input).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}
