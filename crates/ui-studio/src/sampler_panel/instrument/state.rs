//! What the Instrument view reads: `mix.sampler_state` as the engine reports it, read again only
//! when the document changes (the engine's revision), never every frame. Every zone value the view
//! shows comes from here: the stored values beside the ones playback uses, the zone's pad and
//! membership, a slice-bound zone's derived region and the zone source's native rate. Pads are the
//! engine's effective notes. The view derives none of them.

use crate::StudioApp;
use serde::Deserialize;
use serde_json::json;
use transit_model::sampler_patch::{LoopMode, PatchV1, PlayMode, ZoneEntry};
use transit_model::{SourceId, TrackId};

/// A zone's place in a key-map group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Place {
    pub group: u8,
    pub layer: u8,
    pub alternative: u8,
}

/// Edge-fade widths (keys low and high, velocities low and high).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct FadeWidths {
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
}

impl FadeWidths {
    pub fn all(&self) -> [u8; 4] {
        [self.key_lo, self.key_hi, self.vel_lo, self.vel_hi]
    }
}

/// A slice-bound zone's slice: its set, id and 1-based number in that set.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SliceRef {
    pub id: u32,
    pub slice: usize,
}

/// A region in source frames (a slice-bound zone's derived one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Region {
    pub source: u16,
    pub start: u64,
    pub end: u64,
    pub loop_start: u64,
    pub loop_end: u64,
}

/// One zone as `mix.sampler_state` reports it (zone and pad numbers from 1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Zone {
    pub zone: u16,
    pub pad: Option<u8>,
    pub keys: [u8; 2],
    pub velocities: [u8; 2],
    pub root: u8,
    pub mode: PlayMode,
    pub membership: Option<Place>,
    pub loop_mode: LoopMode,
    pub loop_used: LoopMode,
    pub crossfade: u64,
    pub crossfade_used: u64,
    pub fades: FadeWidths,
    pub fades_used: FadeWidths,
    pub slice: Option<SliceRef>,
    pub region: Option<Region>,
    /// The zone source's native rate (the crossfade field's millisecond conversion).
    pub rate: u32,
}

/// One pad (numbers from 1; `note` is the effective note).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Pad {
    pub pad: u8,
    pub note: u8,
    pub zone: Option<u16>,
}

/// One group with a member or a stored mode.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Group {
    pub group: u8,
    pub zones: Vec<u16>,
}

/// The whole read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct State {
    /// The patch as stored (the conflict preview's input to the model's own admission checks).
    pub patch: PatchV1,
    pub pad_start_note: u8,
    pub pads: Vec<Pad>,
    pub zones: Vec<Zone>,
    pub groups: Vec<Group>,
}

/// A zone's region as playback reads it: the source, start and end, and the loop points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub source: Option<SourceId>,
    pub start: u64,
    pub end: u64,
    pub loop_start: u64,
    pub loop_end: u64,
}

impl State {
    /// Zone number `n` (from 1).
    pub fn zone(&self, n: u16) -> Option<&Zone> {
        self.zones.iter().find(|z| z.zone == n)
    }

    /// The stored entry of zone number `n`.
    pub fn entry(&self, n: u16) -> Option<&ZoneEntry> {
        n.checked_sub(1).and_then(|id| self.patch.zone(id))
    }

    /// The effective note of pad number `pad` (from 1).
    pub fn pad_note(&self, pad: u8) -> Option<u8> {
        self.pads.iter().find(|p| p.pad == pad).map(|p| p.note)
    }

    /// The zones sharing `z`'s key range in its group (all of them), or `z` alone.
    pub fn key_peers<'a>(&'a self, z: &'a Zone) -> Vec<&'a Zone> {
        match z.membership {
            Some(m) => self.zones.iter().filter(|o| o.membership.is_some_and(|p| p.group == m.group)).collect(),
            None => vec![z],
        }
    }

    /// The zones of `z`'s layer (its alternatives), or `z` alone.
    pub fn layer_peers<'a>(&'a self, z: &'a Zone) -> Vec<&'a Zone> {
        match z.membership {
            Some(m) => self.zones.iter().filter(|o| o.membership.is_some_and(|p| p.group == m.group && p.layer == m.layer)).collect(),
            None => vec![z],
        }
    }

    /// Zone `z`'s region: the derived one for a slice-bound zone, else its own (the region in use).
    pub fn span(&self, z: &Zone) -> Option<Span> {
        let e = self.entry(z.zone)?;
        let (source, start, end, loop_start, loop_end) = match z.region {
            Some(r) => (r.source, r.start, r.end, r.loop_start, r.loop_end),
            None => (e.source, e.start, e.end, e.loop_start, e.loop_end),
        };
        let source = self.patch.sources.get(usize::from(source)).map(|s| s.source);
        Some(Span { source, start, end, loop_start, loop_end })
    }
}

/// The last read and the revision it was read at.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub data: Option<State>,
    /// A read the engine refused, in its words.
    pub error: Option<String>,
    revision: Option<u64>,
    /// Reads so far (each is one query; the tests count them).
    pub reads: u64,
}

impl Snapshot {
    /// Reads the state again when the document changed since the last read.
    pub fn refresh(&mut self, app: &mut StudioApp, track: TrackId) {
        let revision = app.engine.revision;
        if self.revision == Some(revision) {
            return;
        }
        self.revision = Some(revision);
        self.reads += 1;
        let read = app.engine.execute("mix.sampler_state", &json!({"track": track.0})).map_err(|e| super::super::slices::edit::words(&e));
        match read.and_then(|v| serde_json::from_value::<State>(v).map_err(|e| e.to_string())) {
            Ok(s) => {
                self.data = Some(s);
                self.error = None;
            }
            Err(e) => {
                self.data = None;
                self.error = Some(e);
            }
        }
    }
}
