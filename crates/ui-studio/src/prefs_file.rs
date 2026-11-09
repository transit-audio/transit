//! Writing a few preferences into `ui.json` now, between the app's own checkpoints: the file is
//! read, the named keys replaced, every other key kept as it is on disk, and the result written
//! atomically (a new file renamed over the old one).

/// Replaces `keys` in the settings file at `path`, keeping every other saved key. With no file
/// yet, `ui` is the starting point (its open editors left out). `tag` names the temporary file.
pub(crate) fn merge_keys(path: &std::path::Path, ui: &crate::UiState, tag: &str, keys: Vec<(&str, serde_json::Value)>) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut value = match std::fs::File::open(path) {
        Ok(f) => {
            let mut bytes = Vec::new();
            f.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err("Settings file exceeds the 4 MiB limit".into());
            }
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut saved = ui.clone();
            saved.plugin_windows.clear();
            serde_json::to_value(saved).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let obj = value.as_object_mut().ok_or("Settings must be an object")?;
    for (key, v) in keys {
        obj.insert(key.into(), v);
    }
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let tmp = path.with_extension(format!("{tag}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}
