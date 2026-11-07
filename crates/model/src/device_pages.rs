//! User pages of the device macro strip (ADR 0047 decision 4): a name and eight parameter-id slots
//! per page, stored on the device's slot record ([`Insert::pages`](crate::Insert::pages),
//! [`MidiEffectSlot::pages`](crate::MidiEffectSlot::pages)) so they move, copy and undo with the
//! device. Data only: which ids exist is the engine's question (a slot naming an id the device's
//! table lacks is "missing", never removed here).
//!
//! The file is untrusted, and reading never fails or panics on a page: a missing or non-string
//! `name` reads empty, a name is trimmed and cut to [`MAX_PAGE_NAME_CHARS`], fewer than
//! [`PAGE_SLOTS`] slots pad with empty, extra ones are dropped, a non-string, empty or over-long
//! slot reads empty, a duplicate id keeps its first slot, and pages past [`MAX_USER_PAGES`] are
//! dropped with one load note per device ([`dropped_page_notes`]).

use crate::{INSERT_SLOTS, MAX_MIDI_EFFECTS, Track};
use serde::Deserialize as _;
use serde_json::Value;

/// Slots on a page (one row of knobs, one controller bank; `transit_dsp::PAGE_SLOTS`).
pub const PAGE_SLOTS: usize = 8;

/// User pages a device holds at most.
pub const MAX_USER_PAGES: usize = 16;

/// A page name's length after trimming, in characters.
pub const MAX_PAGE_NAME_CHARS: usize = 32;

/// The longest parameter id a slot keeps, in characters; a longer one reads as empty.
pub const MAX_SLOT_ID_CHARS: usize = 64;

/// One user page. Written with all eight slots, `null` for an empty one.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct UserPage {
    /// Stored at creation ("User n", "<factory name> copy"); empty reads as "User <position>"
    /// ([`UserPage::display_name`]), and that reading is never written back.
    pub name: String,
    pub slots: [Option<String>; PAGE_SLOTS],
}

impl UserPage {
    /// An empty page named `name` (read by the rules: trimmed and cut).
    #[must_use]
    pub fn named(name: &str) -> Self {
        UserPage { name: clean_name(name), slots: Default::default() }
    }

    /// The page as the read rules see `v`: anything that is not an object is a page with no
    /// name and no slots.
    #[must_use]
    pub fn read(v: &Value) -> Self {
        let name = v.get("name").and_then(Value::as_str).map(clean_name).unwrap_or_default();
        let mut slots: [Option<String>; PAGE_SLOTS] = Default::default();
        if let Some(raw) = v.get("slots").and_then(Value::as_array) {
            for (slot, entry) in slots.iter_mut().zip(raw) {
                *slot = entry.as_str().filter(|id| !id.is_empty() && id.chars().count() <= MAX_SLOT_ID_CHARS).map(str::to_owned);
            }
        }
        dedupe(&mut slots);
        UserPage { name, slots }
    }

    /// The name shown for the page at 1-based `position` among the user pages.
    #[must_use]
    pub fn display_name(&self, position: usize) -> String {
        if self.name.is_empty() { format!("User {position}") } else { self.name.clone() }
    }

    /// Whether `id` sits in a slot of this page.
    #[must_use]
    pub fn holds(&self, id: &str) -> bool {
        self.slots.iter().flatten().any(|s| s == id)
    }

    /// The page passed through the read rules again (a page built in code, or by a command).
    #[must_use]
    pub fn normalized(&self) -> Self {
        let mut slots = self.slots.clone();
        for slot in &mut slots {
            if slot.as_deref().is_some_and(|id| id.is_empty() || id.chars().count() > MAX_SLOT_ID_CHARS) {
                *slot = None;
            }
        }
        dedupe(&mut slots);
        UserPage { name: clean_name(&self.name), slots }
    }
}

impl<'de> serde::Deserialize<'de> for UserPage {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Value::deserialize(d).map(|v| UserPage::read(&v))
    }
}

fn clean_name(name: &str) -> String {
    let mut name = name.trim().to_owned();
    crate::truncate_chars(&mut name, MAX_PAGE_NAME_CHARS);
    // Cutting can leave trailing space inside the first 32 characters.
    name.truncate(name.trim_end().len());
    name
}

/// A duplicate id within one page keeps its first slot; later ones read empty.
fn dedupe(slots: &mut [Option<String>; PAGE_SLOTS]) {
    for i in 1..PAGE_SLOTS {
        let (before, rest) = slots.split_at_mut(i);
        if let Some(slot) = rest.first_mut()
            && slot.as_ref().is_some_and(|id| before.iter().flatten().any(|b| b == id))
        {
            *slot = None;
        }
    }
}

/// A device's user pages as the read rules see `v`, and how many pages past [`MAX_USER_PAGES`]
/// were dropped. Anything that is not an array has no pages.
#[must_use]
pub fn read_pages(v: &Value) -> (Vec<UserPage>, usize) {
    let Some(raw) = v.as_array() else { return (Vec::new(), 0) };
    let pages = raw.iter().take(MAX_USER_PAGES).map(UserPage::read).collect();
    (pages, raw.len().saturating_sub(MAX_USER_PAGES))
}

/// `#[serde(deserialize_with)]` for a `pages` field: the read rules, never an error.
pub(crate) fn deserialize_pages<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<UserPage>, D::Error> {
    Value::deserialize(d).map(|v| read_pages(&v).0)
}

/// One load note per device whose saved pages went past [`MAX_USER_PAGES`], naming the track and
/// the device, read from the session file `v` before it is deserialized (the `Insert`
/// deserializer cannot reach the session's load notes).
#[must_use]
pub fn dropped_page_notes(v: &Value) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(tracks) = v.get("tracks").and_then(Value::as_array) else { return notes };
    for t in tracks {
        let mut name = t.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        crate::truncate_chars(&mut name, crate::MAX_NAME_CHARS);
        let mut note = |device: String, pages: Option<&Value>| {
            let dropped = pages.map_or(0, |p| read_pages(p).1);
            if dropped > 0 {
                notes.push(format!("track `{name}`, {device}: {dropped} macro strip page(s) past the {MAX_USER_PAGES} a device keeps were dropped"));
            }
        };
        note("instrument".into(), t.get("instrument").and_then(|i| i.get("pages")));
        if let Some(inserts) = t.get("mixer").and_then(|m| m.get("inserts")).and_then(Value::as_array) {
            for (i, ins) in inserts.iter().take(INSERT_SLOTS).enumerate() {
                note(format!("insert {}", DeviceRef::Insert(i)), ins.get("pages"));
            }
        }
        if let Some(fx) = t.get("midi_effects").and_then(Value::as_array) {
            for (i, slot) in fx.iter().take(MAX_MIDI_EFFECTS).enumerate() {
                note(format!("MIDI effect {}", i + 1), slot.get("pages"));
            }
        }
    }
    notes
}

/// A device of a track as the strip commands address it (ADR 0047 decision 3.8, extending ADR
/// 0028 decision 10): an insert letter `a` to `j`, `instrument`, `midi1` to `midi8` (the MIDI
/// effect at that position plus one, resolved when the command runs) or `rack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceRef {
    Insert(usize),
    Instrument,
    MidiEffect(usize),
    Rack,
}

impl DeviceRef {
    /// Parses the addressing above (case and surrounding space ignored).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let s = text.trim().to_ascii_lowercase();
        match s.as_str() {
            "instrument" => return Some(DeviceRef::Instrument),
            "rack" => return Some(DeviceRef::Rack),
            _ => {}
        }
        if let Some(n) = s.strip_prefix("midi") {
            // `midi01` is not `midi1`: the digits must be the number's own text.
            let n = n.parse::<usize>().ok().filter(|v| v.to_string() == n)?;
            return n.checked_sub(1).filter(|i| *i < MAX_MIDI_EFFECTS).map(DeviceRef::MidiEffect);
        }
        match s.as_bytes() {
            [c @ b'a'..=b'z'] if usize::from(c - b'a') < INSERT_SLOTS => Some(DeviceRef::Insert(usize::from(c - b'a'))),
            _ => None,
        }
    }
}

impl std::fmt::Display for DeviceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceRef::Insert(i) => write!(f, "{}", char::from(b'a' + u8::try_from(*i).unwrap_or(0))),
            DeviceRef::Instrument => f.write_str("instrument"),
            DeviceRef::MidiEffect(i) => write!(f, "midi{}", i + 1),
            DeviceRef::Rack => f.write_str("rack"),
        }
    }
}

/// Why a track's device pages could not be read or replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PagesError {
    /// Nothing sits at that address on the track.
    NoDevice,
    /// A rack card's strip is its eight macros, one fixed page (ADR 0047 decision 6.4).
    Rack,
    /// The device has no parameters to put on a page (a hardware insert, an External Instrument).
    NoParameters,
    /// More than [`MAX_USER_PAGES`] pages.
    TooMany(usize),
}

impl std::fmt::Display for PagesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PagesError::NoDevice => f.write_str("no device at that address"),
            PagesError::Rack => f.write_str("a rack card's strip is its eight macros and takes no user pages"),
            PagesError::NoParameters => f.write_str("the device has no parameters to put on a page"),
            PagesError::TooMany(n) => write!(f, "{n} pages: a device keeps at most {MAX_USER_PAGES}"),
        }
    }
}

/// A slot record seen without its pages, for freeze material (`Track::freeze_inputs`): equal
/// when every other field is equal, serialized as the record with no pages (the field is then
/// left out, so the bytes are those of a record that never had any).
pub(crate) struct NoPages<'a, T>(pub(crate) &'a T);

/// A slot record that carries user pages.
pub(crate) trait HasPages: Clone + serde::Serialize {
    /// Whether every field but `pages` (and the uid, which never takes part) is equal.
    fn same_without_pages(&self, other: &Self) -> bool;
    fn pages_mut(&mut self) -> &mut Vec<UserPage>;
}

impl HasPages for crate::Insert {
    fn same_without_pages(&self, other: &Self) -> bool {
        // Spelled out field by field, so a new field fails to compile here until it is placed.
        let crate::Insert {
            uid: _,
            plugin,
            params,
            bypass,
            active,
            preset,
            state,
            frozen_recording,
            sample,
            sampler_patch,
            key,
            hardware,
            external,
            pages: _,
        } = self;
        *plugin == other.plugin
            && *params == other.params
            && *bypass == other.bypass
            && *active == other.active
            && *preset == other.preset
            && *state == other.state
            && *frozen_recording == other.frozen_recording
            && *sample == other.sample
            && *sampler_patch == other.sampler_patch
            && *key == other.key
            && *hardware == other.hardware
            && *external == other.external
    }
    fn pages_mut(&mut self) -> &mut Vec<UserPage> {
        &mut self.pages
    }
}

impl HasPages for crate::MidiEffectSlot {
    fn same_without_pages(&self, other: &Self) -> bool {
        let crate::MidiEffectSlot { uid: _, plugin, params, bypass, state, pages: _ } = self;
        *plugin == other.plugin && *params == other.params && *bypass == other.bypass && *state == other.state
    }
    fn pages_mut(&mut self) -> &mut Vec<UserPage> {
        &mut self.pages
    }
}

impl<T: HasPages> PartialEq for NoPages<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_without_pages(other.0)
    }
}

impl<T: HasPages> serde::Serialize for NoPages<'_, T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Only a freeze hash serializes this (at freeze time, not per edit), so the copy is cheap
        // enough.
        let mut copy = self.0.clone();
        copy.pages_mut().clear();
        copy.serialize(s)
    }
}

impl Track {
    /// The user pages of `device` (`None` when nothing sits there; a rack card has none).
    #[must_use]
    pub fn device_pages(&self, device: DeviceRef) -> Option<&[UserPage]> {
        match device {
            DeviceRef::Insert(i) => self.mixer.inserts.get(i).and_then(Option::as_ref).map(|x| x.pages.as_slice()),
            DeviceRef::Instrument => self.instrument.as_ref().map(|x| x.pages.as_slice()),
            DeviceRef::MidiEffect(i) => self.midi_effects.get(i).map(|x| x.pages.as_slice()),
            DeviceRef::Rack => None,
        }
    }

    /// Replaces the user pages of `device` with `pages` passed through the read rules, editing
    /// only the `pages` field of the slot record: the device keeps its [`InsertUid`](crate::InsertUid)
    /// and every other field, so the plan compiler keeps its instance. Returns whether anything
    /// changed. Refuses a rack card, a device without parameters, an empty address and more than
    /// [`MAX_USER_PAGES`] pages, changing nothing.
    pub fn set_device_pages(&mut self, device: DeviceRef, pages: &[UserPage]) -> Result<bool, PagesError> {
        if pages.len() > MAX_USER_PAGES {
            return Err(PagesError::TooMany(pages.len()));
        }
        let pages: Vec<UserPage> = pages.iter().map(UserPage::normalized).collect();
        let slot = match device {
            DeviceRef::Rack => return Err(if self.is_rack() { PagesError::Rack } else { PagesError::NoDevice }),
            DeviceRef::Insert(i) => {
                let ins = self.mixer.inserts.get_mut(i).and_then(Option::as_mut).ok_or(PagesError::NoDevice)?;
                if ins.hardware.is_some() || ins.plugin == crate::HARDWARE_PLUGIN {
                    return Err(PagesError::NoParameters);
                }
                &mut ins.pages
            }
            DeviceRef::Instrument => {
                let ins = self.instrument.as_mut().ok_or(PagesError::NoDevice)?;
                if ins.external_instrument().is_some() || ins.plugin == crate::EXTERNAL_INSTRUMENT_PLUGIN {
                    return Err(PagesError::NoParameters);
                }
                &mut ins.pages
            }
            DeviceRef::MidiEffect(i) => &mut self.midi_effects.get_mut(i).ok_or(PagesError::NoDevice)?.pages,
        };
        if *slot == pages {
            return Ok(false);
        }
        *slot = pages;
        Ok(true)
    }

    /// Whether any device of the track (its instrument, inserts or MIDI effects) has user pages.
    #[must_use]
    pub fn has_device_pages(&self) -> bool {
        self.instrument.as_ref().is_some_and(|i| !i.pages.is_empty())
            || self.mixer.inserts.iter().flatten().any(|i| !i.pages.is_empty())
            || self.midi_effects.iter().any(|f| !f.pages.is_empty())
    }
}
