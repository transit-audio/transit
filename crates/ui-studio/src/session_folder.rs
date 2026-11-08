//! Where a session file dialog starts (#385): Save As, Open and Bounce.
//!
//! Ableton and Logic agree: a session that has a file starts in its own folder; a new, never-saved
//! session starts in the last folder a session was saved to or opened from on this machine
//! (`UiState::last_session_dir`, in `ui.json`); otherwise the user's documents folder. Without a
//! folder the OS reuses the last folder any dialog in the process visited, such as a sample folder
//! the Sampler loaded from. The sample, kit and plug-in preset dialogs keep their own memories.

use crate::StudioApp;
use std::path::{Path, PathBuf};

/// The folder a session dialog starts in: the session file's folder, else the remembered last
/// session folder, else `fallback`. A candidate that is not a folder (`is_dir` false, such as a
/// remembered folder since deleted) falls through to the next.
pub fn session_dialog_dir(session: Option<&Path>, remembered: Option<&Path>, fallback: &Path, is_dir: impl Fn(&Path) -> bool) -> PathBuf {
    let own = session.and_then(Path::parent);
    [own, remembered]
        .into_iter()
        .flatten()
        .find(|dir| !dir.as_os_str().is_empty() && is_dir(dir))
        .map_or_else(|| fallback.to_path_buf(), Path::to_path_buf)
}

/// The user's documents folder, else the home folder, else the temporary folder.
pub fn documents_dir() -> PathBuf {
    if cfg!(test) {
        // Tests never point at the user's folders.
        return std::env::temp_dir();
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    match home {
        Some(home) if home.join("Documents").is_dir() => home.join("Documents"),
        Some(home) if home.is_dir() => home,
        _ => std::env::temp_dir(),
    }
}

impl StudioApp {
    /// The folder Save As and Open start in, with `fallback` last (the documents folder for them).
    pub(crate) fn session_dir_or(&self, fallback: &Path) -> PathBuf {
        session_dialog_dir(self.engine.path.as_deref().map(Path::new), self.ui.last_session_dir.as_deref(), fallback, Path::is_dir)
    }

    /// The folder Save As and Open start in.
    pub(crate) fn session_start_dir(&self) -> PathBuf {
        self.session_dir_or(&documents_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::session_dialog_dir;
    use std::path::{Path, PathBuf};

    fn existing<'a>(dirs: &'a [&'a str]) -> impl Fn(&Path) -> bool + 'a {
        move |p: &Path| dirs.iter().any(|d| Path::new(d) == p)
    }

    #[test]
    fn a_saved_session_starts_in_its_own_folder() {
        let is_dir = existing(&["/songs/a", "/remembered", "/docs"]);
        let dir = session_dialog_dir(Some(Path::new("/songs/a/tune.transit")), Some(Path::new("/remembered")), Path::new("/docs"), &is_dir);
        assert_eq!(dir, PathBuf::from("/songs/a"));
        // Negative control: a session whose folder is gone falls through to the remembered one.
        let dir = session_dialog_dir(Some(Path::new("/gone/tune.transit")), Some(Path::new("/remembered")), Path::new("/docs"), &is_dir);
        assert_eq!(dir, PathBuf::from("/remembered"));
    }

    #[test]
    fn a_new_session_starts_in_the_remembered_session_folder() {
        let is_dir = existing(&["/remembered", "/docs"]);
        assert_eq!(session_dialog_dir(None, Some(Path::new("/remembered")), Path::new("/docs"), &is_dir), PathBuf::from("/remembered"));
        // Negative control: a bare file name has no folder of its own and does not win.
        assert_eq!(
            session_dialog_dir(Some(Path::new("tune.transit")), Some(Path::new("/remembered")), Path::new("/docs"), &is_dir),
            PathBuf::from("/remembered")
        );
    }

    #[test]
    fn with_nothing_remembered_the_dialog_starts_in_documents() {
        let is_dir = existing(&["/docs"]);
        assert_eq!(session_dialog_dir(None, None, Path::new("/docs"), &is_dir), PathBuf::from("/docs"));
    }

    #[test]
    fn a_remembered_folder_that_no_longer_exists_falls_through_to_documents() {
        let is_dir = existing(&["/docs"]);
        assert_eq!(session_dialog_dir(None, Some(Path::new("/deleted")), Path::new("/docs"), &is_dir), PathBuf::from("/docs"));
        // Negative control: the same folder, still there, is used.
        let is_dir = existing(&["/deleted", "/docs"]);
        assert_eq!(session_dialog_dir(None, Some(Path::new("/deleted")), Path::new("/docs"), &is_dir), PathBuf::from("/deleted"));
    }
}
