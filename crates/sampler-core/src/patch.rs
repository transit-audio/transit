//! The patch the core plays (ADR 0041 decision 2 and the capacities table): up to [`ZONES`] zones
//! and [`PADS`] pads.
//!
//! A **zone** is one sample region with its key and velocity ranges, root, playback settings,
//! optional layer/round-robin membership (the key-map group's selection is `selection.rs`), and its
//! shaper and matrix settings. Each group's round-robin mode ([`GroupMode`]) comes with the patch. A **pad** binds one zone to one unique trigger
//! note, with mute, solo and a choke group. A pad-bound zone is selected only through its pad, never a
//! second time by the unbound-zone key scan; it has no membership. A pad's tune, reverse and amp
//! envelope are its zone's playback settings (one settings block per zone).

use transit_instrument_core::matrix::Matrix;
use transit_instrument_core::shaper::Settings as ShaperSettings;

use crate::envelope::EnvelopeFrames;
use crate::playhead::Region;
use crate::{CHOKE_GROUPS, PADS, ZONES, math, selection};

/// How a zone plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayMode {
    /// Plays its region once to the end, ignoring key-up and sustain; never loops.
    #[default]
    OneShot,
    /// Plays while held or sustained (looping if the region loops), then releases.
    Gate,
}

/// A zone's playback settings (the controls of a pad or an Instrument zone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playback {
    /// -48..12 dB.
    pub level_db: f32,
    /// -1..1, balance with unity at center.
    pub pan: f32,
    /// -48..48 semitones.
    pub transpose: f32,
    /// -100..100 cents.
    pub cents: f32,
    pub reverse: bool,
    pub mode: PlayMode,
    /// Pitch follows the key from `root` (pads default off, zones on).
    pub key_tracking: bool,
    pub root: u8,
    /// 0..5000 ms.
    pub attack_ms: f32,
    /// 0..10000 ms.
    pub decay_ms: f32,
    /// 0..1.
    pub sustain: f32,
    /// 0..10000 ms.
    pub release_ms: f32,
    /// 0..10 ms.
    pub declick_ms: f32,
    /// 0..1 (default 0.7): gain `(1 - s) + s * velocity / 127`, before the amp envelope.
    pub velocity_sensitivity: f32,
}

impl Playback {
    /// A new pad's defaults: one-shot, key tracking off, root at its trigger note.
    #[must_use]
    pub fn pad(note: u8) -> Playback {
        Playback { mode: PlayMode::OneShot, key_tracking: false, root: note.min(127), ..Playback::zone() }
    }

    /// A new Instrument zone's defaults: Gate, key tracking on, root 60.
    #[must_use]
    pub fn zone() -> Playback {
        Playback {
            level_db: 0.0,
            pan: 0.0,
            transpose: 0.0,
            cents: 0.0,
            reverse: false,
            mode: PlayMode::Gate,
            key_tracking: true,
            root: 60,
            attack_ms: 1.0,
            decay_ms: 300.0,
            sustain: 1.0,
            release_ms: 150.0,
            declick_ms: 0.0,
            velocity_sensitivity: 0.7,
        }
    }
}

/// A zone's place in a multisample key-map group: which velocity layer and which round-robin
/// alternative it is. A group has one key range, up to eight velocity layers and up to eight
/// alternatives per layer with identical ranges, numbered contiguously from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Membership {
    pub group: u8,
    /// 0..8.
    pub layer: u8,
    /// 0..8.
    pub alternative: u8,
}

/// How a key-map group picks among the alternatives of a velocity layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum GroupMode {
    /// Each (group, layer) counter from 0, `counter % count`, advancing once per admitted trigger.
    #[default]
    Sequential,
    /// The group's seeded xorshift32 state (13, 17, 5), `word % count`, one draw per admitted trigger.
    Random,
}

/// One sample-bearing zone.
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub region: Region,
    /// Inclusive key range.
    pub keys: (u8, u8),
    /// Inclusive velocity range (1..=127).
    pub velocities: (u8, u8),
    pub playback: Playback,
    pub membership: Option<Membership>,
    /// The per-voice shaper's settings at the session rate (`on` false: the audio passes unshaped).
    pub shaper: ShaperSettings,
    /// The zone's 16-row modulation matrix.
    pub matrix: Matrix,
}

impl Zone {
    /// A zone over `region` with Instrument defaults, the shaper off and an empty matrix.
    #[must_use]
    pub fn new(region: Region) -> Zone {
        Zone {
            region,
            keys: (0, 127),
            velocities: (1, 127),
            playback: Playback::zone(),
            membership: None,
            shaper: ShaperSettings::default(),
            matrix: Matrix::default(),
        }
    }
}

/// One pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pad {
    /// The bound zone (`None`: an empty pad).
    pub zone: Option<u16>,
    /// The trigger note, unique among pads.
    pub note: u8,
    pub mute: bool,
    pub solo: bool,
    /// 0 Off, 1..=16 a choke group.
    pub choke: u8,
}

impl Pad {
    /// Pad `index`'s default: empty, trigger note `60 + index`.
    #[must_use]
    pub fn new(index: usize) -> Pad {
        Pad { zone: None, note: (60 + index.min(67)) as u8, mute: false, solo: false, choke: 0 }
    }
}

/// Why a patch edit was refused (the patch is unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchError {
    /// The zone index is past [`ZONES`] (or a pad names a zone that is not there).
    Zone,
    /// The pad index is past [`PADS`].
    Pad,
    /// The region's bounds do not hold ([`Region::is_valid`]).
    Region,
    /// A key, velocity or setting is out of its range or not finite.
    Range,
    /// Another pad has this trigger note.
    DuplicateNote,
    /// The choke group is past [`CHOKE_GROUPS`].
    ChokeGroup,
    /// A pad-bound zone cannot have layer/round-robin membership.
    BoundMembership,
    /// Another pad already binds this zone (one zone per pad).
    ZoneTaken,
    /// A key-map group's shape does not hold: members with different key ranges, alternatives of
    /// a layer with different velocity ranges, or alternatives repeated or not contiguous from 0.
    GroupShape,
}

/// A level and pan change gliding linearly over `len` output frames from absolute frame `start`
/// (see [`crate::SamplerCore::set_zone_glide`]): frame `start + j` has
/// `from + (to - from) * (j + 1) / len`, so the last frame of the glide is the new value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Glide {
    /// The linear level and the pan the glide starts from.
    pub from_level: f32,
    pub from_pan: f32,
    pub start: u64,
    pub len: u32,
}

/// What the voices read of a zone, worked out when it is set or the rate changes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Derived {
    pub env: EnvelopeFrames,
    pub level: f32,
    pub declick: u32,
    /// A level and pan change still gliding in.
    pub glide: Option<Glide>,
}

impl Derived {
    pub(crate) fn of(p: &Playback, rate: f32) -> Derived {
        Derived {
            env: EnvelopeFrames::from_ms(p.attack_ms, p.decay_ms, p.sustain, p.release_ms, rate),
            level: math::powf(10.0, p.level_db / 20.0),
            declick: crate::envelope::ms_to_frames(p.declick_ms, rate),
            glide: None,
        }
    }

    /// The linear level and the pan at absolute frame `frame`, `pan` being the zone's own: the
    /// glide's value while it runs, else the zone's values.
    pub(crate) fn level_pan_at(&self, pan: f32, frame: u64) -> (f32, f32) {
        match self.glide {
            Some(g) if frame < g.start.saturating_add(u64::from(g.len)) && g.len > 0 => {
                let k = (frame.saturating_sub(g.start) + 1) as f32 / g.len as f32;
                (g.from_level + (self.level - g.from_level) * k, g.from_pan + (pan - g.from_pan) * k)
            }
            _ => (self.level, pan),
        }
    }

    /// Whether a glide runs at absolute frame `frame`.
    pub(crate) fn glides_at(&self, frame: u64) -> bool {
        self.glide.is_some_and(|g| frame < g.start.saturating_add(u64::from(g.len)))
    }
}

fn in_range(x: f32, lo: f32, hi: f32) -> bool {
    x.is_finite() && (lo..=hi).contains(&x)
}

/// Checks a zone's settings (not its source, whose length is known only when a voice starts).
pub(crate) fn check_zone(z: &Zone) -> Result<(), PatchError> {
    if !z.region.is_valid() {
        return Err(PatchError::Region);
    }
    let p = &z.playback;
    let ok = z.keys.0 <= z.keys.1
        && z.keys.1 <= 127
        && 1 <= z.velocities.0
        && z.velocities.0 <= z.velocities.1
        && z.velocities.1 <= 127
        && p.root <= 127
        && in_range(p.level_db, -48.0, 12.0)
        && in_range(p.pan, -1.0, 1.0)
        && in_range(p.transpose, -48.0, 48.0)
        && in_range(p.cents, -100.0, 100.0)
        && in_range(p.attack_ms, 0.0, 5000.0)
        && in_range(p.decay_ms, 0.0, 10_000.0)
        && in_range(p.sustain, 0.0, 1.0)
        && in_range(p.release_ms, 0.0, 10_000.0)
        && in_range(p.declick_ms, 0.0, 10.0)
        && in_range(p.velocity_sensitivity, 0.0, 1.0)
        && z.membership.is_none_or(|m| m.layer < 8 && m.alternative < 8);
    if ok { Ok(()) } else { Err(PatchError::Range) }
}

/// Checks pad `index` against the other pads and the zones present.
pub(crate) fn check_pad(index: usize, pad: &Pad, pads: &[Pad; PADS], zone: impl Fn(usize) -> Option<Zone>) -> Result<(), PatchError> {
    if index >= PADS {
        return Err(PatchError::Pad);
    }
    if pad.note > 127 {
        return Err(PatchError::Range);
    }
    if pad.choke > CHOKE_GROUPS {
        return Err(PatchError::ChokeGroup);
    }
    if pads.iter().enumerate().any(|(i, p)| i != index && p.note == pad.note) {
        return Err(PatchError::DuplicateNote);
    }
    if let Some(z) = pad.zone {
        if pads.iter().enumerate().any(|(i, p)| i != index && p.zone == Some(z)) {
            return Err(PatchError::ZoneTaken);
        }
        let z = usize::from(z);
        if z >= ZONES {
            return Err(PatchError::Zone);
        }
        match zone(z) {
            None => return Err(PatchError::Zone),
            Some(z) if z.membership.is_some() => return Err(PatchError::BoundMembership),
            Some(_) => {}
        }
    }
    Ok(())
}

/// Checks a whole patch at once: every zone's settings, every key-map group's shape, and every pad
/// against the other pads of `pads` and the zones of `zones` (`zones` is [`ZONES`] long). The control side checks a patch with
/// this before it publishes it; [`crate::SamplerCore::set_patch`] checks again before adopting it.
pub fn check_patch(zones: &[Option<Zone>], pads: &[Pad; PADS]) -> Result<(), PatchError> {
    if zones.len() != ZONES {
        return Err(PatchError::Zone);
    }
    for z in zones.iter().flatten() {
        check_zone(z)?;
    }
    selection::check_groups(|i| zones.get(i).and_then(Option::as_ref).and_then(selection::shape))?;
    for (i, pad) in pads.iter().enumerate() {
        check_pad(i, pad, pads, |z| zones.get(z).copied().flatten())?;
    }
    Ok(())
}
