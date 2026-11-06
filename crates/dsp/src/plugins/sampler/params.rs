//! The core path's slot parameters (ADR 0041 "Capacities and admission"): fixed ids per zone slot,
//! never ids based on the selection or a name. Zone slot `n` (1..=256, zone index `n - 1`) has
//! `zone{n:03}_level` (dB) and `zone{n:03}_pan` (-100..100 %), then the Macro Oscillator's shaper
//! block and matrix rows with the same ids behind the prefix (`zone001_shaper`,
//! `zone001_amp_attack`, .., `zone001_lfo2_amount`, `zone001_mod1_source` .. `zone001_mod16_curve`),
//! with the same ranges and defaults. The existing unprefixed ids keep their legacy meanings (the 19
//! parameters of `SAMPLER_INFO`, which address the legacy renderer only).
//!
//! The ids are not in `SAMPLER_INFO`'s table: 256 zones of 113 fields do not fit the mix's inline
//! parameter cache. Their values live here, in one table allocated when the core path is installed
//! (outside the callback); `set_param` on the audio thread parses the id without allocating, clamps
//! the value, stores it and marks the zone, and the next block start rebuilds that zone's level,
//! pan, shaper settings (at the session rate) and matrix and hands them to the core. A slot value
//! applies to whatever zone the patch puts in that slot; an empty slot keeps the value for later.

use transit_instrument_core::matrix::{Matrix, ROWS};
use transit_sampler_core::ZONES;
use transit_sampler_core::patch::Zone;

use crate::params::lin;
use crate::plugins::macro_osc::{AMP_RELEASE_PARAM, PARAM_COUNT, PARAM_TABLE, SHAPER_FIRST, matrix_row, shaper_settings};
use crate::{ParamInfo, Unit};

static LEVEL: ParamInfo = lin("level", "Level", -48.0, 12.0, 0.0, Unit::Db);
static PAN: ParamInfo = lin("pan", "Pan", -100.0, 100.0, 0.0, Unit::Percent);

/// Fields per zone slot: level, pan, then the shaper block and the matrix rows.
pub const ZONE_FIELDS: usize = 2 + PARAM_COUNT - SHAPER_FIRST;
/// The level field.
pub const FIELD_LEVEL: usize = 0;
/// The pan field.
pub const FIELD_PAN: usize = 1;
/// The shaper's amp release field.
const FIELD_AMP_RELEASE: usize = 2 + AMP_RELEASE_PARAM - SHAPER_FIRST;

/// The id prefix's length: `zone` and three digits and `_`.
const PREFIX: usize = 8;

/// Field `field`'s description (its id is the unprefixed one).
#[must_use]
pub fn field_info(field: usize) -> Option<&'static ParamInfo> {
    match field {
        FIELD_LEVEL => Some(&LEVEL),
        FIELD_PAN => Some(&PAN),
        f => PARAM_TABLE.get(SHAPER_FIRST + f - 2).filter(|_| f < ZONE_FIELDS),
    }
}

/// The id of zone `zone`'s (0-based) field `field`, e.g. `zone001_level`. Control side (allocates).
#[must_use]
pub fn slot_id(zone: usize, field: usize) -> Option<String> {
    (zone < ZONES).then(|| field_info(field).map(|p| format!("zone{:03}_{}", zone + 1, p.id))).flatten()
}

/// The zone index and the id's field part, for an id of the `zoneNNN_` form (no allocation).
fn split(id: &str) -> Option<(usize, &str)> {
    let b = id.as_bytes();
    if b.len() <= PREFIX || !id.starts_with("zone") || b.get(7) != Some(&b'_') {
        return None;
    }
    let digits = b.get(4..7)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = digits.iter().fold(0usize, |n, d| n * 10 + usize::from(d - b'0'));
    ((1..=ZONES).contains(&n)).then(|| id.get(PREFIX..).map(|rest| (n - 1, rest))).flatten()
}

/// The zone index and field index slot id `id` names (control side: no table needed; a linear
/// search of the field ids).
#[must_use]
pub fn resolve(id: &str) -> Option<(usize, usize)> {
    let (zone, rest) = split(id)?;
    let field = (0..ZONE_FIELDS).find(|&f| field_info(f).is_some_and(|p| p.id == rest))?;
    Some((zone, field))
}

/// Every zone slot's values, by zone then field, with the zones changed since the last block.
pub(super) struct SlotParams {
    values: Vec<f32>,
    /// Field indices sorted by id (binary search on the audio thread, built once off it).
    by_id: Vec<usize>,
    /// Bit `z % 64` of word `z / 64`: zone `z` changed.
    dirty: [u64; ZONES / 64],
}

impl SlotParams {
    /// Every slot at its defaults (allocates: off the audio thread).
    pub(super) fn new() -> SlotParams {
        let defaults: Vec<f32> = (0..ZONE_FIELDS).map(|f| field_info(f).map_or(0.0, |p| p.default)).collect();
        let mut by_id: Vec<usize> = (0..ZONE_FIELDS).collect();
        by_id.sort_by_key(|&f| field_info(f).map(|p| p.id));
        SlotParams { values: (0..ZONES).flat_map(|_| defaults.iter().copied()).collect(), by_id, dirty: [0; ZONES / 64] }
    }

    fn field_of(&self, rest: &str) -> Option<usize> {
        let id = |f: usize| field_info(f).map_or("", |p| p.id);
        let at = self.by_id.partition_point(|&f| id(f) < rest);
        self.by_id.get(at).copied().filter(|&f| id(f) == rest)
    }

    /// The zone and field an id names.
    pub(super) fn resolve(&self, id: &str) -> Option<(usize, usize)> {
        let (zone, rest) = split(id)?;
        Some((zone, self.field_of(rest)?))
    }

    /// Clamps and stores (NaN becomes the default) and marks the zone; `false` for an unknown id.
    pub(super) fn set(&mut self, id: &str, value: f32) -> bool {
        let Some((zone, field)) = self.resolve(id) else { return false };
        self.set_index(zone, field, value)
    }

    /// Clamps and stores zone `zone`'s field `field` (NaN becomes the default) and marks the
    /// zone, as [`SlotParams::set`] does for an id; out-of-range indices change nothing.
    pub(super) fn set_index(&mut self, zone: usize, field: usize, value: f32) -> bool {
        let (Some(info), true) = (field_info(field), zone < ZONES) else { return false };
        let Some(slot) = self.values.get_mut(zone * ZONE_FIELDS + field) else { return false };
        *slot = info.clamp(value);
        if let Some(w) = self.dirty.get_mut(zone / 64) {
            *w |= 1 << (zone % 64);
        }
        true
    }

    /// The value an id names.
    pub(super) fn get(&self, id: &str) -> Option<f32> {
        let (zone, field) = self.resolve(id)?;
        self.values.get(zone * ZONE_FIELDS + field).copied()
    }

    /// Zone `zone`'s field `field`.
    pub(super) fn value(&self, zone: usize, field: usize) -> f32 {
        self.values.get(zone * ZONE_FIELDS + field).copied().unwrap_or(0.0)
    }

    /// Takes the set of zones changed since the last call: bit `z % 64` of word `z / 64`.
    pub(super) fn take_dirty(&mut self) -> [u64; ZONES / 64] {
        std::mem::take(&mut self.dirty)
    }

    /// Zone `zone`'s shaper amp release (ms).
    pub(super) fn amp_release_ms(&self, zone: usize) -> f32 {
        self.value(zone, FIELD_AMP_RELEASE)
    }

    /// Writes zone `zone`'s level, pan, shaper settings (at `sr`, for the bar of `bar_quarters`
    /// from `bar_origin`) and matrix into `z`. No allocation.
    pub(super) fn compose(&self, zone: usize, z: &mut Zone, bar_quarters: f64, bar_origin: (i64, f64), sr: f32) {
        z.playback.level_db = self.value(zone, FIELD_LEVEL);
        z.playback.pan = self.value(zone, FIELD_PAN) / 100.0;
        let v = |i: usize| i.checked_sub(SHAPER_FIRST).map_or(0.0, |k| self.value(zone, k + 2));
        z.shaper = shaper_settings(&v, bar_quarters, bar_origin, sr);
        z.matrix = Matrix::new((0..ROWS).map(|r| matrix_row(&v, r)));
    }
}
