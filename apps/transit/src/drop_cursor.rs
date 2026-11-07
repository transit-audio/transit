//! The cursor's position at a file drop (#368 follow-up). winit 0.30 reports a file dropped from
//! the file manager as `DroppedFile(PathBuf)` alone (its Windows drop handler discards the OLE drop
//! point), and egui-winit 0.36 forwards nothing positional for it; on Windows a `PointerGone` can
//! even reach the drop router first. So at the drop's frame the binary asks the OS where the cursor
//! is and puts a `PointerMoved` there at the front of the frame's events, where the router
//! (`transit_ui_studio::drops`) finds it. The OS query is `transit_rt::cursor_in_client`
//! (Windows; `None` elsewhere, which leaves the frame unchanged).

/// When `raw` carries dropped files, asks `cursor` for the cursor in client pixels and, when it is
/// inside the client area, inserts `PointerMoved` at that point (in egui points) as the frame's
/// first event. `zoom` is egui's zoom factor; the native scale comes from the frame's own viewport
/// info. Nothing changes without dropped files, without an answer, or with the cursor outside.
pub(crate) fn place_drop(raw: &mut egui::RawInput, zoom: f32, cursor: impl FnOnce() -> Option<(i32, i32)>) {
    if raw.dropped_files.is_empty() {
        return;
    }
    let Some((x, y)) = cursor() else { return };
    // egui-winit's own arithmetic: `pixels_per_point = zoom_factor * window.scale_factor()` and
    // `CursorMoved` becomes `pos2(x as f32 / pixels_per_point, y as f32 / pixels_per_point)`, with
    // no inset (egui-winit 0.36.2, `pixels_per_point` and `on_cursor_moved`).
    let native = raw.viewport().native_pixels_per_point.unwrap_or(1.0);
    let pixels_per_point = zoom * native;
    if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 || x < 0 || y < 0 {
        return;
    }
    let pos = egui::pos2(x as f32 / pixels_per_point, y as f32 / pixels_per_point);
    // `screen_rect` is the client area in points (egui-winit: inner size / pixels_per_point).
    if raw.screen_rect.is_some_and(|r| !r.contains(pos)) {
        return;
    }
    raw.events.insert(0, egui::Event::PointerMoved(pos));
}

#[cfg(test)]
mod tests {
    use super::place_drop;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[derive(Debug)]
    struct Dropped(PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("not read".into())
        }
    }

    /// A drop frame on a 2.0 scale window of 800 by 600 points, already carrying a `PointerGone`
    /// (the order the maintainer's Windows machine produced).
    fn drop_frame(files: bool) -> egui::RawInput {
        let mut raw =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
        raw.viewports.entry(raw.viewport_id).or_default().native_pixels_per_point = Some(2.0);
        raw.events.push(egui::Event::PointerGone);
        if files {
            raw.dropped_files = vec![Arc::new(Dropped(PathBuf::from("Snare.wav"))) as egui::DroppedFileHandle];
        }
        raw
    }

    #[test]
    fn a_drop_gains_the_cursor_as_its_first_event_in_points() {
        let mut raw = drop_frame(true);
        place_drop(&mut raw, 1.0, || Some((300, 500)));
        assert_eq!(raw.events.first(), Some(&egui::Event::PointerMoved(egui::pos2(150.0, 250.0))), "{:?}", raw.events);
        assert_eq!(raw.events.len(), 2, "the PointerGone stays, after it");
        // The zoom factor scales like egui-winit's: 2.0 native times 1.25 zoom is 2.5 pixels a point.
        let mut zoomed = drop_frame(true);
        place_drop(&mut zoomed, 1.25, || Some((300, 500)));
        assert_eq!(zoomed.events.first(), Some(&egui::Event::PointerMoved(egui::pos2(120.0, 200.0))));
    }

    #[test]
    fn no_answer_or_no_drop_pushes_nothing() {
        // Negative control: the provider has no position (off Windows, or the call failed).
        let mut raw = drop_frame(true);
        place_drop(&mut raw, 1.0, || None);
        assert_eq!(raw.events, vec![egui::Event::PointerGone]);
        // Negative control: no files dropped; the provider is not even asked.
        let asked = std::cell::Cell::new(false);
        let mut plain = drop_frame(false);
        place_drop(&mut plain, 1.0, || {
            asked.set(true);
            Some((300, 500))
        });
        assert_eq!(plain.events, vec![egui::Event::PointerGone]);
        assert!(!asked.get(), "asked without a drop");
    }

    #[test]
    fn a_cursor_outside_the_client_area_pushes_nothing() {
        for outside in [(-1, 10), (10, -1), (1601, 10), (10, 1201)] {
            let mut raw = drop_frame(true);
            place_drop(&mut raw, 1.0, || Some(outside));
            assert_eq!(raw.events, vec![egui::Event::PointerGone], "{outside:?}");
        }
        // Control: the client area's last pixel is inside.
        let mut raw = drop_frame(true);
        place_drop(&mut raw, 1.0, || Some((1599, 1199)));
        assert_eq!(raw.events.len(), 2);
    }
}
