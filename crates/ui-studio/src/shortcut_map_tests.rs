//! Shortcut registry and Help tests, using the shared headless UI rig.

use super::{T, mid, view_item};
use serde_json::json;

#[test]
fn no_two_shortcuts_share_a_key_in_a_scope() {
    use crate::shortcuts::{Scope, TABLE, text};
    let mut seen = std::collections::HashMap::new();
    for s in TABLE {
        let k = (s.scope, text(s));
        assert!(seen.insert(k.clone(), s.label).is_none(), "{k:?}: {} and {}", seen[&k], s.label);
        assert!(!s.note.is_empty(), "{}: every row says where its key comes from", s.label);
    }
    // A global key must not also mean something in the arrangement or the piano roll.
    for g in TABLE.iter().filter(|s| s.scope == Scope::Global) {
        for o in TABLE.iter().filter(|s| matches!(s.scope, Scope::Arrangement | Scope::PianoRoll)) {
            assert_ne!(text(g), text(o), "{} (global) and {} ({:?})", g.label, o.label, o.scope);
        }
    }
}

#[test]
fn interface_zoom_shortcuts_reference_matches_the_global_table() {
    use crate::shortcuts::{Action, Scope, TABLE, text};
    let reference = include_str!("../../../docs/shortcuts.md");
    let actions = [Action::ZoomInterfaceIn, Action::ZoomInterfaceOut, Action::ResetInterfaceZoom];
    let rows: Vec<_> = TABLE.iter().filter(|row| actions.contains(&row.action)).collect();
    assert_eq!(rows.len(), 5);
    for row in rows {
        assert_eq!(row.scope, Scope::Global);
        assert!(reference.contains(row.label));
        assert!(reference.contains(&text(row)));
    }
}

#[test]
fn every_handler_takes_its_keys_from_the_table() {
    let sources = [
        include_str!("menus.rs"),
        include_str!("groups.rs"),
        include_str!("browser.rs"),
        include_str!("keys.rs"),
        include_str!("mixer.rs"),
        include_str!("mixer/actions.rs"),
        include_str!("mixer/controls.rs"),
        include_str!("mixer/menus.rs"),
        include_str!("mixer/meters.rs"),
        include_str!("mixer/routing.rs"),
        include_str!("mixer/sends.rs"),
        include_str!("mixer/strip.rs"),
        include_str!("keys/view.rs"),
        include_str!("arrange/tracks.rs"),
        include_str!("edit_tools.rs"),
        include_str!("arrange/clips.rs"),
        include_str!("host/transport.rs"),
        include_str!("host/recording.rs"),
        include_str!("host/plugin_state.rs"),
        include_str!("host/autosave.rs"),
        include_str!("host/reconnect.rs"),
        include_str!("browser/header.rs"),
        include_str!("browser/plugins.rs"),
        include_str!("automation/pass.rs"),
        // The Audio Devices window's Tab and Escape key reads are tracked separately.
        include_str!("arrange.rs"),
        include_str!("devices.rs"),
        include_str!("transport.rs"),
        include_str!("telemetry.rs"),
        include_str!("load_readout.rs"),
        include_str!("lib.rs"),
        include_str!("finish.rs"),
        include_str!("midi_learn.rs"),
        include_str!("automation.rs"),
        include_str!("automation_picker.rs"),
        include_str!("macro_osc_panel/window.rs"),
        include_str!("device_editor/window.rs"),
        include_str!("sampler_panel/pad_actions.rs"),
        include_str!("compressor_panel/mod.rs"),
        include_str!("compressor_panel/sections.rs"),
    ];
    for src in sources {
        for raw in ["consume_key(", "key_pressed(", "shortcut_text(\"", "Key::"] {
            assert!(!src.contains(raw), "a handler reads keys directly ({raw})");
        }
    }
    // And every action in the table has a handler.
    let all: String = sources.concat();
    for s in crate::shortcuts::TABLE {
        let name = format!("{:?}", s.action);
        assert!(all.contains(&format!("A::{name}")) || all.contains(&format!("Action::{name}")), "{name} has no handler");
    }
}

#[test]
fn menus_show_the_table_keys() {
    use crate::shortcuts::{Action, label};
    assert_eq!(label(Action::Redo), "Ctrl+Shift+Z");
    assert_eq!(label(Action::Split), "Ctrl+E");
    assert_eq!(label(Action::ToggleBrowser), "Ctrl+Alt+B");
    assert_eq!(label(Action::Record), "R");
    assert_eq!(label(Action::NotesTransposeUp), "Ctrl+Up");
}

#[test]
fn keys_from_the_table_still_drive_the_transport_and_views() {
    let mut t = T::new();
    t.input("ui.key", json!({"key": "C"}));
    assert!(t.app.engine.session().edit.loop_playback, "C: cycle");
    t.input("ui.key", json!({"key": "Space"}));
    assert!(t.app.is_playing(), "Space: play");
    t.input("ui.key", json!({"key": "Space"}));
    assert!(!t.app.is_playing());
    let shown = t.app.ui.browser.show;
    t.input("ui.key", json!({"key": "B", "cmd": true, "alt": true}));
    assert_eq!(t.app.ui.browser.show, !shown, "Ctrl+Alt+B: browser");
    t.input("ui.key", json!({"key": "F3"}));
    assert!(t.app.ui.mixer_full, "F3: full mixer");
}

#[test]
fn help_keyboard_shortcuts_opens_groups_by_scope_and_filters() {
    let mut t = T::new();
    let rev = (t.app.engine.is_dirty(), t.app.engine.revision);
    t.click(mid(view_item(&t, "Help")));
    t.click(mid(view_item(&t, "Keyboard Shortcuts")));
    let w = t.inspect()["shortcuts"].clone();
    assert_eq!(w["open"], true);
    let scopes: Vec<String> = serde_json::from_value(w["scopes"].clone()).unwrap();
    assert_eq!(
        scopes,
        ["Global", "Arrangement", "Piano roll", "Step entry", "Automation", "Browser", "Mixer", "Dialogs", "Name field", "Device editor", "Sampler"]
    );
    let all = w["rows"].as_array().unwrap().len();
    assert_eq!(all, crate::shortcuts::TABLE.len(), "every row shown");
    // Filter by typing in its box.
    let filter: [f32; 4] = serde_json::from_value(w["filter"].clone()).unwrap();
    let spp = t.app.arrange.layout.samples_per_px;
    t.click(mid(filter));
    t.input("ui.text", json!({"text": "zoom-="}));
    assert_eq!(t.app.arrange.layout.samples_per_px, spp, "typing - and = in the filter is not a shortcut");
    t.app.ui.shortcuts_filter = "zoom".into();
    t.frames(2);
    let rows: Vec<String> = serde_json::from_value(t.inspect()["shortcuts"]["rows"].clone()).unwrap();
    assert_eq!(
        rows,
        [
            "Global · Zoom Interface In · Ctrl+=",
            "Global · Zoom Interface In · Ctrl+Plus",
            "Global · Zoom Interface In · Ctrl+Shift+Plus",
            "Global · Zoom Interface Out · Ctrl+-",
            "Global · Reset Interface Zoom · Ctrl+0",
            "Arrangement · Zoom In · +",
            "Arrangement · Zoom In · =",
            "Arrangement · Zoom In · Shift+Plus",
            "Arrangement · Zoom Out · -",
        ]
    );
    t.app.ui.shortcuts_filter.clear();
    t.click(mid(filter));
    t.input("ui.text", json!({"text": "ctrl+e"}));
    let rows: Vec<String> = serde_json::from_value(t.inspect()["shortcuts"]["rows"].clone()).unwrap();
    assert_eq!(rows, ["Arrangement · Split at Cursor · Ctrl+E"]);
    assert_eq!((t.app.engine.is_dirty(), t.app.engine.revision), rev, "the window never touches the document");
}

#[test]
fn the_shortcuts_window_closes_from_its_title_bar() {
    let mut t = T::new();
    t.click(mid(view_item(&t, "Help")));
    t.click(mid(view_item(&t, "Keyboard Shortcuts")));
    let w: [f32; 4] = serde_json::from_value(t.inspect()["shortcuts"]["window"].clone()).unwrap();
    t.click((w[2] - 14.0, w[1] + 13.0));
    assert_eq!(t.inspect()["shortcuts"]["open"], false);
    assert!(!t.app.ui.show_shortcuts);
}

#[test]
fn every_table_row_is_reachable_and_no_looser_row_shadows_it() {
    use crate::shortcuts::{Action, Scope, TABLE, pressed, text};
    let live = [Scope::Global, Scope::Arrangement, Scope::PianoRoll, Scope::Step, Scope::Mixer];
    // Does `action` consume a press of `key` with exactly `mods` held? (A fresh frame each time,
    // so one action cannot eat the press another is asked about.)
    let fires = |key: egui::Key, mods: egui::Modifiers, action: Action| {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(mods),
                egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods },
            ],
            ..Default::default()
        };
        let mut hit = false;
        let mut out = ctx.run_ui(raw, |ui| hit = pressed(ui.ctx(), action));
        out.textures_delta.clear();
        hit
    };
    for row in TABLE.iter().filter(|r| live.contains(&r.scope)) {
        assert!(fires(row.key, row.mods, row.action), "{} ({}) does not fire", row.label, text(row));
        // Only rows with exactly these modifiers answer; a looser row of the same key does not.
        for o in TABLE.iter().filter(|o| o.key == row.key && o.mods != row.mods) {
            assert!(
                !fires(row.key, row.mods, o.action) || TABLE.iter().any(|x| x.action == o.action && x.key == row.key && x.mods == row.mods),
                "{} ({}) also fires on {}",
                o.label,
                text(o),
                text(row)
            );
        }
        // Extra Alt or Shift on top of the row's modifiers reaches none of the same-key rows.
        for extra in [egui::Modifiers { alt: true, ..row.mods }, egui::Modifiers { shift: true, ..row.mods }] {
            if extra == row.mods || TABLE.iter().any(|o| o.key == row.key && o.mods == extra) {
                continue;
            }
            for o in TABLE.iter().filter(|o| o.key == row.key) {
                assert!(!fires(row.key, extra, o.action), "{} fires with extra modifiers on {}", o.label, text(row));
            }
        }
    }
}

#[test]
fn the_help_window_lists_the_new_rows() {
    let mut t = T::new();
    t.click(mid(view_item(&t, "Help")));
    t.click(mid(view_item(&t, "Keyboard Shortcuts")));
    let rows: Vec<String> = serde_json::from_value(t.inspect()["shortcuts"]["rows"].clone()).unwrap();
    for want in [
        "Global · New Audio Track · Ctrl+T",
        "Global · New Instrument Track · Ctrl+Shift+T",
        "Global · Record · F9",
        "Arrangement · Select All Clips · Ctrl+A",
    ] {
        assert!(rows.iter().any(|r| r == want), "{want} missing from {rows:?}");
    }
    assert!(!rows.iter().any(|r| r.contains("Split at Cursor · Ctrl+T")));
}

#[test]
fn the_shortcut_filter_searches_notes_and_spells_out_shift_plus() {
    use crate::shortcuts::{TABLE, filtered, text};
    for s in TABLE {
        let hit = filtered(s.note).into_iter().flat_map(|(_, r)| r).any(|r| r.action == s.action && r.key == s.key && r.mods == s.mods);
        assert!(hit, "{}: its own note finds it", s.label);
    }
    assert!(TABLE.iter().any(|s| text(s) == "Shift+Plus"));
    assert!(!TABLE.iter().any(|s| text(s).ends_with("++")));
}
