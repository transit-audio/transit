//! The page resolver as pure functions (ADR 0047 slice 1): slot states, the hosted derivation at
//! the 64-parameter rule, and the current page's identity rules of decision 1.3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::device_pages::*;
use transit_dsp::{Category, PAGE_SLOTS, ParamInfo, PluginInfo, Taper, Unit};
use transit_model::UserPage;

fn user(name: &str, ids: &[Option<&str>]) -> UserPage {
    let mut p = UserPage::named(name);
    for (slot, id) in p.slots.iter_mut().zip(ids) {
        *slot = id.map(str::to_owned);
    }
    p
}

fn states(p: &Page) -> Vec<SlotState> {
    p.slots.iter().map(|s| s.state).collect()
}

fn ids(p: &Page) -> Vec<Option<&str>> {
    p.slots.iter().map(|s| s.id.as_deref()).collect()
}

/// A hosted description with `n` parameters `1000`, `1001`, ... (leaked: a test's own table).
fn hosted(n: usize) -> &'static PluginInfo {
    let params: Vec<ParamInfo> = (0..n)
        .map(|i| ParamInfo {
            id: Box::leak((1000 + i).to_string().into_boxed_str()),
            name: Box::leak(format!("P{i}").into_boxed_str()),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: Unit::None,
            taper: Taper::Linear,
            choices: &[],
            group: "",
            note_labels: None,
        })
        .collect();
    Box::leak(Box::new(PluginInfo {
        id: "clap:test.hosted",
        name: "Hosted",
        short_name: "Hosted",
        category: Category::Other,
        params: Box::leak(params.into_boxed_slice()),
        is_instrument: false,
        audiosuite: false,
        pages: &[],
    }))
}

#[test]
fn a_missing_id_resolves_missing_at_its_own_index_and_the_others_keep_theirs() {
    let eq = transit_dsp::plugin_info("eq_7band").unwrap();
    let first = eq.params[0].id;
    let third = eq.params[2].id;
    let page = user("Mine", &[Some(first), Some("gone_param"), Some(third), None, Some("also_gone")]);
    let pages = resolve(Table::BuiltIn(eq), &[page]);
    assert_eq!(pages.len(), 2, "the derived page, then the user page");
    let u = &pages[1];
    assert_eq!((u.tier, u.name.as_str()), (Tier::User, "Mine"));
    use SlotState::*;
    assert_eq!(states(u), vec![Ok, Missing, Ok, Empty, Missing, Empty, Empty, Empty]);
    assert_eq!(ids(u)[1], Some("gone_param"), "a missing slot keeps its id");
    assert_eq!(ids(u)[4], Some("also_gone"));
    assert_eq!(u.slots[2].name.as_deref(), Some(eq.params[2].name));
    assert_eq!(u.slots[1].name, None);
}

#[test]
fn an_unavailable_plug_ins_slots_are_missing_and_a_loading_ones_pending() {
    let page = user("", &[Some("1032"), None, Some("1033")]);
    let gone = resolve(classify(None, true, false), std::slice::from_ref(&page));
    assert_eq!(gone.len(), 1, "no factory tier without a table");
    assert_eq!(gone[0].name, "User 1");
    assert_eq!(states(&gone[0])[..3], [SlotState::Missing, SlotState::Empty, SlotState::Missing]);
    let loading = resolve(classify(None, true, true), std::slice::from_ref(&page));
    assert_eq!(states(&loading[0])[..3], [SlotState::Pending, SlotState::Empty, SlotState::Pending]);
    assert_eq!(ids(&loading[0])[0], Some("1032"));
    // Once the table is known, the same slots resolve against it.
    let known = resolve(classify(Some(hosted(40)), true, false), &[page]);
    assert_eq!(states(known.last().unwrap())[..3], [SlotState::Ok, SlotState::Empty, SlotState::Ok]);
    // A device without parameters has no pages at all, even with stored ones.
    assert!(resolve(Table::NoParameters, &[user("x", &[Some("a")])]).is_empty());
}

#[test]
fn a_hosted_table_of_64_fills_eight_pages_and_65_none() {
    let full = factory(Table::Hosted(hosted(64)));
    assert_eq!(full.len(), 8);
    assert_eq!(full.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Main", "2", "3", "4", "5", "6", "7", "8"]);
    assert!(full.iter().all(|p| p.slots.len() == PAGE_SLOTS && p.slots.iter().all(|s| s.state == SlotState::Ok)));
    assert_eq!(full[7].slots[7].id.as_deref(), Some("1063"), "table order, eight per page");
    assert!(factory(Table::Hosted(hosted(65))).is_empty(), "above 64 the factory tier is empty");
    let odd = factory(Table::Hosted(hosted(11)));
    assert_eq!(odd.len(), 2);
    assert_eq!(states(&odd[1]), [&[SlotState::Ok; 3][..], &[SlotState::Empty; 5][..]].concat());
    assert!(factory(Table::Hosted(hosted(0))).is_empty());
    // Above 64, user pages still resolve against the table.
    let big = resolve(Table::Hosted(hosted(200)), &[user("", &[Some("1150")])]);
    assert_eq!((big.len(), big[0].slots[0].state), (1, SlotState::Ok));
}

#[test]
fn built_ins_show_declared_pages_or_their_derived_page() {
    let osc = transit_dsp::plugin_info("macro_oscillator").unwrap();
    let pages = factory(Table::BuiltIn(osc));
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].name, "Main");
    assert_eq!(ids(&pages[0]), ["model", "harmonics", "timbre", "morph", "level", "cutoff", "amp_attack", "amp_release"].map(Some));
    assert_eq!(pages[0].slots.iter().map(|s| s.enable.as_deref()).collect::<Vec<_>>()[4..], [None, Some("shaper"), Some("shaper"), Some("shaper")]);
    let eq1 = transit_dsp::plugin_info("eq_1band").unwrap();
    let derived = factory(Table::BuiltIn(eq1));
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].name, "Main");
    let n = eq1.params.len().min(PAGE_SLOTS);
    assert_eq!(ids(&derived[0])[..n], eq1.params[..n].iter().map(|p| Some(p.id)).collect::<Vec<_>>()[..]);
    assert!(derived[0].slots[n..].iter().all(|s| s.state == SlotState::Empty));
    assert!(factory(Table::BuiltIn(transit_dsp::plugin_info("invert").unwrap())).is_empty(), "no parameters, no page");
    let transpose = transit_dsp::midi_fx::info("midi_transpose").unwrap();
    assert_eq!(factory(Table::BuiltIn(transpose)).len(), 1, "MIDI effects derive too");
}

/// Two factory pages and three user pages.
fn five() -> Vec<Page> {
    let mut pages = factory(Table::Hosted(hosted(16)));
    pages.extend(resolve(Table::Hosted(hosted(16)), &[user("A", &[]), user("B", &[]), user("C", &[])]).into_iter().skip(2));
    assert_eq!(pages.len(), 5);
    pages
}

#[test]
fn the_current_page_follows_the_uis_own_edits() {
    use CurrentPage::*;
    let pages = five();
    assert_eq!(first(&pages), Some(Factory(0)));
    assert_eq!(position(&pages, User(1)), Some(3));
    assert_eq!(at(&pages, 4), Some(User(2)));
    // Deleting a page before the current one shifts it; deleting the current one makes the next
    // current, else the previous; with no user page left, the last factory page.
    let without = |i: usize| {
        let mut p = pages.clone();
        p.remove(2 + i);
        p
    };
    assert_eq!(after_user_delete(&without(0), User(2), 0), Some(User(1)));
    assert_eq!(after_user_delete(&without(1), User(1), 1), Some(User(1)), "the next page");
    assert_eq!(after_user_delete(&without(2), User(2), 2), Some(User(1)), "the previous page");
    assert_eq!(after_user_delete(&without(2), User(0), 2), Some(User(0)), "a page before the deleted one stays");
    assert_eq!(after_user_delete(&without(1), Factory(1), 1), Some(Factory(1)));
    let factory_only = factory(Table::Hosted(hosted(16)));
    assert_eq!(after_user_delete(&factory_only, User(0), 0), Some(Factory(1)));
    // A move carries the current page with it.
    assert_eq!(after_user_move(User(0), 0, 2), User(2));
    assert_eq!(after_user_move(User(1), 0, 2), User(0));
    assert_eq!(after_user_move(User(2), 2, 0), User(0));
    assert_eq!(after_user_move(User(0), 2, 0), User(1));
    assert_eq!(after_user_move(Factory(1), 0, 2), Factory(1));
}

#[test]
fn after_other_changes_a_page_is_kept_while_it_exists_else_the_last_page() {
    use CurrentPage::*;
    let pages = five();
    assert_eq!(refind(&pages, User(2)), Some(User(2)));
    let shorter: Vec<Page> = pages[..4].to_vec();
    assert_eq!(refind(&shorter, User(2)), Some(User(1)), "clamped to the last page");
    let mut remote = pages.clone();
    remote[1].remote_id = Some(77);
    assert_eq!(refind(&remote, Remote(77)), Some(Remote(77)));
    remote.swap(0, 1);
    assert_eq!(position(&remote, Remote(77)), Some(0), "a CLAP page is found by its id, not its index");
    assert_eq!(refind(&pages, Remote(77)), Some(User(2)), "an id that is gone clamps");
    assert_eq!(refind(&[], User(0)), None);
    assert_eq!(indicator(&[], None), (0, 0, "No pages".to_owned()));
    assert_eq!(indicator(&pages, Some(User(1))), (4, 5, "B".to_owned()));
    assert_eq!(indicator(&pages, Some(Factory(0))), (1, 5, "Main".to_owned()));
}

#[test]
fn the_rack_page_is_its_eight_macros() {
    let mut rack = transit_model::Rack::default();
    rack.macros[2].name = "Bright".into();
    let page = rack_page(&rack);
    assert_eq!((page.tier, page.name.as_str()), (Tier::Factory, "Macros"));
    assert_eq!(page.slots[0].id.as_deref(), Some("macro1"));
    assert_eq!(page.slots[2].name.as_deref(), Some("Bright"));
    assert_eq!(page.slots[7].name.as_deref(), Some("Macro 8"));
}
