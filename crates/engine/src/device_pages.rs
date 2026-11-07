//! The device macro strip's page resolver (ADR 0047 decisions 1, 2.4, 3.1 and 4.4): a device's
//! pages as one ordered list, the factory tier first (a built-in's declared or derived pages, a
//! small hosted plug-in's derived pages) and the user tier after it (the session's
//! [`UserPage`]s), with each slot's state. Pure functions of the device's known table and its
//! stored pages: no instance, no UI state. The current page's identity rules of decision 1.3
//! live here too, for the studio to call (the current page itself is studio state).
//!
//! CLAP remote-controls pages (decision 5) join the factory tier with slice 4; [`Page::remote_id`]
//! is their identity and is `None` until then.

use transit_dsp::{DERIVED_PAGE_NAME, PAGE_SLOTS, PluginInfo};
use transit_model::{RACK_MACROS, Rack, UserPage};

/// A hosted plug-in whose registry table has at most this many parameters derives its factory
/// tier from it (eight per page, at most eight pages); above it, the factory tier is empty and
/// the strip offers Configure (decision 3.1, Live's cutoff).
pub const HOSTED_DERIVE_MAX: usize = 64;

/// The name of the rack card's one fixed page (decision 6.4).
pub const RACK_PAGE_NAME: &str = "Macros";

/// What a slot shows (decision 4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotState {
    /// The id is in the device's known table.
    Ok,
    /// No id.
    Empty,
    /// The id is not in the known table, or no table will arrive (the plug-in is not installed
    /// or failed to load). The slot keeps its id and is never compacted.
    Missing,
    /// The device's table is not known yet (a hosted plug-in loading or not responding).
    Pending,
}

/// Which tier a page belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Factory,
    User,
}

/// One resolved slot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Slot {
    /// The parameter id (kept for a missing or pending slot).
    pub id: Option<String>,
    /// The parameter's name, for an `ok` slot.
    pub name: Option<String>,
    pub state: SlotState,
    /// A factory slot's toggle that must be on for it to read as active (decision 2.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<String>,
}

impl Slot {
    fn empty() -> Self {
        Slot { id: None, name: None, state: SlotState::Empty, enable: None }
    }
}

/// One resolved page: always [`PAGE_SLOTS`] slots.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Page {
    pub tier: Tier,
    pub name: String,
    /// A CLAP remote-controls page's id (slice 4); `None` for every other page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<u32>,
    pub slots: Vec<Slot>,
}

/// What the resolver knows of a device's parameter table.
#[derive(Debug, Clone, Copy)]
pub enum Table<'a> {
    /// A built-in's table (the instrument and effect registry or the MIDI effect registry): its
    /// declared pages, else its derived page.
    BuiltIn(&'a PluginInfo),
    /// A hosted plug-in's registry table (hidden, read-only and meter parameters already
    /// dropped): derived pages at or below [`HOSTED_DERIVE_MAX`] parameters.
    Hosted(&'a PluginInfo),
    /// A hosted table that is not known yet.
    Pending,
    /// No table will arrive: not installed, failed to load, or an id this build does not know.
    Unavailable,
    /// The device has no parameters at all (a hardware insert, an External Instrument): no pages.
    NoParameters,
}

impl Table<'_> {
    fn info(&self) -> Option<&PluginInfo> {
        match self {
            Table::BuiltIn(i) | Table::Hosted(i) => Some(i),
            _ => None,
        }
    }
}

/// The table state from a lookup: the description when there is one, else pending while it is
/// still being read, else unavailable.
#[must_use]
pub fn classify(info: Option<&'static PluginInfo>, hosted: bool, loading: bool) -> Table<'static> {
    match (info, hosted) {
        (Some(i), false) => Table::BuiltIn(i),
        (Some(i), true) => Table::Hosted(i),
        (None, _) if loading => Table::Pending,
        (None, _) => Table::Unavailable,
    }
}

/// A device's pages, factory tier then user tier.
#[must_use]
pub fn resolve(table: Table<'_>, user: &[UserPage]) -> Vec<Page> {
    if matches!(table, Table::NoParameters) {
        return Vec::new();
    }
    let mut pages = factory(table);
    pages.extend(user.iter().enumerate().map(|(i, p)| Page {
        tier: Tier::User,
        name: p.display_name(i + 1),
        remote_id: None,
        slots: p.slots.iter().map(|id| user_slot(table, id.as_deref())).collect(),
    }));
    pages
}

/// The factory tier of a device with `table`.
#[must_use]
pub fn factory(table: Table<'_>) -> Vec<Page> {
    let page = |name: String, slots: Vec<Slot>| Page { tier: Tier::Factory, name, remote_id: None, slots: padded(slots) };
    match table {
        Table::BuiltIn(info) if !info.pages.is_empty() => info
            .pages
            .iter()
            .map(|p| {
                let slots = p
                    .slots
                    .iter()
                    .take(PAGE_SLOTS)
                    .map(|s| Slot {
                        id: Some(s.param.to_owned()),
                        name: info.param(s.param).map(|x| x.name.to_owned()),
                        state: if info.param(s.param).is_some() { SlotState::Ok } else { SlotState::Missing },
                        enable: s.enable.map(str::to_owned),
                    })
                    .collect();
                page(p.name.to_owned(), slots)
            })
            .collect(),
        Table::BuiltIn(info) => {
            let slots: Vec<Slot> = info.derived_page().map(ok_slot).collect();
            if slots.is_empty() { Vec::new() } else { vec![page(DERIVED_PAGE_NAME.to_owned(), slots)] }
        }
        Table::Hosted(info) if info.params.len() <= HOSTED_DERIVE_MAX => info
            .params
            .chunks(PAGE_SLOTS)
            .take(HOSTED_DERIVE_MAX / PAGE_SLOTS)
            .enumerate()
            .map(|(i, chunk)| page(hosted_page_name(i), chunk.iter().map(ok_slot).collect()))
            .collect(),
        Table::Hosted(_) | Table::Pending | Table::Unavailable | Table::NoParameters => Vec::new(),
    }
}

/// "Main", "2", "3" ... (decision 3.1).
fn hosted_page_name(i: usize) -> String {
    if i == 0 { DERIVED_PAGE_NAME.to_owned() } else { (i + 1).to_string() }
}

fn ok_slot(p: &transit_dsp::ParamInfo) -> Slot {
    Slot { id: Some(p.id.to_owned()), name: Some(p.name.to_owned()), state: SlotState::Ok, enable: None }
}

fn padded(mut slots: Vec<Slot>) -> Vec<Slot> {
    slots.truncate(PAGE_SLOTS);
    slots.resize_with(PAGE_SLOTS, Slot::empty);
    slots
}

/// A user slot's state against `table`.
fn user_slot(table: Table<'_>, id: Option<&str>) -> Slot {
    let Some(id) = id else { return Slot::empty() };
    let (state, name) = match table {
        Table::Pending => (SlotState::Pending, None),
        Table::Unavailable | Table::NoParameters => (SlotState::Missing, None),
        Table::BuiltIn(_) | Table::Hosted(_) => match table.info().and_then(|i| i.param(id)) {
            Some(p) => (SlotState::Ok, Some(p.name.to_owned())),
            None => (SlotState::Missing, None),
        },
    };
    Slot { id: Some(id.to_owned()), name, state, enable: None }
}

/// The rack card's strip: its eight macros as one fixed factory page (decision 6.4). The slots
/// are the macros' automation ids (`macro1` .. `macro8`), named as the card names them.
#[must_use]
pub fn rack_page(rack: &Rack) -> Page {
    let slots = (0..RACK_MACROS)
        .map(|i| Slot { id: Some(transit_model::macro_param(i)), name: Some(rack.macro_name(i)), state: SlotState::Ok, enable: None })
        .collect();
    Page { tier: Tier::Factory, name: RACK_PAGE_NAME.to_owned(), remote_id: None, slots: padded(slots) }
}

// ---- the current page's identity (decision 1.3) ----------------------------------------------

/// Which page is current, as the studio keeps it per device instance: a factory page by its
/// index in the factory tier (a CLAP page by its page id), a user page by its index among the
/// user pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentPage {
    Factory(usize),
    Remote(u32),
    User(usize),
}

/// The page's position in the whole list (0-based), if it is there.
#[must_use]
pub fn position(pages: &[Page], current: CurrentPage) -> Option<usize> {
    let factory = pages.iter().take_while(|p| p.tier == Tier::Factory).count();
    match current {
        CurrentPage::Factory(i) => (i < factory).then_some(i),
        CurrentPage::Remote(id) => pages.iter().take(factory).position(|p| p.remote_id == Some(id)),
        CurrentPage::User(i) => (factory + i < pages.len()).then_some(factory + i),
    }
}

/// The identity of the page at `index` of the whole list.
#[must_use]
pub fn at(pages: &[Page], index: usize) -> Option<CurrentPage> {
    let factory = pages.iter().take_while(|p| p.tier == Tier::Factory).count();
    let page = pages.get(index)?;
    Some(match (page.tier, page.remote_id) {
        (Tier::Factory, Some(id)) => CurrentPage::Remote(id),
        (Tier::Factory, None) => CurrentPage::Factory(index),
        (Tier::User, _) => CurrentPage::User(index - factory),
    })
}

/// The page a device starts on: the first (`None` with no pages).
#[must_use]
pub fn first(pages: &[Page]) -> Option<CurrentPage> {
    at(pages, 0)
}

/// After a change the UI did not make (undo, redo, a plug-in's `changed`, a tier switch): a CLAP
/// page is found again by its id, any other page is kept while it exists, and otherwise the last
/// page becomes current (`None` with no pages).
#[must_use]
pub fn refind(pages: &[Page], current: CurrentPage) -> Option<CurrentPage> {
    match position(pages, current) {
        Some(i) => at(pages, i),
        None => at(pages, pages.len().checked_sub(1)?),
    }
}

/// After the UI deleted user page `deleted` (`pages` is the list after the delete): a later
/// current page shifts down with it; the deleted current page hands over to the next one, else
/// the previous one (the last factory page when no user page is left).
#[must_use]
pub fn after_user_delete(pages: &[Page], current: CurrentPage, deleted: usize) -> Option<CurrentPage> {
    let users = pages.iter().filter(|p| p.tier == Tier::User).count();
    let next = match current {
        CurrentPage::User(i) if i > deleted => CurrentPage::User(i - 1),
        CurrentPage::User(i) if i == deleted => {
            if deleted < users {
                CurrentPage::User(deleted)
            } else if let Some(prev) = deleted.checked_sub(1) {
                CurrentPage::User(prev)
            } else {
                let factory = pages.len() - users;
                return factory.checked_sub(1).and_then(|last| at(pages, last));
            }
        }
        other => other,
    };
    refind(pages, next)
}

/// After the UI moved user page `from` to position `to`: the current page follows the page it
/// shows.
#[must_use]
pub fn after_user_move(current: CurrentPage, from: usize, to: usize) -> CurrentPage {
    let CurrentPage::User(i) = current else { return current };
    CurrentPage::User(if i == from {
        to
    } else if from < to && i > from && i <= to {
        i - 1
    } else if to < from && i >= to && i < from {
        i + 1
    } else {
        i
    })
}

/// The page indicator: `(n, m, name)` with `n` 1-based, and `(0, 0, "No pages")` when the device
/// has none (decision 1.3).
#[must_use]
pub fn indicator(pages: &[Page], current: Option<CurrentPage>) -> (usize, usize, String) {
    match current.and_then(|c| position(pages, c)).and_then(|i| pages.get(i).map(|p| (i, p))) {
        Some((i, p)) => (i + 1, pages.len(), p.name.clone()),
        None if pages.is_empty() => (0, 0, "No pages".to_owned()),
        None => (1, pages.len(), pages.first().map(|p| p.name.clone()).unwrap_or_default()),
    }
}
