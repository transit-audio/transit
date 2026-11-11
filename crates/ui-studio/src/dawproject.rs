//! File > Export DAWproject (#261 slice 2): the interoperability caveat, then the save dialog.

use crate::{Dialog, StudioApp};
use serde_json::json;

const DETAIL: &str = "Audible audio and instrument tracks include full-length WAV stems, and MIDI clips remain editable. Muted and empty tracks have no stem; inactive tracks are omitted. Devices, routing, groups and parameter automation are not exported separately. Volume/pan automation is omitted in favor of static mixer values. External Instrument audio is left out; its MIDI is included.";

impl StudioApp {
    pub fn open_dawproject_export(&mut self) {
        self.dialog = Some(Dialog::DawprojectExport);
    }

    fn pick_dawproject_export(&mut self) {
        let start = self.session_start_dir().join(format!("{}.dawproject", self.engine.session().name));
        let Some(path) = self.services.pick_save.as_ref().and_then(|pick| pick(&start.to_string_lossy())) else { return };
        if let Ok(v) = self.run("file.export_dawproject", json!({"path": path, "replace": true})) {
            let path = v["path"].as_str().unwrap_or(&path).to_string();
            let stems = v["stems"].as_u64().unwrap_or(0);
            let external = v["left_out"].as_array().map_or(0, Vec::len);
            let note = if external == 0 { String::new() } else { format!("; External Instrument audio left out on {external} tracks") };
            self.status = format!("Exported DAWproject to {path} ({stems} stems{note})");
        }
    }
}

pub fn dialog(app: &mut StudioApp, ctx: &egui::Context) {
    let ask = crate::chrome::Ask {
        question: "Export DAWproject?".into(),
        detail: Some(DETAIL.into()),
        default: "Choose Location…",
        cancel: "Cancel",
        third: None,
    };
    match crate::chrome::modal(ctx, egui::Id::new("dawproject_export"), &ask) {
        Some(crate::chrome::Choice::Default) => {
            app.dialog = None;
            app.pick_dawproject_export();
        }
        Some(_) => app.dialog = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    fn temp_dir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("transit-dawproject-export-ui-{}-{stamp}-{serial}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn the_export_dialog_discloses_the_stem_fallback() {
        let mut app = crate::tests::app();
        app.open_dawproject_export();
        assert!(matches!(app.dialog, Some(Dialog::DawprojectExport)));
        assert!(DETAIL.contains("full-length WAV stems") && DETAIL.contains("MIDI clips remain editable"));
        assert!(DETAIL.contains("External Instrument audio is left out"));
    }

    #[test]
    fn choosing_a_location_exports_and_reports_the_result() {
        let dir = temp_dir();
        let path = dir.join("song.dawproject");
        let offered = Rc::new(RefCell::new(String::new()));
        let seen = offered.clone();
        let answer = path.clone();
        let mut app = crate::tests::app();
        app.services.pick_save = Some(Box::new(move |default| {
            *seen.borrow_mut() = default.to_string();
            Some(answer.to_string_lossy().into_owned())
        }));
        app.open_dawproject_export();
        app.dialog = None;
        app.pick_dawproject_export();
        assert!(path.is_file());
        assert_eq!(Path::new(offered.borrow().as_str()).extension().and_then(|e| e.to_str()), Some("dawproject"));
        assert!(app.status.starts_with("Exported DAWproject to "), "{}", app.status);
        assert!(app.dialog.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
