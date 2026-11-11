//! Folder preview and load through the engine's atomic kit command, on explicit gestures only.

use super::view;
use crate::StudioApp;
use crate::device_editor::Dev;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Preview {
    folder: String,
    pub count: String,
}

fn command(app: &mut StudioApp, dev: Dev, folder: &str, preview: bool) -> Option<Value> {
    let result = app.run("mix.sampler_load_kit", json!({"track": dev.key.0.0, "folder": folder, "preview": preview}));
    let error = match result {
        Ok(reply) if reply["loaded"].as_u64().is_some_and(|n| n > 0) => {
            view(app, dev.uid()).note = None;
            return Some(reply);
        }
        Ok(reply) => reply["notice"].as_str().unwrap_or("Load kit returned no result.").to_string(),
        Err(error) => error,
    };
    app.status = error.clone();
    view(app, dev.uid()).note = Some(error);
    None
}

pub(crate) fn pick(app: &mut StudioApp, dev: Dev) {
    let Some(folder) = app.services.pick_open.as_ref().and_then(|pick| pick("folder", None)) else { return };
    let Some(reply) = command(app, dev, &folder, true) else { return };
    let loaded = reply["loaded"].as_u64().unwrap_or(0);
    let pad_name = |key: &str| reply[key].as_u64().and_then(|pad| u8::try_from(pad.checked_sub(1)?).ok()).map(super::pad_id).unwrap_or_default();
    let mut count = format!("{loaded} files · pads {}–{}", pad_name("first_pad"), pad_name("last_pad"));
    if let Some(ignored) = reply["ignored"].as_array().filter(|files| !files.is_empty()) {
        let names = ignored.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
        count.push_str(&format!("\nIgnored: {names}"));
    }
    view(app, dev.uid()).kit_preview = Some(Preview { folder, count });
}

pub(crate) fn modal_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_load_kit", uid))
}

pub(crate) fn show(app: &mut StudioApp, ctx: &egui::Context, dev: Dev) {
    let Some(preview) = view(app, dev.uid()).kit_preview.clone() else { return };
    let ask = crate::chrome::Ask { question: "Load kit".into(), detail: Some(preview.count), default: "Load kit", cancel: "Cancel", third: None };
    match crate::chrome::modal(ctx, modal_id(dev.uid()), &ask) {
        Some(crate::chrome::Choice::Default) => {
            view(app, dev.uid()).kit_preview = None;
            command(app, dev, &preview.folder, false);
        }
        Some(_) => view(app, dev.uid()).kit_preview = None,
        None => {}
    }
}
