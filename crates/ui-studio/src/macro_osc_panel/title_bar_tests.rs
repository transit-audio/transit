//! The device title bar: its anatomy and tokens, the preset browser over the existing preset
//! system, the output meter, bypass and the follow toggle.

use super::test_rig::{T, info, mid};
use super::title_bar::{BUTTON, FIELD_W, METER_W, stepped};
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
    assert_eq!(size(b.field).0, FIELD_W);
    assert_eq!(size(b.meter).0, METER_W);
    // The field is centered on the bar; prev, field, next, save run left to right.
    let center = (b.bar[0] + b.bar[2]) / 2.0;
    assert!(b.prev[2] < b.field[0] && b.field[2] < b.next[0] && b.next[2] < b.save[0]);
    assert!(((b.prev[0] + b.save[2]) / 2.0 - center).abs() < 1.0, "the browser is centered");
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
