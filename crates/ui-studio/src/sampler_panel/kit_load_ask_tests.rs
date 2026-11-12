//! Load Kit Folder over a kit with loaded pads (#465): the question after the folder is picked and
//! before anything changes, driven the way a user drives it (an injected folder picker, generated
//! audio, one frame at a time with no wall clock).

use super::kit_load::{FILL, REPLACE};
use super::test_rig::{T, mid};
use serde_json::{Value, json};
use std::path::PathBuf;
use transit_model::TrackId;

fn doc(t: &T) -> Value {
    serde_json::to_value(t.app.engine.session()).unwrap()
}

/// A folder of `n` generated files `n01.wav`... in the test's folder.
fn folder(t: &T, n: usize) -> PathBuf {
    let d = t.dir.join(format!("kit{n}"));
    std::fs::create_dir_all(&d).unwrap();
    for i in 0..n {
        super::test_rig::wav(&d.join(format!("n{:02}.wav", i + 1)), 64);
    }
    d
}

/// Clicks Load kit… with the picker answering `folder`.
fn load(t: &mut T, tr: TrackId, folder: &std::path::Path) {
    let f = folder.to_string_lossy().into_owned();
    t.app.services.pick_open = Some(Box::new(move |_, _| Some(f.clone())));
    t.click(mid(t.panel(tr).bar.load_kit.unwrap()), 1);
}

fn uid(t: &T, tr: TrackId) -> u64 {
    t.app.engine.session().track(tr).unwrap().instrument.as_ref().unwrap().uid.get()
}

fn modal(t: &T, tr: TrackId) -> Option<crate::chrome::ModalLayout> {
    let open = t.app.builtin_editor.devices.sampler.views.get(&uid(t, tr)).is_some_and(|v| v.kit_preview.is_some());
    open.then(|| crate::chrome::modal_layout(&t.ctx, super::kit_load::modal_id(uid(t, tr))).expect("the preview is drawn"))
}

fn asked(t: &T, tr: TrackId) -> bool {
    t.app.builtin_editor.devices.sampler.views.get(&uid(t, tr)).and_then(|v| v.kit_preview.as_ref()).is_some_and(|p| p.ask.is_some())
}

fn button(m: &crate::chrome::ModalLayout, text: &str) -> crate::chrome::ButtonLayout {
    m.buttons.iter().find(|b| b.text == text).unwrap_or_else(|| panic!("{text} is a button: {:?}", m.buttons)).clone()
}

/// The stem of the sample on pad `pad` (0-based).
fn on(t: &T, tr: TrackId, pad: u8) -> Option<String> {
    let p = t.patch(tr);
    let zone = p.pad(pad).zone.and_then(|z| p.zone(z))?;
    let source = t.app.engine.session().source(p.sources[usize::from(zone.source)].source)?;
    std::path::Path::new(&source.path).file_stem().and_then(|x| x.to_str()).map(str::to_string)
}

#[test]
fn an_empty_kit_is_never_asked() {
    let (mut t, tr) = T::open();
    let d = folder(&t, 2);
    load(&mut t, tr, &d);
    let m = modal(&t, tr).expect("the plain preview");
    assert!(!asked(&t, tr), "no loaded pads: nothing to ask");
    assert_eq!(m.detail_text.as_deref(), Some("2 files · pads A1–A2"));
    assert!(m.buttons.iter().any(|b| b.default && b.text == "Load kit"));
    t.key("Enter");
    assert_eq!(on(&t, tr, 0).as_deref(), Some("n01"));
}

#[test]
fn the_question_counts_the_loaded_pads_and_slices_replace_takes_and_previews_both_choices() {
    let (mut t, tr) = T::kit();
    // A slice loop over pad A1's sample, two Manual slices, and pad A1's zone bound to the first.
    let kick = t.patch(tr).sources[0].source;
    t.app.run("mix.sampler_slice_loop", json!({"track": tr.0, "source": kick})).unwrap();
    {
        use transit_model::sampler_patch::{Marker, Markers, SliceBinding, SliceMode, SliceSet};
        let ins = t.app.engine.session_mut().track_mut(tr).unwrap().instrument.as_mut().unwrap();
        let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        let l = p.slicing.as_mut().unwrap();
        let markers = vec![Marker { id: 1, frame: 0, ..Marker::default() }, Marker { id: 2, frame: 2400, ..Marker::default() }];
        l.manual = Some(Markers { markers, next_id: 3, ..Markers::default() });
        l.mode = SliceMode::Manual;
        let z = p.pad(0).zone.unwrap();
        p.zone_mut(z).unwrap().slice = Some(SliceBinding { set: SliceSet::Manual, id: 1, ..SliceBinding::default() });
    }
    t.frames(2);
    let d = folder(&t, 2);
    load(&mut t, tr, &d);
    assert!(asked(&t, tr));
    let m = modal(&t, tr).unwrap();
    let detail = m.detail_text.clone().unwrap();
    assert_eq!(
        detail,
        "Replace: 2 files · pads A1–A2. 2 loaded pads are replaced (1 of them a slice); each pad filled is cleared first (name, choke \
         group, mute, solo and settings). Pads after A2 stay as they are, and so do the slice markers and the slice loop.\nFill free pads: \
         2 files · pads A4–A5. Loaded pads stay as they are."
    );
    // Replace alone at the far left, Cancel, and Fill free pads as the default (Enter).
    let texts: Vec<_> = m.buttons.iter().map(|b| (b.text.as_str(), b.default, b.enabled)).collect();
    assert_eq!(texts, [(REPLACE, false, true), ("Cancel", false, true), (FILL, true, true)]);
}

#[test]
fn fill_free_pads_is_unavailable_with_the_engines_reason_and_enter_does_nothing() {
    let (mut t, tr) = T::kit();
    let d = folder(&t, 62);
    let reason =
        t.app.run("mix.sampler_load_kit", json!({"track": tr.0, "folder": d, "preview": true})).unwrap()["notice"].as_str().unwrap().to_string();
    assert_eq!(reason, "Load kit needs 62 pads; 61 are free.");
    let before = doc(&t);
    let undo = t.undo_len();
    load(&mut t, tr, &d);
    let m = modal(&t, tr).expect("asked");
    assert!(m.detail_text.as_deref().unwrap().ends_with(&format!("\nFill free pads is unavailable: {reason}")), "{:?}", m.detail_text);
    let fill = button(&m, FILL);
    assert!(!fill.enabled && !fill.default, "unavailable, and not the default");
    assert!(m.buttons.iter().all(|b| !b.default), "no default at all");
    t.key("Enter");
    assert!(modal(&t, tr).is_some(), "Enter does nothing: the question stays");
    t.click(mid(fill.rect), 1);
    assert!(modal(&t, tr).is_some(), "the unavailable button takes no click");
    assert_eq!(doc(&t), before);
    assert_eq!(t.undo_len(), undo);
    // Keyboard focus never reaches Replace: the first Tab lands on Cancel, so Tab then Enter
    // cancels (review N1).
    t.key("Tab");
    let focused: Vec<_> = modal(&t, tr).unwrap().buttons.iter().filter(|b| b.focused).map(|b| b.text.clone()).collect();
    assert_eq!(focused, ["Cancel"], "the first Tab lands on Cancel");
    t.key("Enter");
    assert_eq!(doc(&t), before, "Tab then Enter never replaces");
    assert_eq!(t.undo_len(), undo);
    load(&mut t, tr, &d);
    let m = modal(&t, tr).expect("asked again");
    t.click(mid(button(&m, REPLACE).rect), 1);
    assert!(modal(&t, tr).is_none());
    assert_eq!(t.undo_len(), undo + 1, "Replace is one undo step");
    assert_eq!(t.patch(tr).zones.len(), 62);
    assert_eq!(on(&t, tr, 0).as_deref(), Some("n01"));
    assert_eq!(on(&t, tr, 61).as_deref(), Some("n62"));
}

#[test]
fn cancel_and_escape_change_nothing() {
    let (mut t, tr) = T::kit();
    let d = folder(&t, 2);
    let before = doc(&t);
    let undo = t.undo_len();
    load(&mut t, tr, &d);
    let m = modal(&t, tr).expect("asked");
    t.click(mid(button(&m, "Cancel").rect), 1);
    assert!(modal(&t, tr).is_none());
    assert_eq!(doc(&t), before);
    load(&mut t, tr, &d);
    assert!(asked(&t, tr));
    t.key("Escape");
    assert!(modal(&t, tr).is_none());
    assert!(t.app.builtin_editor.windows.iter().any(|w| w.key.0 == tr), "the window stays open");
    assert_eq!(doc(&t), before);
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn each_choice_is_one_undo_step_and_loads_the_pads_its_preview_named() {
    let (mut t, tr) = T::kit();
    let d = folder(&t, 2);
    let before = doc(&t);
    let undo = t.undo_len();
    // Fill free pads (Enter): pads A4–A5, the loaded pads stay.
    load(&mut t, tr, &d);
    t.key("Enter");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!((0..5).map(|p| on(&t, tr, p)).collect::<Vec<_>>(), ["Kick", "CH Hat", "OH Hat", "n01", "n02"].map(|s| Some(s.to_string())));
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_eq!(doc(&t), before, "one undo restores the kit");
    // Replace: pads A1–A2, pad A3 stays.
    load(&mut t, tr, &d);
    let m = modal(&t, tr).unwrap();
    t.click(mid(button(&m, REPLACE).rect), 1);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!((0..4).map(|p| on(&t, tr, p)).collect::<Vec<_>>(), [Some("n01"), Some("n02"), Some("OH Hat"), None].map(|s| s.map(str::to_string)));
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_eq!(doc(&t), before, "one undo restores the kit and its sources");
}
