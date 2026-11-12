//! Folder preview and load through the engine's atomic kit command, on explicit gestures only.
//!
//! On a kit with loaded pads (#465) the preview asks what to do, after the folder is picked and
//! before anything changes: **Replace** (the folder fills pads from A1, each filled pad cleared
//! first), **Fill free pads** (the default when the folder fits; unavailable with the engine's
//! reason when it does not) or **Cancel**. The detail line previews each choice's files and pads
//! from the engine's own header-only previews; the UI states the choice with `replace` and never
//! checks admission itself. With no loaded pad nothing is asked: the plain preview shows.

use super::view;
use crate::StudioApp;
use crate::device_editor::Dev;
use serde_json::{Value, json};

/// The question asked over a kit with loaded pads.
pub const QUESTION: &str = "Replace the loaded pads or fill the free ones?";
/// The question's choices.
pub const REPLACE: &str = "Replace";
pub const FILL: &str = "Fill free pads";

#[derive(Debug, Clone)]
pub struct Preview {
    folder: String,
    pub count: String,
    /// Set when the kit has loaded pads: the question is asked.
    pub ask: Option<Ask>,
}

/// The question's state: whether Fill free pads is available (else the engine's reason).
#[derive(Debug, Clone)]
pub struct Ask {
    pub fill: Result<(), String>,
}

/// The command's reply, or what it said instead (a refusal's notice or the error).
fn engine(app: &mut StudioApp, dev: Dev, folder: &str, preview: bool, replace: bool) -> Result<Value, String> {
    let mut params = json!({"track": dev.key.0.0, "folder": folder, "preview": preview});
    if replace {
        params["replace"] = json!(true);
    }
    match app.run("mix.sampler_load_kit", params) {
        Ok(reply) if reply["loaded"].as_u64().is_some_and(|n| n > 0) => Ok(reply),
        Ok(reply) => Err(reply["notice"].as_str().unwrap_or("Load kit returned no result.").to_string()),
        Err(error) => Err(error),
    }
}

/// Runs the command; a refusal goes to the status line and the panel's note row.
fn command(app: &mut StudioApp, dev: Dev, folder: &str, preview: bool, replace: bool) -> Option<Value> {
    match engine(app, dev, folder, preview, replace) {
        Ok(reply) => {
            view(app, dev.uid()).note = None;
            Some(reply)
        }
        Err(error) => {
            app.status = error.clone();
            view(app, dev.uid()).note = Some(error);
            None
        }
    }
}

/// `N files · pads A1–A4` of a preview reply.
fn span(reply: &Value) -> String {
    let loaded = reply["loaded"].as_u64().unwrap_or(0);
    let files = if loaded == 1 { "1 file".to_string() } else { format!("{loaded} files") };
    format!("{files} · pads {}–{}", pad_name(reply, "first_pad"), pad_name(reply, "last_pad"))
}

fn pad_name(reply: &Value, key: &str) -> String {
    reply[key].as_u64().and_then(|pad| u8::try_from(pad.checked_sub(1)?).ok()).map(super::pad_id).unwrap_or_default()
}

/// The `Ignored: ...` line of a preview reply, if any file was ignored.
fn ignored(reply: &Value) -> Option<String> {
    let names = reply["ignored"].as_array().filter(|files| !files.is_empty())?;
    Some(format!("Ignored: {}", names.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")))
}

/// What Replace will do, from its preview: the pads it fills, how many loaded pads (and slices)
/// it replaces, and what stays.
pub fn replace_line(reply: &Value) -> String {
    let n = reply["replaced"].as_u64().unwrap_or(0);
    let slices = reply["replaced_slices"].as_u64().unwrap_or(0);
    let mut out = format!("Replace: {}. ", span(reply));
    out.push_str(&match n {
        0 => "No loaded pad is replaced".to_string(),
        1 => "1 loaded pad is replaced".to_string(),
        n => format!("{n} loaded pads are replaced"),
    });
    match slices {
        0 => {}
        1 if n == 1 => out.push_str(" (it is a slice)"),
        1 => out.push_str(" (1 of them a slice)"),
        k => out.push_str(&format!(" ({k} of them slices)")),
    }
    out.push_str("; each pad filled is cleared first (name, choke group, mute, solo and settings). ");
    let last = reply["last_pad"].as_u64().unwrap_or(0);
    if last < transit_model::sampler_patch::PADS as u64 {
        out.push_str(&format!("Pads after {} stay as they are, and so do the slice markers and the slice loop.", pad_name(reply, "last_pad")));
    } else {
        out.push_str("The slice markers and the slice loop stay as they are.");
    }
    out
}

pub(crate) fn pick(app: &mut StudioApp, dev: Dev) {
    let Some(folder) = app.services.pick_open.as_ref().and_then(|pick| pick("folder", None)) else { return };
    let loaded_pads = super::patch_at(app.engine.session(), dev.key).is_some_and(|p| p.pads.iter().any(|q| q.zone.is_some()));
    if !loaded_pads {
        let Some(reply) = command(app, dev, &folder, true, false) else { return };
        let mut count = span(&reply);
        if let Some(line) = ignored(&reply) {
            count.push_str(&format!("\n{line}"));
        }
        view(app, dev.uid()).kit_preview = Some(Preview { folder, count, ask: None });
        return;
    }
    // Replace's preview first: a folder Replace cannot take either is refused as today, in the
    // engine's words, and nothing is asked.
    let Some(replace) = command(app, dev, &folder, true, true) else { return };
    let fill = engine(app, dev, &folder, true, false);
    let mut count = replace_line(&replace);
    count.push('\n');
    count.push_str(&match &fill {
        Ok(reply) => format!("Fill free pads: {}. Loaded pads stay as they are.", span(reply)),
        Err(why) => format!("Fill free pads is unavailable: {why}"),
    });
    if let Some(line) = ignored(&replace) {
        count.push_str(&format!("\n{line}"));
    }
    let ask = Ask { fill: fill.map(|_| ()) };
    view(app, dev.uid()).kit_preview = Some(Preview { folder, count, ask: Some(ask) });
}

pub(crate) fn modal_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_load_kit", uid))
}

pub(crate) fn show(app: &mut StudioApp, ctx: &egui::Context, dev: Dev) {
    use crate::chrome::{Choice, Primary};
    let Some(preview) = view(app, dev.uid()).kit_preview.clone() else { return };
    let choice = match &preview.ask {
        None => {
            let ask = crate::chrome::Ask {
                question: "Load kit".into(),
                detail: Some(preview.count.clone()),
                default: "Load kit",
                cancel: "Cancel",
                third: None,
            };
            crate::chrome::modal(ctx, modal_id(dev.uid()), &ask).map(|c| (c, false))
        }
        Some(asked) => {
            let ask = crate::chrome::Ask {
                question: QUESTION.into(),
                detail: Some(preview.count.clone()),
                default: FILL,
                cancel: "Cancel",
                third: Some(REPLACE),
            };
            let primary = match &asked.fill {
                Ok(()) => Primary::Enter,
                Err(why) => Primary::Unavailable(why),
            };
            crate::chrome::modal_with(ctx, modal_id(dev.uid()), &ask, primary).map(|c| (c, true))
        }
    };
    match choice {
        Some((Choice::Default, _)) => {
            view(app, dev.uid()).kit_preview = None;
            command(app, dev, &preview.folder, false, false);
        }
        Some((Choice::Third, true)) => {
            view(app, dev.uid()).kit_preview = None;
            command(app, dev, &preview.folder, false, true);
        }
        Some(_) => view(app, dev.uid()).kit_preview = None,
        None => {}
    }
}
