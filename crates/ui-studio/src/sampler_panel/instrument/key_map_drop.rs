//! Another sample onto free keys while zones exist (#492; the rule is the maintainer's to confirm):
//! a file dropped over a key no zone covers makes a new zone in the first empty slot, rooted at
//! that key and spanning the whole run of free keys around it (from one above the nearest zone
//! below, or key 0, to one below the nearest zone above, or key 127), at full velocity, in one
//! `mix.sampler_zone_source` that sets its keys, velocities and root together (one undo step). A
//! drop over keys a zone covers is refused with [`covered`]'s words and changes nothing; a pad's
//! zone covers only its pad's note. Load… with zones places the file on the lowest free run, rooted
//! at the run's first C, else its lowest key; with no free key it is unavailable ([`FULL`]). The
//! engine still decides (the eight-voice rule, the budgets) and its refusal is the note row.

use super::key_map::{HIGHEST, KeyMapLayout, Keys, LOWEST};
use super::state::State;
use serde_json::{Value, json};

/// Load…'s reason when every key plays a zone.
pub const FULL: &str = "Every key already plays a zone: there is no free key for another sample.";

/// Which keys play a zone: a pad's zone its pad's note, any other zone its key range.
pub fn covered_keys(state: &State) -> [bool; 128] {
    let mut out = [false; 128];
    for z in &state.zones {
        let (lo, hi) = match z.pad {
            Some(p) => match state.pad_note(p) {
                Some(n) => (n, n),
                None => continue,
            },
            None => (z.keys[0], z.keys[1]),
        };
        for k in lo.min(127)..=hi.min(127) {
            out[usize::from(k)] = true;
        }
    }
    out
}

/// The run of free keys around `key` (none: a zone covers `key`).
pub fn free_run(state: &State, key: u8) -> Option<[u8; 2]> {
    let c = covered_keys(state);
    if c[usize::from(key)] {
        return None;
    }
    let lo = (LOWEST..key).rev().find(|k| c[usize::from(*k)]).map_or(LOWEST, |k| k + 1);
    let hi = (key + 1..=HIGHEST).find(|k| c[usize::from(*k)]).map_or(HIGHEST, |k| k - 1);
    Some([lo, hi])
}

/// Load…'s place with zones: the lowest free run and its root (the run's first C, else its lowest
/// key); none when every key plays a zone.
pub fn lowest_free(state: &State) -> Option<([u8; 2], u8)> {
    let c = covered_keys(state);
    let lo = (LOWEST..=HIGHEST).find(|k| !c[usize::from(*k)])?;
    let run = free_run(state, lo)?;
    let root = (run[0]..=run[1]).find(|k| k % 12 == 0).unwrap_or(run[0]);
    Some((run, root))
}

/// Why a drop over `key` is refused: the zone that plays it.
pub fn covered(state: &State, key: u8, names: transit_midi::MiddleC) -> String {
    let zone = state
        .zones
        .iter()
        .find(|z| match z.pad {
            Some(p) => state.pad_note(p) == Some(key),
            None => (z.keys[0]..=z.keys[1]).contains(&key),
        })
        .map(|z| z.zone);
    let name = transit_midi::note_name(key, names);
    match zone {
        Some(z) => format!("{name} already plays zone {z}: drop on a free key (layers on the key map come in a later release)."),
        None => format!("{name} already plays a zone: drop on a free key (layers on the key map come in a later release)."),
    }
}

/// Where a drop at `at` lands on the map as last drawn: the key under it, when `at` is over the
/// well or the keyboard.
pub fn key_under(k: &KeyMapLayout, at: egui::Pos2) -> Option<u8> {
    let over = at.x >= k.well[0] && at.x <= k.well[2] && at.y >= k.well[1] && at.y <= k.keyboard[3];
    (over && k.frame.1 > 0.0).then(|| Keys::new(LOWEST, HIGHEST, k.frame.0, k.frame.1).at(at.x))
}

/// The new zone's settings in the source command: its keys, full velocity and root.
pub fn playback(run: [u8; 2], root: u8) -> Value {
    json!({"keys": run, "velocities": [1, 127], "root": root})
}
