//! The device title bar: its anatomy and tokens, the preset browser over the existing preset
//! system, the output meter, bypass and the follow toggle.

use super::test_rig::{T, info, mid};
use super::title_bar::{BUTTON, FIELD_MIN, FIELD_W, METER_W, preset_layout, preset_width, stepped};
use crate::theme::TOKENS;
use crate::widgets::test_support::size;

fn names(list: &[(bool, &str)]) -> Vec<(bool, String)> {
    list.iter().map(|(f, n)| (*f, n.to_string())).collect()
}

#[test]
fn the_browser_steps_and_wraps_and_starts_at_either_end_from_no_preset() {
    let n = names(&[(true, "Init"), (true, "Bass"), (false, "Mine")]);
    assert_eq!(stepped(&n, "Init", 1), Some(1));
    assert_eq!(stepped(&n, "Mine", 1), Some(0), "next wraps to the first");
    assert_eq!(stepped(&n, "Init", -1), Some(2), "previous wraps to the last");
    assert_eq!(stepped(&n, "", 1), Some(0));
    assert_eq!(stepped(&n, "<factory default>", -1), Some(2));
    // Negative control: an empty list steps nowhere.
    assert_eq!(stepped(&[], "Init", 1), None);
}

#[test]
fn the_bar_draws_the_board_anatomy_in_tokens() {
    let (t, tr) = T::open();
    let b = t.panel(tr).title;
    assert_eq!(size(b.bar).1, crate::tokens::layout::TITLE_BAR_HEIGHT);
    assert_eq!(b.device, "Macro Oscillator");
    assert_eq!(b.track, t.app.engine.session().track(tr).unwrap().name);
    for r in [b.prev, b.next, b.save, b.bypass, b.menu, b.close, b.follow] {
        assert_eq!(size(r), (BUTTON, BUTTON), "{r:?}");
    }
    assert!((size(b.field).0 - FIELD_W).abs() < 0.01, "{:?}", b.field);
    assert_eq!(size(b.meter).0, METER_W);
    // The browser centers in the free space between the side groups; prev, field, next, save run
    // left to right.
    let center = (b.device_rect[2] + b.meter[0]) / 2.0;
    assert!(b.prev[2] < b.field[0] && b.field[2] < b.next[0] && b.next[2] < b.save[0]);
    assert!(((b.prev[0] + b.save[2]) / 2.0 - center).abs() < 1.0, "the browser is centered in the free space");
    // Right side, from the edge in: close, menu, bypass, follow, then the meter.
    assert!(b.close[0] > b.menu[2] && b.menu[0] > b.bypass[2] && b.bypass[0] > b.follow[2] && b.follow[0] > b.meter[2]);
    // An instrument has no bypass: the glyph is ghosted and does nothing.
    assert!(!b.bypass_available && !b.bypass_on);
    assert!(!b.follow_on, "follow selection starts off");
    assert_eq!(b.follow_fill, TOKENS.surface_raised, "the off toggle");
    assert_eq!(t.window(tr).preset, b.field, "the editor record's preset rect is the field");
}

#[test]
fn next_and_previous_recall_the_factory_presets_in_order_as_one_step_each() {
    let (mut t, tr) = T::open();
    t.app.services.config_dir = None;
    let factory = crate::presets::factory_for(super::MACRO_OSC);
    let before = t.undo_len();
    let next = t.panel(tr).title.next;
    t.click(mid(next), 1);
    let ins = t.app.builtin_at((tr, transit_mix::INSTRUMENT_SLOT)).unwrap();
    assert_eq!(ins.preset, factory[0].0, "from no preset, next is the first");
    assert_eq!(t.undo_len(), before + 1, "a recall is one undo step");
    t.click(mid(next), 1);
    assert_eq!(t.app.builtin_at((tr, transit_mix::INSTRUMENT_SLOT)).unwrap().preset, factory[1].0);
    let h = factory[1].1.params.get("harmonics").copied().unwrap_or(info().param("harmonics").unwrap().default);
    assert_eq!(t.param(tr, "harmonics"), h, "the preset's values applied");
    assert_eq!(t.panel(tr).title.preset_name, factory[1].0);
    assert_eq!(t.panel(tr).title.preset_group, "Factory");
    let prev = t.panel(tr).title.prev;
    t.click(mid(prev), 1);
    t.click(mid(prev), 1);
    let last = factory.last().unwrap().0;
    assert_eq!(t.app.builtin_at((tr, transit_mix::INSTRUMENT_SLOT)).unwrap().preset, last, "previous from the first wraps to the last");
    // Negative control: a click on the field opens the list and recalls nothing.
    let n = t.undo_len();
    let field = t.panel(tr).title.field;
    t.click(mid(field), 1);
    assert_eq!(t.undo_len(), n);
    assert!(t.painted("Factory").0 >= 1, "the list opens with its Factory heading");
}

#[test]
fn save_writes_over_the_current_user_preset_and_opens_the_list_for_a_factory_one() {
    let dir = std::env::temp_dir().join(format!("transit-326m-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut t, tr) = T::open();
    t.app.services.config_dir = Some(dir.clone());
    let synth = info();
    let values: std::collections::BTreeMap<String, f32> = synth.params.iter().map(|p| (p.id.to_string(), p.default)).collect();
    crate::presets::save_user(&dir, synth, "Mine", &values).unwrap();
    let p = crate::presets::load_user(&dir, synth.id, "Mine").unwrap();
    t.app.apply_preset((tr, transit_mix::INSTRUMENT_SLOT), synth, "Mine", &p);
    t.set(tr, "harmonics", 77.0);
    t.frames(2);
    assert_eq!(t.panel(tr).title.preset_group, "User");
    let save = t.panel(tr).title.save;
    t.click(mid(save), 1);
    let saved = crate::presets::load_user(&dir, synth.id, "Mine").unwrap();
    assert_eq!(saved.params.get("harmonics"), Some(&77.0), "Save writes the current values over the user preset");
    // A factory preset: Save opens the list (Save As names a new one) and writes no file.
    let factory = crate::presets::factory_for(synth.id);
    t.app.apply_preset((tr, transit_mix::INSTRUMENT_SLOT), synth, factory[0].0, &factory[0].1);
    t.frames(2);
    t.click(mid(save), 1);
    assert_eq!(crate::presets::user_names(&dir, synth.id), ["Mine"], "negative control: no file for a factory preset");
    assert!(t.painted("Factory").0 >= 1, "the list is open");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_meter_reads_the_newest_peak_on_the_fader_taper_with_peak_fall_off() {
    use super::title_bar::{meter_fractions, newest_peak_db, readout};
    use crate::widgets::level_meter::{Ballistics, meter_step};
    let mut frames = vec![0.0f32; 2048];
    assert_eq!(readout(newest_peak_db(&frames)), "-inf dB");
    frames[2000] = 0.5;
    assert_eq!(readout(newest_peak_db(&frames)), "-6.0 dB");
    // Negative control: a peak older than the newest 1024 frames does not count.
    let mut old = vec![0.0f32; 2048];
    old[10] = 0.5;
    assert_eq!(readout(newest_peak_db(&old)), "-inf dB");
    let (ok, over) = meter_fractions(-6.0);
    assert!((ok - (1.0 - crate::widgets::fader::taper(-6.0))).abs() < 1e-6 && over == 0.0);
    let (ok0, over) = meter_fractions(3.0);
    assert!(over > 0.0 && (ok0 - (1.0 - crate::widgets::fader::taper(0.0))).abs() < 1e-6, "above 0 dBFS the rest is meter.hot");
    // Peak fall-off: up at once, down over 300 ms.
    let up = meter_step(-90.0, -6.0, 1.0 / 60.0, Ballistics::PEAK_FALLOFF);
    assert_eq!(up, -6.0);
    let down = meter_step(-6.0, -90.0, 1.0 / 60.0, Ballistics::PEAK_FALLOFF);
    assert!(down < -6.0 && down > -90.0, "{down}");
}

#[test]
fn the_meter_of_a_silent_instrument_is_empty() {
    let mut t = T::new();
    let tr = t.instrument_track();
    let hub = std::sync::Arc::new(transit_mix::ScopeHub::default());
    let tap = std::sync::Arc::new(transit_mix::ScopeTap::new());
    hub.register(tr, transit_mix::INSTRUMENT_UID, &tap);
    t.app.builtin_editor.hub = Some(hub);
    t.open_card(0);
    assert!(tap.enabled(), "the open window reads the tap");
    assert_eq!(t.panel(tr).title.readout, "-inf dB");
    assert_eq!(size(t.panel(tr).title.meter_fill).0, 0.0, "silence: no fill");
}

#[test]
fn the_follow_toggle_latches_in_the_option_role() {
    let (mut t, tr) = T::open();
    let link = t.panel(tr).title.follow;
    t.click(mid(link), 1);
    assert!(t.panel(tr).title.follow_on);
    assert_eq!(t.panel(tr).title.follow_fill, TOKENS.toggle_option_fill);
    assert_eq!(t.app.ui.editor_follow, vec![(tr, transit_mix::INSTRUMENT_SLOT)]);
    t.click(mid(link), 1);
    assert!(!t.panel(tr).title.follow_on && t.app.ui.editor_follow.is_empty());
}

#[test]
fn the_title_bar_preset_list_stays_open_for_save_as_then_saves_and_closes() {
    let dir = std::env::temp_dir().join(format!("transit-326p-saveas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut t, tr) = T::open();
    t.app.services.config_dir = Some(dir.clone());
    let synth = info();
    let field = t.panel(tr).title.field;
    t.click(mid(field), 1);
    assert!(t.painted("Factory").0 >= 1, "the list opens");
    // Click into the Save As field: the list stays open.
    let hint = t.painted("Save As\u{2026}").1.expect("the Save As field is in the list");
    t.click(hint, 1);
    assert!(t.painted("Factory").0 >= 1, "the click into the field does not close the list");
    t.send("ui.text", serde_json::json!({"text": "Fresh"}));
    assert!(t.painted("Factory").0 >= 1, "typing keeps it open");
    assert!(crate::presets::user_names(&dir, synth.id).is_empty(), "nothing is saved before Enter");
    t.key("enter");
    assert_eq!(crate::presets::user_names(&dir, synth.id), ["Fresh"], "Enter saves under the typed name");
    assert_eq!(t.painted("Factory").0, 0, "and the list closes");
    // A pick closes the list; Escape closes it too.
    t.click(mid(field), 1);
    let pick = t.painted("Init").1.expect("a factory preset is listed");
    t.click(pick, 1);
    assert_eq!(t.painted("Factory").0, 0, "a pick closes the list");
    t.click(mid(field), 1);
    assert!(t.painted("Factory").0 >= 1);
    t.key("escape");
    assert_eq!(t.painted("Factory").0, 0, "Escape closes it");
    let _ = std::fs::remove_dir_all(dir);
}

/// Every rect the bar records, named.
fn bar_rects(b: &super::TitleLayout) -> Vec<(&'static str, [f32; 4])> {
    vec![
        ("track", b.track_rect),
        ("device", b.device_rect),
        ("prev", b.prev),
        ("field", b.field),
        ("next", b.next),
        ("save", b.save),
        ("meter", b.meter),
        ("readout", b.readout_rect),
        ("follow", b.follow),
        ("bypass", b.bypass),
        ("menu", b.menu),
        ("close", b.close),
    ]
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The pairs of rects that intersect and the rects outside the bar.
fn faults(b: &super::TitleLayout) -> Vec<String> {
    let r = bar_rects(b);
    let mut out = Vec::new();
    for (i, (n, a)) in r.iter().enumerate() {
        if a[0] < b.bar[0] || a[2] > b.bar[2] || a[1] < b.bar[1] || a[3] > b.bar[3] {
            out.push(format!("{n} outside the bar"));
        }
        for (m, c) in &r[i + 1..] {
            if overlaps(*a, *c) {
                out.push(format!("{n} meets {m}"));
            }
        }
    }
    out
}

#[test]
fn the_bar_reflows_without_overlap_from_the_minimum_to_the_design_width() {
    // The minimum, the design width, and 1040 (the minimum before 326-X, where the bar broke:
    // the reflow holds below the minimum too, and there the old centering fails).
    const OLD_MIN: f32 = 1040.0;
    for width in [super::MIN_WIDTH, super::DESIGN_WIDTH, OLD_MIN] {
        let (mut t, tr) = T::open();
        t.app.builtin_editor.macro_osc.min_override = Some(width.min(super::MIN_WIDTH));
        t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 900.0);
        t.frames(3);
        let p = t.panel(tr);
        assert_eq!(p.window[2] - p.window[0], width);
        let b = p.title;
        assert_eq!(faults(&b), Vec::<String>::new(), "at {width}");
        assert!(size(b.field).0 >= FIELD_MIN && size(b.field).0 <= FIELD_W + 0.01, "at {width}: {:?}", b.field);
        let full = crate::widgets::role_size(
            &egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING),
            &crate::tokens::TITLE_DEVICE,
            &b.device,
        )
        .x;
        assert!((b.device_rect[2] - b.device_rect[0] - full).abs() < 0.01, "at {width}: the device title in full");
        if width == OLD_MIN {
            // Negative control: the browser centered on the bar (before 326-X) meets a side group.
            let w = preset_width(FIELD_W);
            let x = (b.bar[0] + b.bar[2]) / 2.0 - w / 2.0;
            let (y0, y1) = (b.prev[1], b.prev[3]);
            let mut old = b.clone();
            old.prev = [x, y0, x + BUTTON, y1];
            old.field = [x + BUTTON + 4.0, y0, x + BUTTON + 4.0 + FIELD_W, y1];
            old.next = [old.field[2] + 4.0, y0, old.field[2] + 4.0 + BUTTON, y1];
            old.save = [old.next[2] + 8.0, y0, old.next[2] + 8.0 + BUTTON, y1];
            assert!(!faults(&old).is_empty(), "the old centering would overlap at {width}");
        }
    }
}

#[test]
fn the_field_shrinks_to_its_floor_before_the_device_title_truncates() {
    // Room for the widest field: it centers in the free space.
    let (x, field, cut) = preset_layout(300.0, 1000.0, 200.0);
    assert_eq!((field, cut), (FIELD_W, None));
    assert_eq!(x - 312.0, 988.0 - (x + preset_width(FIELD_W)), "centered between the groups");
    // Less room: the field narrows, nothing else moves.
    let (_, field, cut) = preset_layout(300.0, 700.0, 200.0);
    assert!(field < FIELD_W && field > FIELD_MIN && cut.is_none(), "{field}");
    // Below the floor: the field keeps 160 against the right group, the device title gives way.
    let (x, field, cut) = preset_layout(300.0, 560.0, 200.0);
    assert_eq!(field, FIELD_MIN);
    assert_eq!(x + preset_width(FIELD_MIN), 548.0, "against the right group");
    assert_eq!(cut, Some(x - 12.0 - 200.0), "the device title ends a gap before the browser");
    // Negative control: never a field under the floor.
    assert_eq!(preset_layout(300.0, 320.0, 200.0).1, FIELD_MIN);
}
