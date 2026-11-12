//! Scratch folders for tests (#497).
//!
//! Every test that needs a folder in the system temp directory takes it from [`TestDir`]. The folder
//! is removed when the guard drops, so a passing, failing or panicking test leaves nothing behind. The
//! name carries the process id, the clock and a per-process counter, so a path is never reused, not
//! by a test in the same process and not by a later run that lands on a repeated process id.
//!
//! `cargo xtask temp-dirs` fails when code outside this crate calls `std::env::temp_dir` directly
//! (outside its short allow list), so the leak cannot come back.
//!
//! This is a dev-dependency only (`cargo xtask layers` rejects any other kind).

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A unique path that is removed (folder or file, with everything under it) on drop.
///
/// It derefs to [`Path`], so `dir.join("a.wav")` and `&dir` work where a path is expected. The path
/// is not created by [`TestDir::new`]; [`TestDir::created`] creates the folder.
#[derive(Debug)]
pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    /// A fresh unique path under the system temp directory. Nothing exists there yet.
    pub fn new(tag: &str) -> TestDir {
        TestDir::in_dir(&std::env::temp_dir(), tag)
    }

    /// A fresh unique folder under the system temp directory, created.
    pub fn created(tag: &str) -> TestDir {
        let dir = TestDir::new(tag);
        dir.create();
        dir
    }

    /// A fresh unique path under `base` (for example a crate's `CARGO_TARGET_TMPDIR`). Nothing exists there yet.
    pub fn in_dir(base: &Path, tag: &str) -> TestDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // The clock alone is not unique: threads of one process can read the same tick (coarse on Windows).
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let tag: String = tag.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
        TestDir { path: base.join(format!("transit-test-{tag}-{}-{nanos}-{serial}", std::process::id())) }
    }

    /// Like [`TestDir::in_dir`], created; refuses (panics on) a path that already exists.
    pub fn created_in(base: &Path, tag: &str) -> TestDir {
        let dir = TestDir::in_dir(base, tag);
        if let Some(parent) = dir.path.parent() {
            assert!(std::fs::create_dir_all(parent).is_ok(), "test folder parent {} could not be created", parent.display());
        }
        assert!(std::fs::create_dir(&dir.path).is_ok(), "test folder {} exists or could not be created", dir.path.display());
        dir
    }

    /// A fresh unique file path (`.{ext}`) under the system temp directory. Nothing exists there yet;
    /// the file is removed on drop.
    pub fn file(tag: &str, ext: &str) -> TestDir {
        let mut dir = TestDir::new(tag);
        dir.path.set_extension(ext);
        dir
    }

    /// Creates the folder (and any missing parents).
    pub fn create(&self) {
        assert!(std::fs::create_dir_all(&self.path).is_ok(), "test folder {} could not be created", self.path.display());
    }

    /// The path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Deref for TestDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<std::ffi::OsStr> for TestDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.path.as_os_str()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        // Best effort: a path a test already removed, or a file, is fine; never panic while unwinding.
        if std::fs::remove_dir_all(&self.path).is_err() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_unique_and_not_created() {
        let (a, b) = (TestDir::new("same"), TestDir::new("same"));
        assert_ne!(a.path(), b.path());
        assert!(!a.exists());
    }

    #[test]
    fn the_folder_is_removed_on_drop_with_its_contents() {
        let dir = TestDir::created("drop");
        let path = dir.path().to_path_buf();
        std::fs::write(path.join("f.txt"), b"x").unwrap();
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn the_folder_is_removed_when_the_test_panics() {
        let path = std::sync::Mutex::new(PathBuf::new());
        let result = std::panic::catch_unwind(|| {
            let dir = TestDir::created("panic");
            *path.lock().unwrap() = dir.path().to_path_buf();
            panic!("a failing test");
        });
        assert!(result.is_err());
        let path = path.lock().unwrap().clone();
        assert!(!path.as_os_str().is_empty() && !path.exists());
    }

    #[test]
    fn a_file_path_is_removed_too() {
        let dir = TestDir::new("file");
        std::fs::write(dir.path(), b"x").unwrap();
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn created_in_refuses_nothing_new_and_unique_names_never_collide() {
        let base = TestDir::created("base");
        let a = TestDir::created_in(&base, "x");
        let b = TestDir::created_in(&base, "x");
        assert_ne!(a.path(), b.path());
        assert!(a.is_dir() && b.is_dir());
    }
}
