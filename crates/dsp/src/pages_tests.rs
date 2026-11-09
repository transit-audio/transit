//! The factory pages of every built-in (ADR 0047 decision 2.5): declared pages validate against
//! the device's own table, and the devices still on the derived page are an allow-list that may
//! only shrink (the ratchet pattern of the file-size gate).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Category, DERIVED_PAGE_NAME, PAGE_SLOTS, PageInfo, PageSlot, ParamInfo, PluginInfo, Unit};

/// Built-ins still on the derived page (no `PAGES` declared). A device leaves the list when its
/// panel migration declares `PAGES`; a new built-in declares `PAGES` at birth and never joins it.
const DERIVED_PAGE_ALLOW_LIST: &[&str] = &[
    "eq_1band",
    "eq_7band",
    "filter",
    "compressor",
    "expander_gate",
    "room_reverb",
    "plate_reverb",
    "saturator",
    "texture",
    "time_stretcher",
    "cross_modulator",
    "gain",
    "trim",
    "invert",
    "dc_offset_removal",
    "signal_generator",
    "dither",
    "tuner",
    "sampler",
    "resonator",
    "texture_synth",
    "modal_voice",
    "midi_transpose",
    "midi_velocity",
    "random_sequencer",
];

/// The allow-list's length when the ratchet was set. Lower it when a device leaves the list;
/// never raise it.
const ALLOW_LIST_CEILING: usize = 25;

/// Every built-in: the instrument and effect registry, then the MIDI effect registry.
fn built_ins() -> impl Iterator<Item = &'static PluginInfo> {
    crate::plugins().iter().copied().chain(crate::midi_fx::REGISTRY.iter().copied())
}

/// Decision 2.5's rules for one device's declared pages.
fn validate(info: &PluginInfo) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for page in info.pages {
        if page.name.trim().is_empty() {
            return Err(format!("{}: a page has no name", info.id));
        }
        if !names.insert(page.name) {
            return Err(format!("{}: page name `{}` appears twice", info.id, page.name));
        }
        if page.slots.len() > PAGE_SLOTS {
            return Err(format!("{}: page `{}` has {} slots, at most {PAGE_SLOTS}", info.id, page.name, page.slots.len()));
        }
        let mut seen = std::collections::HashSet::new();
        for slot in page.slots {
            if info.param(slot.param).is_none() {
                return Err(format!("{}: page `{}` names `{}`, not in the table", info.id, page.name, slot.param));
            }
            if !seen.insert(slot.param) {
                return Err(format!("{}: page `{}` holds `{}` twice", info.id, page.name, slot.param));
            }
            if let Some(enable) = slot.enable {
                match info.param(enable) {
                    None => return Err(format!("{}: page `{}` enables `{}` by `{enable}`, not in the table", info.id, page.name, slot.param)),
                    Some(p) if p.unit != Unit::Toggle => {
                        return Err(format!("{}: page `{}` enables `{}` by `{enable}`, which is not a toggle", info.id, page.name, slot.param));
                    }
                    Some(_) => {}
                }
            }
        }
    }
    Ok(())
}

#[test]
fn every_built_in_page_validates_and_the_derived_page_list_only_shrinks() {
    let mut ids = std::collections::HashSet::new();
    for info in built_ins() {
        assert!(ids.insert(info.id), "{} is registered twice", info.id);
        validate(info).unwrap();
        let listed = DERIVED_PAGE_ALLOW_LIST.contains(&info.id);
        if info.pages.is_empty() {
            assert!(listed, "{} declares no PAGES: a new built-in declares them at birth (ADR 0047 decision 2.4)", info.id);
            // The derived rule: the first eight parameters in table order, "Main".
            let derived: Vec<&str> = info.derived_page().map(|p| p.id).collect();
            let expected: Vec<&str> = info.params.iter().take(PAGE_SLOTS).map(|p| p.id).collect();
            assert_eq!(derived, expected, "{}", info.id);
            assert_eq!(derived.len(), info.params.len().min(PAGE_SLOTS), "{}", info.id);
        } else {
            assert!(!listed, "{} declares PAGES: take it off the allow-list and lower the ceiling", info.id);
        }
    }
    for id in DERIVED_PAGE_ALLOW_LIST {
        assert!(ids.contains(id), "allow-list entry `{id}` is not a built-in: remove it and lower the ceiling");
    }
    assert!(DERIVED_PAGE_ALLOW_LIST.len() <= ALLOW_LIST_CEILING, "the derived-page allow-list grew past {ALLOW_LIST_CEILING}");
    assert_eq!(DERIVED_PAGE_NAME, "Main");
}

#[test]
fn the_macro_oscillators_main_page_is_the_adr_list() {
    let info = crate::plugin_info("macro_oscillator").unwrap();
    assert_eq!(info.pages.len(), 1);
    let main = &info.pages[0];
    assert_eq!(main.name, "Main");
    let slots: Vec<(&str, Option<&str>)> = main.slots.iter().map(|s| (s.param, s.enable)).collect();
    assert_eq!(
        slots,
        vec![
            ("model", None),
            ("harmonics", None),
            ("timbre", None),
            ("morph", None),
            ("level", None),
            ("cutoff", Some("shaper")),
            ("amp_attack", Some("shaper")),
            ("amp_release", Some("shaper")),
        ]
    );
}

const TOGGLE: ParamInfo =
    ParamInfo { id: "on", name: "On", min: 0.0, max: 1.0, default: 0.0, unit: Unit::Toggle, taper: crate::Taper::Linear, choices: &[], group: "", note_labels: None };
const KNOB: ParamInfo = ParamInfo { id: "knob", unit: Unit::Percent, max: 100.0, ..TOGGLE };
static TABLE: [ParamInfo; 2] = [TOGGLE, KNOB];

fn device(pages: &'static [PageInfo]) -> PluginInfo {
    PluginInfo {
        id: "probe",
        name: "Probe",
        short_name: "Probe",
        category: Category::Other,
        params: &TABLE,
        is_instrument: false,
        audiosuite: false,
        pages,
    }
}

/// Controls: the validation fails for each rule broken on its own (a page naming an id not in
/// the table first), so the test above can fail.
#[test]
fn the_validation_fails_for_each_broken_rule() {
    static GOOD: [PageInfo; 1] = [PageInfo { name: "Main", slots: &[PageSlot::new("knob"), PageSlot::enabled_by("knob2", "on")] }];
    let err = validate(&device(&GOOD)).unwrap_err();
    assert!(err.contains("`knob2`, not in the table"), "{err}");
    static OK: [PageInfo; 2] = [
        PageInfo { name: "Main", slots: &[PageSlot::enabled_by("knob", "on"), PageSlot::new("on")] },
        PageInfo { name: "Two", slots: &[PageSlot::new("knob")] },
    ];
    validate(&device(&OK)).unwrap();
    static UNKNOWN_ENABLE: [PageInfo; 1] = [PageInfo { name: "Main", slots: &[PageSlot::enabled_by("knob", "off")] }];
    assert!(validate(&device(&UNKNOWN_ENABLE)).unwrap_err().contains("not in the table"));
    static NOT_TOGGLE: [PageInfo; 1] = [PageInfo { name: "Main", slots: &[PageSlot::enabled_by("on", "knob")] }];
    assert!(validate(&device(&NOT_TOGGLE)).unwrap_err().contains("not a toggle"));
    static TWICE: [PageInfo; 1] = [PageInfo { name: "Main", slots: &[PageSlot::new("knob"), PageSlot::new("knob")] }];
    assert!(validate(&device(&TWICE)).unwrap_err().contains("twice"));
    static SAME_NAME: [PageInfo; 2] = [PageInfo { name: "Main", slots: &[] }, PageInfo { name: "Main", slots: &[] }];
    assert!(validate(&device(&SAME_NAME)).unwrap_err().contains("appears twice"));
    static NO_NAME: [PageInfo; 1] = [PageInfo { name: " ", slots: &[] }];
    assert!(validate(&device(&NO_NAME)).unwrap_err().contains("no name"));
    static NINE: [PageInfo; 1] = [PageInfo { name: "Main", slots: &[PageSlot::new("knob"); 9] }];
    assert!(validate(&device(&NINE)).unwrap_err().contains("at most 8"));
}
