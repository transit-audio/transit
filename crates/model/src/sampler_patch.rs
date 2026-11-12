//! The built-in Sampler's patch (ADR 0041 "Session format, media and SFZ"), stored on the
//! instrument's [`crate::Insert::sampler_patch`]. Schema version 1.
//!
//! - **Structure:** up to 256 zones with fixed logical ids (0..256, the `zone001`..`zone256` slot
//!   of their controls), each one region of a source reference with its key and velocity ranges,
//!   root and playback settings, optional layer/round-robin membership and an optional slice
//!   binding; 64 pads (banks A to D) binding a zone to a unique trigger note, with mute, solo,
//!   choke group (Off or 1..16) and name, and the pad start note the pads without a stored note
//!   follow ([`crate::pad_notes`]); the key-map groups' round-robin modes; the seed; and
//!   one loop's slicing state (its region, the Auto and Manual marker sets with ids and labels,
//!   the selected mode, snap, detector settings, grid division and bar override, root and beat
//!   length). Sources are references to session sources ([`crate::Source`]) with an ownership
//!   field: absent means managed (written into `Audio Files/` like any session audio), and
//!   `referenced` media keeps its own path, which an ordinary save never rewrites.
//! - **Controls:** per-zone values (level, pan, the shaper, the matrix) are the slot
//!   parameters' (`zone001_level`, ...), stored as a sparse map of slot id to value. Only the
//!   nondefault ones are present; the engine applies them through the plug-in's parameters.
//! - **Serialized form:** only populated zones and nondefault pads are written; every field has a
//!   default (seed 1, root 60, pad start note 60, mode Auto, snap on, division 1/16, no bar
//!   override), so an absent field loads as its default. A new patch ([`PatchV1::default`]) has
//!   pad start note 36, which is written. Unknown fields, at the top and in every entry, are kept and
//!   written back ([`Extra`]).
//! - **Opaque patches:** a schema version this build does not know keeps its raw object (and
//!   so its media references), plays silence and reports itself unavailable; a patch that does
//!   not parse or fails [`PatchV1::validate`] is kept the same way with a load note. Neither is
//!   ever rewritten into an empty patch.

use crate::SourceId;
pub use crate::sampler_fades::Fades;
pub use crate::sampler_validate::Invalid;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The schema version this build reads and writes.
pub const SCHEMA: u32 = 1;
/// Zone slots (fixed logical ids 0..256).
pub const ZONES: usize = 256;
/// Pads (four banks of sixteen).
pub const PADS: usize = 64;
/// Choke groups (0 is Off).
pub const CHOKE_GROUPS: u8 = 16;
/// Velocity layers per key-map group and alternatives per layer.
pub const LAYERS: u8 = 8;
pub const ALTERNATIVES: u8 = 8;
/// Distinct source references.
pub const MAX_SOURCES: usize = 256;
/// Slices per marker set (starts, the loop start included; the loop end is implicit).
pub const MAX_SLICES: usize = 64;
/// Longest label or name, in bytes.
pub const MAX_LABEL_BYTES: usize = 4096;
/// Most control entries (256 zones of at most 128 fields).
pub const MAX_CONTROLS: usize = ZONES * 128;
/// Longest control id.
pub(crate) const MAX_CONTROL_ID: usize = 64;

/// Fields this build does not know, kept as they were and written back.
pub type Extra = BTreeMap<String, Value>;

fn one() -> u32 {
    1
}
fn is_one(x: &u32) -> bool {
    *x == 1
}
fn yes() -> bool {
    true
}
fn is_true(x: &bool) -> bool {
    *x
}
fn schema() -> u32 {
    SCHEMA
}
fn sixty() -> u8 {
    60
}
fn is_sixty(x: &u8) -> bool {
    *x == 60
}
fn half() -> f32 {
    0.5
}
fn is_default<T: Default + PartialEq>(x: &T) -> bool {
    *x == T::default()
}

/// Who owns a source's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// The session's own audio: written into `Audio Files/` on save (the existing behavior).
    #[default]
    Managed,
    /// A file the user referenced where it is: its path is kept and never rewritten by a save.
    Referenced,
}

/// One source the zones and the loop name, by index.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceRef {
    /// The session source ([`crate::Session::sources`]).
    pub source: SourceId,
    #[serde(skip_serializing_if = "is_default")]
    pub ownership: Ownership,
    #[serde(flatten)]
    pub extra: Extra,
}

/// How a zone loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Off,
    Forward,
    PingPong,
}

/// How a zone plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    OneShot,
    #[default]
    Gate,
}

/// A zone's place in a key-map group (#270).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Membership {
    pub group: u8,
    pub layer: u8,
    pub alternative: u8,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Which marker set a slice binding names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceSet {
    #[default]
    Auto,
    Manual,
}

/// A zone's slice: the marker set and the slice id (its region comes from that slice).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SliceBinding {
    pub set: SliceSet,
    pub id: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One populated zone. Defaults are the Instrument view's (Gate, key tracking on, root 60, keys
/// 0..127, velocities 1..127, attack 1 ms, decay 300 ms, sustain 100 %, release 150 ms).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneEntry {
    /// The fixed logical zone id, 0..256 (its controls are `zone{id+1:03}_...`).
    pub id: u16,
    /// Index into [`PatchV1::sources`].
    pub source: u16,
    /// Native source frames, half-open.
    pub start: u64,
    pub end: u64,
    pub loop_mode: LoopMode,
    pub loop_start: u64,
    pub loop_end: u64,
    /// The forward-loop crossfade X in native source frames ([`crate::sampler_loops`]); written
    /// only when nonzero.
    #[serde(skip_serializing_if = "is_default")]
    pub crossfade: u64,
    /// Inclusive.
    pub keys: [u8; 2],
    /// Inclusive, 1..=127.
    pub velocities: [u8; 2],
    #[serde(default = "sixty")]
    pub root: u8,
    pub transpose: f32,
    pub cents: f32,
    pub reverse: bool,
    pub mode: PlayMode,
    #[serde(default = "yes")]
    pub key_tracking: bool,
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
    pub declick_ms: f32,
    pub velocity_sensitivity: f32,
    /// Edge-fade widths ([`crate::sampler_fades`]); written only when one is nonzero.
    #[serde(skip_serializing_if = "Fades::is_none")]
    pub fades: Fades,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub membership: Option<Membership>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slice: Option<SliceBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ZoneEntry {
    fn default() -> Self {
        ZoneEntry {
            id: 0,
            source: 0,
            start: 0,
            end: 0,
            loop_mode: LoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            crossfade: 0,
            keys: [0, 127],
            velocities: [1, 127],
            root: 60,
            transpose: 0.0,
            cents: 0.0,
            reverse: false,
            mode: PlayMode::Gate,
            key_tracking: true,
            attack_ms: 1.0,
            decay_ms: 300.0,
            sustain: 1.0,
            release_ms: 150.0,
            declick_ms: 0.0,
            velocity_sensitivity: 0.7,
            fades: Fades::default(),
            membership: None,
            slice: None,
            label: None,
            extra: Extra::new(),
        }
    }
}

impl ZoneEntry {
    /// A zone `id` over the whole of source `source` (`frames` long), with Instrument defaults.
    #[must_use]
    pub fn whole(id: u16, source: u16, frames: u64) -> ZoneEntry {
        ZoneEntry { id, source, end: frames, loop_end: frames, ..ZoneEntry::default() }
    }

    /// A new pad's playback defaults on this zone: one-shot, key tracking off, root at `note`.
    pub fn pad_defaults(&mut self, note: u8) {
        self.mode = PlayMode::OneShot;
        self.key_tracking = false;
        self.root = note.min(127);
    }
}

/// A pad that differs from its default (empty, trigger note `start + index` with the patch's pad
/// start note, nothing set).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PadEntry {
    /// 0..64: bank A is 0..16, numbered from bottom left across each row, then upward.
    pub index: u8,
    /// The bound zone's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<u16>,
    /// The stored trigger note, absolute (`None`: the default, the patch's pad start note plus the
    /// index, which moves with the start note).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
    #[serde(skip_serializing_if = "is_default")]
    pub mute: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub solo: bool,
    /// 0 Off, 1..=16.
    #[serde(skip_serializing_if = "is_default")]
    pub choke: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl PadEntry {
    /// Pad `index`'s default trigger note under pad start note `start`: `start + index`.
    #[must_use]
    pub fn default_note(index: u8, start: u8) -> u8 {
        start.saturating_add(index)
    }

    /// The trigger note under pad start note `start` (the patch's effective note for this pad is
    /// [`PatchV1::pad_note`]).
    #[must_use]
    pub fn trigger(&self, start: u8) -> u8 {
        self.note.unwrap_or_else(|| Self::default_note(self.index, start))
    }

    /// Whether the entry says nothing its default does not (it is then not serialized). A stored
    /// note is never plain, whatever its value: stored notes are absolute.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.zone.is_none() && self.note.is_none() && !self.mute && !self.solo && self.choke == 0 && self.name.is_none() && self.extra.is_empty()
    }
}

/// How a key-map group picks among its round-robin alternatives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundRobin {
    #[default]
    Sequential,
    /// Seeded random (the patch's seed, xorshift32).
    Random,
}

/// A key-map group's settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupEntry {
    pub id: u8,
    #[serde(skip_serializing_if = "is_default")]
    pub round_robin: RoundRobin,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The slicing mode shown in the Slices view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceMode {
    #[default]
    Auto,
    Manual,
}

/// Where Auto's markers come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoSource {
    /// The first nonempty usable result of cues, transients, then the grid.
    #[default]
    Auto,
    Cues,
    Transients,
    Grid,
}

/// Auto's detector settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detector {
    pub source: AutoSource,
    /// 0..1, default 0.5.
    #[serde(default = "half")]
    pub sensitivity: f32,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Detector {
    fn default() -> Self {
        Detector { source: AutoSource::Auto, sensitivity: 0.5, extra: Extra::new() }
    }
}

/// The beat grid's division.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Division {
    #[serde(rename = "1/4")]
    Quarter,
    #[serde(rename = "1/8")]
    Eighth,
    #[default]
    #[serde(rename = "1/16")]
    Sixteenth,
    #[serde(rename = "1/32")]
    ThirtySecond,
}

/// One marker: a slice start in native source frames, its stable id and optional label.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Marker {
    pub id: u32,
    pub frame: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One marker set (Auto or Manual): at most [`MAX_SLICES`] starts in increasing order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Markers {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// The lowest id this set never handed out (0: none recorded, the next id is one past the
    /// highest), so a removed id is never reused. Edits are in [`crate::slicing`].
    #[serde(skip_serializing_if = "is_default")]
    pub next_id: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The loop's slicing state (one loop per patch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SliceLoop {
    /// Index into [`PatchV1::sources`].
    pub source: u16,
    /// The loop region S..E in native source frames.
    pub start: u64,
    pub end: u64,
    pub mode: SliceMode,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub snap: bool,
    #[serde(skip_serializing_if = "is_default_detector")]
    pub detector: Detector,
    #[serde(skip_serializing_if = "is_default")]
    pub division: Division,
    /// An explicit bar count 1..=16 (`None`: From tempo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bars: Option<u8>,
    /// The beat grid's tempo override, 5 to 1000 BPM (`None`: the file's or the session's tempo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid_bpm: Option<f64>,
    /// The slice mapping root as the user stored it, absolute (`None`: it follows the patch's pad
    /// start note, [`PatchV1::slice_root`]); written whenever stored, 60 included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<u8>,
    /// Slice to MIDI's beat length in quarter notes (`None`: derived from the tempo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beat_length: Option<f64>,
    /// Auto's accepted markers (absent: S only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<Markers>,
    /// Manual's markers (absent: S only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<Markers>,
    /// A single marker set from a build without the two-set extension: loaded as Manual (with
    /// Manual selected and Auto at S only) unless explicit two-set state is present. Never written.
    #[serde(skip_serializing)]
    pub markers: Option<Vec<Marker>>,
    #[serde(flatten)]
    pub extra: Extra,
}

fn is_default_detector(d: &Detector) -> bool {
    *d == Detector::default()
}

impl Default for SliceLoop {
    fn default() -> Self {
        SliceLoop {
            source: 0,
            start: 0,
            end: 0,
            mode: SliceMode::Auto,
            snap: true,
            detector: Detector::default(),
            division: Division::Sixteenth,
            bars: None,
            grid_bpm: None,
            root: None,
            beat_length: None,
            auto: None,
            manual: None,
            markers: None,
            extra: Extra::new(),
        }
    }
}

/// A schema version 1 patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PatchV1 {
    #[serde(default = "schema")]
    pub schema: u32,
    /// The voices' random state and the random round robin's seed (0 normalizes to 1).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub seed: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRef>,
    /// Populated zones, by id.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<ZoneEntry>,
    /// The note of pad A1 when it has no stored note, 0..=64; every pad without a stored note
    /// plays `pad_start_note + index` ([`crate::pad_notes`]). A patch saved without it reads 60;
    /// a new patch has 36. Written only when not 60.
    #[serde(default = "sixty", skip_serializing_if = "is_sixty")]
    pub pad_start_note: u8,
    /// Pads that differ from their defaults, by index.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pads: Vec<PadEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupEntry>,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub slicing: Option<SliceLoop>,
    /// Nondefault slot parameter values by id (`zone001_level`, ...).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub controls: BTreeMap<String, f32>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for PatchV1 {
    fn default() -> Self {
        PatchV1 {
            schema: SCHEMA,
            seed: 1,
            sources: Vec::new(),
            zones: Vec::new(),
            pad_start_note: crate::pad_notes::NEW_PAD_START,
            pads: Vec::new(),
            groups: Vec::new(),
            slicing: None,
            controls: BTreeMap::new(),
            extra: Extra::new(),
        }
    }
}

/// Whether `id` has the form of a zone slot parameter id: `zoneNNN_field`, NNN 001..256.
#[must_use]
pub fn is_control_id(id: &str) -> bool {
    let b = id.as_bytes();
    let digits = b.get(4..7).filter(|d| d.iter().all(u8::is_ascii_digit));
    let n = digits.map(|d| d.iter().fold(0usize, |n, x| n * 10 + usize::from(x - b'0')));
    id.starts_with("zone")
        && b.get(7) == Some(&b'_')
        && b.len() > 8
        && b.len() <= MAX_CONTROL_ID
        && id.is_ascii()
        && n.is_some_and(|n| (1..=ZONES).contains(&n))
}

impl PatchV1 {
    /// The zone with logical id `id`.
    #[must_use]
    pub fn zone(&self, id: u16) -> Option<&ZoneEntry> {
        self.zones.iter().find(|z| z.id == id)
    }

    pub fn zone_mut(&mut self, id: u16) -> Option<&mut ZoneEntry> {
        self.zones.iter_mut().find(|z| z.id == id)
    }

    /// Pad `index`'s entry (its default when not stored).
    #[must_use]
    pub fn pad(&self, index: u8) -> PadEntry {
        self.pads.iter().find(|p| p.index == index).cloned().unwrap_or(PadEntry { index, ..PadEntry::default() })
    }

    /// Stores pad `index`'s entry; a plain entry is dropped, so only nondefault pads are kept.
    pub fn set_pad(&mut self, pad: PadEntry) {
        self.pads.retain(|p| p.index != pad.index);
        if !pad.is_plain() {
            self.pads.push(pad);
            self.pads.sort_by_key(|p| p.index);
        }
    }

    /// The pad bound to zone `id`.
    #[must_use]
    pub fn pad_of(&self, id: u16) -> Option<u8> {
        self.pads.iter().find(|p| p.zone == Some(id)).map(|p| p.index)
    }

    /// The index of session source `source` in [`PatchV1::sources`], adding it (as `ownership`)
    /// when it is not there yet.
    pub fn source_index(&mut self, source: SourceId, ownership: Ownership) -> usize {
        match self.sources.iter().position(|s| s.source == source) {
            Some(i) => i,
            None => {
                self.sources.push(SourceRef { source, ownership, extra: Extra::new() });
                self.sources.len() - 1
            }
        }
    }

    /// Drops source references no zone and no loop use, renumbering the indices that remain.
    pub fn drop_unused_sources(&mut self) {
        let used = |i: usize, p: &PatchV1| {
            p.zones.iter().any(|z| usize::from(z.source) == i) || p.slicing.as_ref().is_some_and(|l| usize::from(l.source) == i)
        };
        let keep: Vec<bool> = (0..self.sources.len()).map(|i| used(i, self)).collect();
        let mut map = vec![0u16; self.sources.len()];
        let mut next = 0u16;
        for (i, k) in keep.iter().enumerate() {
            if let Some(m) = map.get_mut(i) {
                *m = next;
            }
            if *k {
                next = next.saturating_add(1);
            }
        }
        let mut i = 0;
        self.sources.retain(|_| {
            let k = keep.get(i).copied().unwrap_or(true);
            i += 1;
            k
        });
        for z in &mut self.zones {
            z.source = map.get(usize::from(z.source)).copied().unwrap_or(z.source);
        }
        if let Some(l) = &mut self.slicing {
            l.source = map.get(usize::from(l.source)).copied().unwrap_or(l.source);
        }
    }

    /// Sets slot control `id` to `value`; a value equal to its default (`default`) removes the
    /// entry, so only nondefault controls are kept.
    pub fn set_control(&mut self, id: &str, value: f32, default: f32) {
        if value == default {
            self.controls.remove(id);
        } else {
            self.controls.insert(id.to_string(), value);
        }
    }

    /// Removes zone slot `zone`'s controls (`zoneNNN_...`, `zone` 0-based), so they read their
    /// defaults again: every edit that empties a zone calls it in the same step, so the next
    /// sample in that slot starts from defaults (#478) and undo restores the controls with the
    /// zone.
    pub fn reset_controls(&mut self, zone: u16) {
        let prefix = format!("zone{:03}_", u32::from(zone) + 1);
        self.controls.retain(|id, _| !id.starts_with(&prefix));
    }

    /// Moves a single legacy marker set into Manual (ADR 0041: Manual selected, Auto at S only,
    /// no redetection); explicitly saved two-set state wins.
    fn migrate(&mut self) {
        if let Some(l) = &mut self.slicing
            && let Some(old) = l.markers.take()
            && l.auto.is_none()
            && l.manual.is_none()
        {
            l.manual = Some(Markers { markers: old, ..Markers::default() });
            l.mode = SliceMode::Manual;
        }
    }
}

/// What an [`Insert::sampler_patch`](crate::Insert::sampler_patch) holds.
#[derive(Debug, Clone, PartialEq)]
pub enum PatchState {
    /// A valid schema version 1 patch.
    Ready(Box<PatchV1>),
    /// A schema version this build does not know: the raw object, kept and written back.
    Unknown { schema: u64, raw: Value },
    /// A patch that did not parse or validate: the raw object, kept and written back.
    Invalid { reason: String, raw: Value },
}

/// The Sampler's patch, with its codec (see the module doc).
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerPatch {
    pub state: PatchState,
}

impl Default for SamplerPatch {
    fn default() -> Self {
        SamplerPatch::new()
    }
}

impl SamplerPatch {
    /// An empty patch: no zones, default pads from pad start note 36, seed 1.
    #[must_use]
    pub fn new() -> SamplerPatch {
        SamplerPatch { state: PatchState::Ready(Box::default()) }
    }

    /// Reads a patch from its JSON object: a valid version 1 patch is ready; anything else is
    /// kept opaque.
    #[must_use]
    pub fn from_value(raw: Value) -> SamplerPatch {
        let state = match raw.get("schema").map(Value::as_u64) {
            _ if !raw.is_object() => PatchState::Invalid { reason: "not an object".into(), raw },
            Some(None) => PatchState::Invalid { reason: "the schema is not a number".into(), raw },
            Some(Some(v)) if v != u64::from(SCHEMA) => PatchState::Unknown { schema: v, raw },
            _ => match serde_json::from_value::<PatchV1>(raw.clone()) {
                Ok(mut p) => {
                    p.migrate();
                    match p.validate() {
                        Ok(()) => PatchState::Ready(Box::new(p)),
                        Err(Invalid(reason)) => PatchState::Invalid { reason, raw },
                    }
                }
                Err(e) => PatchState::Invalid { reason: e.to_string(), raw },
            },
        };
        SamplerPatch { state }
    }

    /// The playable patch (`None`: opaque, silent).
    #[must_use]
    pub fn ready(&self) -> Option<&PatchV1> {
        match &self.state {
            PatchState::Ready(p) => Some(p),
            _ => None,
        }
    }

    pub fn ready_mut(&mut self) -> Option<&mut PatchV1> {
        match &mut self.state {
            PatchState::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// The seed the instance's voices start from (1 for an opaque patch; 0 normalizes to 1).
    #[must_use]
    pub fn seed(&self) -> u32 {
        self.ready().map_or(1, |p| p.seed.max(1))
    }

    /// The load note for an opaque patch on track `track` (`None` for a ready one).
    #[must_use]
    pub fn note(&self, track: &str) -> Option<String> {
        match &self.state {
            PatchState::Ready(_) => None,
            PatchState::Unknown { schema, .. } => Some(format!(
                "track \"{track}\": its Sampler patch uses schema {schema}, which this version does not know: it plays silence and is kept as saved"
            )),
            PatchState::Invalid { reason, .. } => {
                let reason: String = reason.chars().take(200).collect();
                Some(format!("track \"{track}\": its Sampler patch is not valid ({reason}): it plays silence and is kept as saved"))
            }
        }
    }

    /// The session sources the patch names, also for an opaque patch (its raw `sources`).
    #[must_use]
    pub fn media(&self) -> Vec<SourceId> {
        match &self.state {
            PatchState::Ready(p) => p.sources.iter().map(|s| s.source).collect(),
            PatchState::Unknown { raw, .. } | PatchState::Invalid { raw, .. } => raw
                .get("sources")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|s| s.get("source").and_then(Value::as_u64)).map(SourceId).take(MAX_SOURCES).collect())
                .unwrap_or_default(),
        }
    }

    /// The session sources the patch references where they are ([`Ownership::Referenced`]), also
    /// for an opaque patch (its raw `sources` entries marked `"ownership": "referenced"`), so a save
    /// keeps their paths as it would a readable patch's (#489).
    #[must_use]
    pub fn referenced(&self) -> Vec<SourceId> {
        match &self.state {
            PatchState::Ready(p) => p.sources.iter().filter(|s| s.ownership == Ownership::Referenced).map(|s| s.source).collect(),
            PatchState::Unknown { raw, .. } | PatchState::Invalid { raw, .. } => raw
                .get("sources")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter(|s| s.get("ownership").and_then(Value::as_str) == Some("referenced"))
                        .filter_map(|s| s.get("source").and_then(Value::as_u64))
                        .map(SourceId)
                        .take(MAX_SOURCES)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Renames every session source the patch names through `f` (a track moved to another session),
    /// also in an opaque patch's raw object.
    pub fn remap_media(&mut self, mut f: impl FnMut(SourceId) -> SourceId) {
        match &mut self.state {
            PatchState::Ready(p) => {
                for r in &mut p.sources {
                    r.source = f(r.source);
                }
            }
            PatchState::Unknown { raw, .. } | PatchState::Invalid { raw, .. } => {
                if let Some(a) = raw.get_mut("sources").and_then(Value::as_array_mut) {
                    for s in a.iter_mut().take(MAX_SOURCES) {
                        if let Some(n) = s.get("source").and_then(Value::as_u64) {
                            s["source"] = Value::from(f(SourceId(n)).0);
                        }
                    }
                }
            }
        }
    }

    /// Whether the patch carries loop slicing state (the `sampler_slicing_modes` need).
    #[must_use]
    pub fn uses_slicing(&self) -> bool {
        match &self.state {
            PatchState::Ready(p) => p.slicing.is_some(),
            PatchState::Unknown { raw, .. } | PatchState::Invalid { raw, .. } => raw.get("loop").is_some_and(|l| !l.is_null()),
        }
    }
}

impl Serialize for SamplerPatch {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.state {
            PatchState::Ready(p) => p.serialize(s),
            PatchState::Unknown { raw, .. } | PatchState::Invalid { raw, .. } => raw.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for SamplerPatch {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Value::deserialize(d).map(SamplerPatch::from_value)
    }
}

#[cfg(test)]
#[path = "sampler_patch_tests.rs"]
mod tests;
