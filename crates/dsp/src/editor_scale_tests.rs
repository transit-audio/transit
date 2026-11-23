//! The Scale decision (issue #153) for each kind of editor the issue names, the no-double-factor
//! rule over every input, and the setting's saved form.

#![allow(clippy::unwrap_used)]

use super::*;

const SETTINGS_MODES: [Option<ScaleMode>; 4] = [None, Some(ScaleMode::HostFactor), Some(ScaleMode::PluginZoom), Some(ScaleMode::WindowsStretch)];

fn win(support: ScaleSupport, display: f64) -> ScaleInputs {
    ScaleInputs { support, display, platform: Platform::Windows, kept_size: false, stretch_available: true }
}

fn set(step: ScaleStep, mode: Option<ScaleMode>) -> EditorScale {
    EditorScale { step, mode }
}

fn enabled(plan: &ScalePlan) -> Vec<ScaleStep> {
    plan.steps.iter().filter(|(_, e)| *e).map(|(s, _)| *s).collect()
}

fn every_setting() -> Vec<EditorScale> {
    let mut out = Vec::new();
    for step in all_steps() {
        for mode in SETTINGS_MODES {
            out.push(set(step, mode));
        }
    }
    out
}

/// The first kind: an editor that takes the host's factor draws itself sharp at the display's
/// scale under Auto, and at a chosen percentage otherwise; nothing is stretched.
#[test]
fn an_editor_that_takes_the_factor_draws_itself_at_the_display_scale_or_the_chosen_step() {
    let auto = decide(EditorScale::default(), win(ScaleSupport::Accepts, 1.5));
    assert_eq!((auto.factor, auto.stretch, auto.mode), (Some(1.5), None, ScaleMode::HostFactor));
    assert_eq!(enabled(&auto).len(), 7, "every step");
    assert_eq!(auto.reason, None);
    assert_eq!(auto.in_use, "In use: drawn by the plug-in at 150%");
    let chosen = decide(set(ScaleStep::Percent(200), None), win(ScaleSupport::Accepts, 1.5));
    assert_eq!((chosen.factor, chosen.stretch), (Some(2.0), None));
    let small = decide(set(ScaleStep::Percent(75), None), win(ScaleSupport::Accepts, 1.0));
    assert_eq!(small.factor, Some(0.75));
}

/// The second kind (K74): an editor that refuses the factor is stretched by Windows to the
/// display's scale under Auto; the only other size is its own 100 percent; the other steps are
/// disabled with a reason.
#[test]
fn an_editor_that_refuses_is_stretched_by_windows_and_offers_only_100_and_the_display_scale() {
    let auto = decide(EditorScale::default(), win(ScaleSupport::Refuses, 1.5));
    assert_eq!((auto.factor, auto.stretch, auto.mode), (None, Some(1.5), ScaleMode::WindowsStretch));
    assert_eq!(enabled(&auto), [ScaleStep::Auto, ScaleStep::Percent(100), ScaleStep::Percent(150)]);
    assert!(auto.reason.as_deref().unwrap().contains("100% (its own drawing) or the display's 150%"), "{:?}", auto.reason);
    assert_eq!(auto.in_use, "In use: stretched by Windows to 150%");
    let own = decide(set(ScaleStep::Percent(100), None), win(ScaleSupport::Refuses, 1.5));
    assert_eq!((own.factor, own.stretch, own.mode), (None, None, ScaleMode::PluginZoom), "100%: its own drawing");
    let display = decide(set(ScaleStep::Percent(150), None), win(ScaleSupport::Refuses, 1.5));
    assert_eq!(display.stretch, Some(1.5));
    // A step it cannot offer (saved when the editor was another kind) acts as Auto.
    let other = decide(set(ScaleStep::Percent(200), None), win(ScaleSupport::Refuses, 1.5));
    assert_eq!((other.step, other.stretch), (ScaleStep::Auto, Some(1.5)));
    // At a display of 125% the offered sizes are 100 and 125.
    assert_eq!(
        enabled(&decide(EditorScale::default(), win(ScaleSupport::Refuses, 1.25))),
        [ScaleStep::Auto, ScaleStep::Percent(100), ScaleStep::Percent(125)]
    );
    // The host factor cannot be chosen for it; picking it anyway acts as automatic.
    let forced = decide(set(ScaleStep::Auto, Some(ScaleMode::HostFactor)), win(ScaleSupport::Refuses, 1.5));
    assert!(forced.mode_unavailable);
    assert_eq!((forced.factor, forced.stretch), (None, Some(1.5)));
    assert!(forced.modes.iter().any(|(m, ok)| *m == ScaleMode::HostFactor && ok.is_err()));
}

/// On a display at 100% there is nothing to stretch: the refusing editor keeps its own drawing.
#[test]
fn nothing_is_stretched_on_a_display_at_100_percent() {
    let p = decide(EditorScale::default(), win(ScaleSupport::Refuses, 1.0));
    assert_eq!((p.factor, p.stretch, p.mode), (None, None, ScaleMode::PluginZoom));
    assert_eq!(enabled(&p), [ScaleStep::Auto, ScaleStep::Percent(100)]);
}

/// The third kind: an editor that accepts the factor and keeps its size. Auto cannot tell it from
/// the first kind (accepted is not applied), so it sends the factor and a hint names the other
/// modes; the user can then pick the Windows stretch or the plug-in's own zoom, and neither sends
/// a factor.
#[test]
fn an_editor_that_accepts_and_ignores_the_factor_can_be_stretched_or_left_to_its_own_zoom() {
    let mut inputs = win(ScaleSupport::Accepts, 1.5);
    inputs.kept_size = true;
    let auto = decide(EditorScale::default(), inputs);
    assert_eq!((auto.factor, auto.stretch, auto.mode), (Some(1.5), None, ScaleMode::HostFactor));
    assert!(auto.hint.as_deref().unwrap().contains("Stretched by Windows"), "{:?}", auto.hint);
    let stretched = decide(set(ScaleStep::Auto, Some(ScaleMode::WindowsStretch)), inputs);
    assert_eq!((stretched.factor, stretched.stretch, stretched.mode), (None, Some(1.5), ScaleMode::WindowsStretch));
    assert_eq!(stretched.hint, None);
    let zoom = decide(set(ScaleStep::Percent(150), Some(ScaleMode::PluginZoom)), inputs);
    assert_eq!((zoom.factor, zoom.stretch, zoom.mode), (None, None, ScaleMode::PluginZoom));
    assert_eq!(enabled(&zoom), [ScaleStep::Auto], "its own zoom control sets the size");
    assert!(zoom.reason.as_deref().unwrap().contains("own zoom"));
    // No hint at a factor of 1 (nothing to ignore), nor without the kept size.
    assert_eq!(decide(EditorScale::default(), ScaleInputs { display: 1.0, ..inputs }).hint, None);
    assert_eq!(decide(EditorScale::default(), win(ScaleSupport::Accepts, 1.5)).hint, None);
}

/// An editor whose mode sends no factor was not asked: it may still take one, so the host factor
/// stays on offer and Automatic asks it.
#[test]
fn an_editor_not_asked_keeps_the_host_factor_on_offer() {
    let p = decide(EditorScale::default(), win(ScaleSupport::Unasked, 1.5));
    assert_eq!((p.factor, p.mode), (Some(1.5), ScaleMode::HostFactor));
    assert!(p.modes.iter().all(|(_, ok)| ok.is_ok()));
}

/// The rule for every input: never a factor and a stretch together; no factor to an editor that
/// refused; a stretch only on Windows and only with the display's scale; macOS gets neither.
#[test]
fn no_factor_is_ever_applied_twice() {
    let supports = [ScaleSupport::Accepts, ScaleSupport::Refuses, ScaleSupport::Unasked];
    let platforms = [Platform::Windows, Platform::MacOs, Platform::Linux];
    for setting in every_setting() {
        for support in supports {
            for platform in platforms {
                for display in [1.0, 1.25, 1.5, 1.75, 2.0] {
                    for kept_size in [false, true] {
                        let inputs = ScaleInputs { support, display, platform, kept_size, stretch_available: true };
                        let p = decide(setting, inputs);
                        let what = format!("{setting:?} {inputs:?} -> {p:?}");
                        assert!(p.factor.is_none() || p.stretch.is_none(), "factor and stretch: {what}");
                        assert!(p.factor.is_none() || support != ScaleSupport::Refuses, "a factor to an editor that refused: {what}");
                        if let Some(s) = p.stretch {
                            assert_eq!(platform, Platform::Windows, "{what}");
                            assert!(near(s, display), "Windows stretches by the display's scale only: {what}");
                        }
                        if platform == Platform::MacOs {
                            assert_eq!((p.factor, p.stretch), (None, None), "{what}");
                        }
                        assert_eq!(p.steps.len(), 7, "{what}");
                        let disabled = p.steps.iter().any(|(_, e)| !e);
                        assert!(!disabled || p.reason.is_some(), "a disabled step without a reason: {what}");
                        let m = menu(setting, inputs, None);
                        let checked_steps = m.iter().take(9).filter(|e| matches!(e, ScaleEntry::Choice { checked: true, .. })).count();
                        assert_eq!(checked_steps, 1, "one step checked: {what}");
                    }
                }
            }
        }
    }
}

/// Linux: only an editor that takes the factor can be scaled; there is no stretch.
#[test]
fn on_linux_only_an_editor_that_takes_the_factor_is_scaled() {
    let linux = |support| ScaleInputs { support, display: 1.5, platform: Platform::Linux, kept_size: false, stretch_available: true };
    let takes = decide(set(ScaleStep::Percent(125), None), linux(ScaleSupport::Accepts));
    assert_eq!((takes.factor, takes.stretch), (Some(1.25), None));
    assert!(takes.modes.iter().any(|(m, ok)| *m == ScaleMode::WindowsStretch && *ok == Err("Windows only")));
    let refuses = decide(EditorScale::default(), linux(ScaleSupport::Refuses));
    assert_eq!((refuses.factor, refuses.stretch, refuses.mode), (None, None, ScaleMode::PluginZoom));
    assert_eq!(enabled(&refuses), [ScaleStep::Auto]);
    assert!(refuses.reason.as_deref().unwrap().starts_with("On Linux only editors that take a scale"));
    let stretch = decide(set(ScaleStep::Auto, Some(ScaleMode::WindowsStretch)), linux(ScaleSupport::Accepts));
    assert!(stretch.mode_unavailable);
    assert_eq!(stretch.stretch, None);
}

/// macOS scales editors itself: the menu says so and every choice is disabled.
#[test]
fn on_macos_the_menu_says_the_system_scales_and_disables_every_choice() {
    let inputs = ScaleInputs { support: ScaleSupport::Accepts, display: 2.0, platform: Platform::MacOs, kept_size: false, stretch_available: true };
    let p = decide(set(ScaleStep::Percent(150), Some(ScaleMode::HostFactor)), inputs);
    assert!(enabled(&p).is_empty());
    assert_eq!(p.in_use, "In use: scaled by macOS");
    let m = menu(EditorScale::default(), inputs, None);
    assert!(m.iter().all(|e| !matches!(e, ScaleEntry::Choice { enabled: true, .. })), "{m:?}");
    assert!(m.contains(&ScaleEntry::Info("macOS scales plug-in editors itself: transit does not change their size".into())));
}

/// An open editor takes a new factor at once; a stretch turned on or off, or a factor withdrawn,
/// waits for the next open, and the menu says so.
#[test]
fn a_new_factor_applies_at_once_and_a_stretch_or_a_withdrawn_factor_on_the_next_open() {
    let w = win(ScaleSupport::Accepts, 1.5);
    let at = |s: EditorScale| decide(s, w);
    let auto = at(EditorScale::default());
    assert_eq!(applies(&auto, &at(set(ScaleStep::Percent(200), None))), Applies::Now);
    assert_eq!(applies(&auto, &at(set(ScaleStep::Percent(150), None))), Applies::Unchanged, "the same factor");
    assert_eq!(applies(&auto, &at(set(ScaleStep::Auto, Some(ScaleMode::WindowsStretch)))), Applies::NextOpen);
    assert_eq!(applies(&auto, &at(set(ScaleStep::Auto, Some(ScaleMode::PluginZoom)))), Applies::NextOpen, "a factor cannot be taken back");
    let zoom = at(set(ScaleStep::Auto, Some(ScaleMode::PluginZoom)));
    assert_eq!(applies(&zoom, &auto), Applies::Now, "a factor sent to an editor that can take one");
    let r = win(ScaleSupport::Refuses, 1.5);
    let stretched = decide(EditorScale::default(), r);
    let own = decide(set(ScaleStep::Percent(100), None), r);
    assert_eq!(applies(&stretched, &own), Applies::NextOpen);
    // The menu of an open stretched editor whose setting now asks for its own drawing.
    let m = menu(set(ScaleStep::Percent(100), None), r, Some(&stretched));
    assert_eq!(m[0], ScaleEntry::Info("In use: stretched by Windows to 150%".into()));
    assert_eq!(m[1], ScaleEntry::Info("Next open: the plug-in's own drawing (100%)".into()));
}

/// The menu: the in-use line, Auto with the display's scale, the steps and modes with the setting
/// each stores, disabled steps with the reason line.
#[test]
fn the_menu_lists_steps_and_modes_with_the_setting_each_picks() {
    let m = menu(EditorScale::default(), win(ScaleSupport::Refuses, 1.5), None);
    let choices: Vec<(String, EditorScale, bool, bool)> = m
        .iter()
        .filter_map(|e| match e {
            ScaleEntry::Choice { label, value, checked, enabled } => Some((label.clone(), *value, *checked, *enabled)),
            _ => None,
        })
        .collect();
    assert_eq!(choices[0], ("Auto (150%)".into(), EditorScale::default(), true, true));
    assert_eq!(choices[1], ("50%".into(), set(ScaleStep::Percent(50), None), false, false));
    assert_eq!(choices[3], ("100%".into(), set(ScaleStep::Percent(100), None), false, true));
    assert_eq!(choices[7], ("Automatic: Stretched by Windows".into(), EditorScale::default(), true, true));
    assert_eq!(
        choices[8],
        (
            "Drawn by the plug-in at the host's factor (the plug-in does not take a scale from the host)".into(),
            set(ScaleStep::Auto, Some(ScaleMode::HostFactor)),
            false,
            false
        )
    );
    assert_eq!(choices.len(), 11);
    assert!(matches!(m.last(), Some(ScaleEntry::Info(r)) if r.contains("stretched by Windows")));
}

/// The saved form: `{"step": "auto" | <percent>, "mode": null | "host_factor" | ...}`; a missing
/// key takes its default, and a value that is not one of these does not load (as every other
/// preference: the settings loader keeps that field at its default).
#[test]
fn the_setting_round_trips_and_unknown_values_do_not_load() {
    for s in every_setting() {
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(serde_json::from_value::<EditorScale>(v).unwrap(), s);
    }
    assert_eq!(
        serde_json::to_value(set(ScaleStep::Percent(150), Some(ScaleMode::WindowsStretch))).unwrap(),
        serde_json::json!({"step": 150, "mode": "windows_stretch"})
    );
    assert_eq!(serde_json::to_value(EditorScale::default()).unwrap(), serde_json::json!({"step": "auto", "mode": null}));
    assert_eq!(serde_json::from_value::<EditorScale>(serde_json::json!({})).unwrap(), EditorScale::default());
    for odd in [serde_json::json!({"step": 333}), serde_json::json!({"step": "huge"}), serde_json::json!({"mode": "sideways"})] {
        assert!(serde_json::from_value::<EditorScale>(odd.clone()).is_err(), "{odd} loads");
    }
}

/// A Windows that cannot host the DPI-unaware container (before 1803): no stretch is offered, and
/// a refusing editor keeps its own drawing with the reason said.
#[test]
fn without_the_mixed_hosting_windows_offers_no_stretch() {
    let inputs = ScaleInputs { stretch_available: false, ..win(ScaleSupport::Refuses, 1.5) };
    let p = decide(EditorScale::default(), inputs);
    assert_eq!((p.factor, p.stretch, p.mode), (None, None, ScaleMode::PluginZoom));
    assert!(p.modes.iter().any(|(m, ok)| *m == ScaleMode::WindowsStretch && *ok == Err("needs Windows 10 version 1803 or later")));
    assert!(p.reason.as_deref().unwrap().contains("cannot stretch"));
}

/// One editor's state through an open, a change while open, a DPI change and a menu pick.
#[test]
fn the_editor_state_follows_an_open_a_change_a_dpi_change_and_a_pick() {
    let mut s = ScaleState { stretch_available: true, ..ScaleState::default() };
    // Auto on an editor with the call: ask with the display's factor; it accepts.
    let first = s.before_open(1.5, true);
    assert_eq!(first.factor, Some(1.5));
    let plan = s.opened(Some((true, false)));
    assert_eq!((plan.factor, plan.stretch), (Some(1.5), None));
    assert!(s.log_line().starts_with("accepts a factor: yes; factor in use: 1.5; Windows stretch: none; in use: drawn by the plug-in at 150%"));
    // 200% while open: sent at once.
    let (applies, wanted) = s.set_setting(EditorScale { step: ScaleStep::Percent(200), mode: None });
    assert_eq!((applies, wanted.factor), (Applies::Now, Some(2.0)));
    s.applied(true, false);
    assert_eq!(s.in_use.as_ref().unwrap().factor, Some(2.0));
    // A chosen percentage stays on a DPI change.
    assert_eq!(s.dpi_changed(1.25), None);
    // Back to Auto, then a DPI change sends the new display scale.
    let _ = s.set_setting(EditorScale::default());
    s.applied(true, false);
    assert_eq!(s.dpi_changed(2.0), Some(2.0));
    // Stretched by Windows while open: next open, and the menu says so.
    let (applies, _) = s.set_setting(EditorScale { mode: Some(ScaleMode::WindowsStretch), ..EditorScale::default() });
    assert_eq!(applies, Applies::NextOpen);
    let entries = s.menu().to_vec();
    assert!(matches!(&entries[1], ScaleEntry::Info(t) if t == "Next open: stretched by Windows to 200%"), "{entries:?}");
    // A pick maps the entry's index to the setting it stores; a disabled or info entry is none.
    let idx = entries.iter().position(|e| matches!(e, ScaleEntry::Choice { label, .. } if label == "75%")).unwrap();
    assert_eq!(s.pick(idx), None, "75% is not offered while stretched");
    let auto = entries.iter().position(|e| matches!(e, ScaleEntry::Choice { label, .. } if label.starts_with("Automatic"))).unwrap();
    assert_eq!(s.pick(auto), Some(EditorScale::default()));
    assert_eq!(s.pick(0), None, "the in-use line");
    // Closed and opened again: the stretch is in use; the factor is not asked (the mode sends none).
    s.closed();
    let again = s.before_open(1.5, true);
    assert_eq!(again.factor, None);
    let plan = s.opened(None);
    assert_eq!((plan.factor, plan.stretch), (None, Some(1.5)));
    // An editor without the call refuses without being asked.
    let mut r = ScaleState { stretch_available: true, ..ScaleState::default() };
    assert_eq!(r.before_open(1.5, false).stretch, Some(1.5));
    assert_eq!(r.opened(None).factor, None);
    assert!(r.log_line().starts_with("accepts a factor: no; factor in use: none; Windows stretch: 1.5"));
}
