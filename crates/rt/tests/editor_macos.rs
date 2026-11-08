//! The macOS editor window on the process main thread, which the standard test harness never uses,
//! so this binary has its own `main` (`harness = false`): `resize_content` resizes a live window
//! and refuses any other pointer without dereferencing it. A small window flashes on screen.

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("editor_macos: macOS editor windows are macOS only; nothing to test here");
}

#[cfg(target_os = "macos")]
fn main() {
    use transit_rt::editor_window::{EditorWindow, resize_content};

    let Ok(win) = EditorWindow::new("editor_macos", 200.0, 100.0, None) else {
        panic!("a window on the main thread");
    };
    let raw = win.raw();
    assert_eq!(win.content_size(), (200, 100));
    assert!(resize_content(raw, 300.0, 150.0), "a live window resizes");
    assert_eq!(win.content_size(), (300, 150));
    assert!(!resize_content(std::ptr::dangling_mut(), 1.0, 1.0), "a foreign pointer is refused, not dereferenced");
    assert!(!resize_content(std::ptr::null_mut(), 1.0, 1.0), "null is refused");
    win.close();
    assert!(!resize_content(raw, 1.0, 1.0), "a closed window is refused");
    println!("editor_macos: ok");
}
