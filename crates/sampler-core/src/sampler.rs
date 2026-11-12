//! [`SamplerCore`]: the patch, the voice pool, event handling and the block render.
//!
//! **Admission.** A note-on triggers the pad whose trigger note it is and then the unbound zones whose
//! inclusive key and velocity ranges hold it, in zone order, at most [`crate::LAYERS_PER_NOTE`] voices (the
//! pad counts as one; a key-map group plays one alternative per admitted velocity layer, chosen as
//! `selection.rs` describes, at the place of its first zone in the order). A muted pad, or a
//! pad that is not solo while any pad is, admits nothing and chokes nothing. An admitted pad trigger
//! first fades every other sounding pad of its choke group over [`crate::CHOKE_FRAMES`], then starts its
//! voice: a pad has one voice, and a retrigger cuts the previous one at the event frame and starts
//! at the region entry with a fresh envelope, taking the new note identity. A start the source
//! cannot serve (missing, not mono or stereo, shorter than the region, no usable rate or step) is
//! refused and counted.
//!
//! **Voices.** [`crate::VOICES`] in all, tails included. A new voice takes a free slot in index order, else
//! steals the voice whose release (or fade) began first, else the oldest sounding voice, ties by slot
//! index; a stolen voice is cut at once and counted. A pad's retrigger reuses its own voice and steals
//! nothing. Voices render summed in zone order, then start order, whatever their slots.

use alloc::vec::Vec;

use crate::event::{Context, Control, Event, EventKind};
use crate::fades;
use crate::patch::{self, Derived, Glide, GroupMode, Pad, PatchError, Zone};
use crate::reader::SourceBank;
use crate::selection::{self, RoundRobin};
use crate::voice::{Controls, Owner, Start, Transport, Voice};
use crate::{ALTERNATIVES, GROUPS, LAYERS, LAYERS_PER_NOTE, PADS, VOICES, ZONES, math};
use transit_instrument_core::pairing::NotePairing;

#[cfg(test)]
#[path = "tests/steal_ties.rs"]
mod steal_ties;

/// Counters the adapter reads (they only grow; [`SamplerCore::reset`] keeps them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Diagnostics {
    /// Voice starts refused: no source, a source that is not mono or stereo or is shorter than the
    /// region, or an unusable rate or step.
    pub refused_starts: u64,
    /// Voices faded by a choke group, a mute, or a solo that excludes their pad.
    pub chokes: u64,
    /// Voices cut to make room for a new one.
    pub steals: u64,
    /// Voice-blocks that read a missing frame (played as zero): one per voice per `process` call.
    pub underruns: u64,
    /// Key-map group checks a zone edit ran (each one a pass over every zone of every group): only
    /// an edit that changes a zone's group, layer, alternative, keys or velocities runs one (#490).
    pub group_checks: u64,
}

/// One sounding voice's place ([`SamplerCore::voice_positions`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoicePosition {
    /// The zone slot the voice plays (a fading voice of a replaced zone keeps its slot).
    pub zone: u16,
    /// The phase in the source's native frames (above -1: only a crossfaded reverse loop that
    /// starts at frame 0 reads below 0).
    pub phase: f64,
}

#[derive(Debug, Clone)]
struct Slot {
    zone: Zone,
    derived: Derived,
}

/// The sampler core. Allocates in [`SamplerCore::prepare`] only.
#[derive(Debug, Clone)]
pub struct SamplerCore {
    rate: f32,
    seed: u32,
    zones: Vec<Option<Slot>>,
    /// The settings of each cleared zone, which its voices read while they fade out.
    ghosts: Vec<Option<Slot>>,
    /// The pad each zone is bound to.
    bound: [Option<u8>; ZONES],
    pads: [Pad; PADS],
    voices: Vec<Voice>,
    /// Each pad's voice: slot and serial (stale once that voice ends or is reused).
    pad_voice: [Option<(u8, u64)>; PADS],
    /// Active voices in render order (zone, then start order).
    order: [u8; VOICES],
    order_len: usize,
    serial: u64,
    controls: Controls,
    any_solo: bool,
    diag: Diagnostics,
    /// The round-robin modes, counters and random states.
    rr: RoundRobin,
    /// Each (group, layer)'s alternatives by zone index (`GROUPS * LAYERS` entries, allocated by
    /// [`SamplerCore::prepare`]).
    members: Vec<[u16; ALTERNATIVES]>,
    /// Whether the last block reported a playing transport (`None`: no block yet).
    was_playing: Option<bool>,
    /// The open note-ons per key and channel: a note-off without a host id closes the oldest
    /// (#292).
    pairing: NotePairing,
}

impl Default for SamplerCore {
    fn default() -> Self {
        SamplerCore::new()
    }
}

impl SamplerCore {
    /// An unprepared core (nothing allocated yet): call [`SamplerCore::prepare`] before editing or
    /// processing.
    #[must_use]
    pub fn new() -> SamplerCore {
        SamplerCore {
            rate: 48_000.0,
            seed: 1,
            zones: Vec::new(),
            ghosts: Vec::new(),
            bound: [None; ZONES],
            pads: core::array::from_fn(Pad::new),
            voices: Vec::new(),
            pad_voice: [None; PADS],
            order: [0; VOICES],
            order_len: 0,
            serial: 0,
            controls: Controls::new(),
            any_solo: false,
            diag: Diagnostics::default(),
            rr: RoundRobin::new(1),
            members: Vec::new(),
            was_playing: None,
            pairing: NotePairing::new(),
        }
    }

    /// Prepares for `sample_rate` (Hz): allocates the voices and the zone settings (once), works the
    /// zones' durations out at the rate, seeds the voices' random state and the key-map groups'
    /// random round robin from `seed` (the patch seed; 0 is 1 for the round robin), and resets. The
    /// zones' shaper settings must be built at this rate.
    pub fn prepare(&mut self, sample_rate: f32, seed: u32) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        if self.zones.len() != ZONES {
            self.zones = (0..ZONES).map(|_| None).collect();
        }
        if self.ghosts.len() != ZONES {
            self.ghosts = (0..ZONES).map(|_| None).collect();
        }
        if self.members.len() != GROUPS * LAYERS {
            self.members = alloc::vec![[selection::NONE; ALTERNATIVES]; GROUPS * LAYERS];
            selection::rebuild(&mut self.members, self.zones.iter().map(|s| s.as_ref().map(|s| &s.zone)));
        }
        self.rr.set_seed(seed);
        if self.voices.len() != VOICES || self.seed != seed {
            self.voices =
                (0..VOICES as u32).map(|i| Voice::new(seed.wrapping_mul(0x9E37_79B9) ^ i.wrapping_add(1).wrapping_mul(0x85EB_CA6B))).collect();
        }
        self.seed = seed;
        let rate = self.rate;
        for s in self.zones.iter_mut().chain(self.ghosts.iter_mut()).flatten() {
            s.derived = Derived::of(&s.zone.playback, rate);
        }
        self.reset();
    }

    /// The session rate of the last [`SamplerCore::prepare`].
    #[must_use]
    pub fn sample_rate(&self) -> f32 {
        self.rate
    }

    /// Sets (or with `None` clears) zone `index`. Voices playing it fade over [`crate::CHOKE_FRAMES`] when its
    /// region changes or it is cleared (ADR 0041's structural-edit rule; a cleared zone's voices
    /// read its last settings while they fade). A zone bound to a pad cannot be cleared or given
    /// membership, and the key-map groups must keep their shape ([`PatchError::GroupShape`]).
    pub fn set_zone(&mut self, index: usize, zone: Option<Zone>) -> Result<(), PatchError> {
        if index >= ZONES || self.zones.len() != ZONES {
            return Err(PatchError::Zone);
        }
        let bound = self.bound.get(index).copied().flatten().is_some();
        if let Some(z) = &zone {
            patch::check_zone(z)?;
            if bound && z.membership.is_some() {
                return Err(PatchError::BoundMembership);
            }
        } else if bound {
            return Err(PatchError::ZoneTaken);
        }
        let shape = zone.as_ref().and_then(selection::shape);
        let old_member = self.zone(index).and_then(|z| z.membership);
        // The zones in place always pass the check, so an edit that keeps this zone's shape (a
        // level, pan or shaper change) leaves every group as it was and needs no pass (#490).
        if (shape.is_some() || old_member.is_some()) && shape != self.zone(index).and_then(selection::shape) {
            self.diag.group_checks += 1;
            selection::check_groups(|i| if i == index { shape } else { self.zone(i).and_then(selection::shape) })?;
        }
        let old = self.zones.get(index).and_then(|s| s.as_ref().map(|s| s.zone.region));
        if old != zone.map(|z| z.region) {
            self.fade_where(|v| usize::from(v.zone) == index);
        }
        if let Some(m) = old_member
            && let Some(slot) = self.members.get_mut(selection::entry(m.group, m.layer)).and_then(|e| e.get_mut(usize::from(m.alternative)))
            && usize::from(*slot) == index
        {
            *slot = selection::NONE;
        }
        if let Some(m) = zone.and_then(|z| z.membership)
            && let Some(slot) = self.members.get_mut(selection::entry(m.group, m.layer)).and_then(|e| e.get_mut(usize::from(m.alternative)))
        {
            *slot = index as u16;
        }
        self.replace_slot(index, zone);
        Ok(())
    }

    /// Puts `zone` in slot `index`; a cleared zone's settings move to its ghost for its fading
    /// voices. No allocation (the slots are moved, not boxed).
    fn replace_slot(&mut self, index: usize, zone: Option<Zone>) {
        let rate = self.rate;
        let Some(slot) = self.zones.get_mut(index) else { return };
        let old = core::mem::replace(slot, zone.map(|zone| Slot { derived: Derived::of(&zone.playback, rate), zone }));
        if slot.is_none()
            && old.is_some()
            && let Some(ghost) = self.ghosts.get_mut(index)
        {
            *ghost = old;
        }
    }

    /// Sets zone `index` as [`SamplerCore::set_zone`] does, but a change of its level or pan
    /// glides: it ramps linearly from the value at absolute frame `start` (the
    /// [`crate::event::Context::frame`] of the block it applies in) to the new one over `frames`
    /// output frames, the same for every voice of the zone, each frame by the same increment.
    /// Other settings apply at once. The adapter's slot level and pan changes come this way.
    pub fn set_zone_glide(&mut self, index: usize, zone: Zone, start: u64, frames: u32) -> Result<(), PatchError> {
        let old = self.zones.get(index).and_then(Option::as_ref).map(|s| s.derived.level_pan_at(s.zone.playback.pan, start));
        self.set_zone(index, Some(zone))?;
        if let (Some((from_level, from_pan)), Some(Some(slot))) = (old, self.zones.get_mut(index))
            && frames > 0
            && (from_level != slot.derived.level || from_pan != slot.zone.playback.pan)
        {
            slot.derived.glide = Some(Glide { from_level, from_pan, start, len: frames });
        }
        Ok(())
    }

    /// Replaces the whole patch (every zone and pad) in one step, as an adapter adopts a patch it
    /// prepared off the audio thread: checked first with [`patch::check_patch`] (on an error nothing
    /// changes), then applied with the rules of [`SamplerCore::set_zone`] and
    /// [`SamplerCore::set_pad`]: voices of a zone whose region changes or that is cleared fade
    /// over [`crate::CHOKE_FRAMES`], a pad voice fades when the
    /// pad's zone changes or the new mute and solo exclude it; other voices play on. `zones` is
    /// [`ZONES`] long. Neither allocates nor frees.
    pub fn set_patch(&mut self, zones: &[Option<Zone>], pads: &[Pad; PADS]) -> Result<(), PatchError> {
        if self.zones.len() != ZONES {
            return Err(PatchError::Zone);
        }
        patch::check_patch(zones, pads)?;
        for (index, zone) in zones.iter().enumerate() {
            let old = self.zones.get(index).and_then(|s| s.as_ref().map(|s| s.zone.region));
            if old != zone.map(|z| z.region) {
                self.fade_where(|v| usize::from(v.zone) == index);
            }
            self.replace_slot(index, *zone);
        }
        selection::rebuild(&mut self.members, zones.iter().map(Option::as_ref));
        for (index, pad) in pads.iter().enumerate() {
            let changed = self.pads.get(index).is_some_and(|o| o.zone != pad.zone);
            if changed && let Some(vi) = self.live_pad_voice(index) {
                self.fade_voice(vi);
            }
        }
        self.pads = *pads;
        self.bound = [None; ZONES];
        for (i, p) in self.pads.iter().enumerate() {
            if let Some(b) = p.zone.and_then(|z| self.bound.get_mut(usize::from(z))) {
                *b = Some(i as u8);
            }
        }
        self.any_solo = self.pads.iter().any(|p| p.solo);
        for q in 0..PADS {
            if !self.admits(q)
                && let Some(vi) = self.live_pad_voice(q)
            {
                self.fade_voice(vi);
            }
        }
        Ok(())
    }

    /// Sets every key-map group's round-robin mode (by group id; a group the caller has no entry for
    /// is [`GroupMode::Sequential`]). A patch edit: counters and random states carry on. Neither
    /// allocates nor frees.
    pub fn set_group_modes(&mut self, modes: &[GroupMode; GROUPS]) {
        self.rr.set_modes(modes);
    }

    /// The source index of every sounding voice (tails included), in voice-slot order: an adapter
    /// keeps a source generation until no voice reads it.
    pub fn playing_sources(&self) -> impl Iterator<Item = u16> + '_ {
        self.voices.iter().filter(|v| v.active).map(Voice::source)
    }

    /// Where every sounding voice reads (tails included), in voice-slot order: its zone slot and its
    /// phase in its source's native frames as the last block left it (the frame the next output
    /// frame reads: pitch, the source-to-session rate, loops, ping-pong and reverse included). A
    /// display seam (the Slices view's playheads, #470): reading it changes nothing, and it neither
    /// allocates nor frees. At most [`crate::VOICES`] items.
    pub fn voice_positions(&self) -> impl Iterator<Item = VoicePosition> + '_ {
        self.voices.iter().filter(|v| v.active).map(|v| VoicePosition { zone: v.zone, phase: v.phase() })
    }

    /// Zone `index`, if set.
    #[must_use]
    pub fn zone(&self, index: usize) -> Option<&Zone> {
        self.zones.get(index).and_then(|s| s.as_ref().map(|s| &s.zone))
    }

    /// Sets pad `index`. A sounding pad that becomes muted, excluded by another pad's solo, or bound to
    /// another zone fades over [`crate::CHOKE_FRAMES`].
    pub fn set_pad(&mut self, index: usize, pad: Pad) -> Result<(), PatchError> {
        patch::check_pad(index, &pad, &self.pads, |z| self.zone(z).copied())?;
        let old = self.pads.get(index).copied();
        if let Some(p) = self.pads.get_mut(index) {
            *p = pad;
        }
        if old.is_some_and(|o| o.zone != pad.zone)
            && let Some(vi) = self.live_pad_voice(index)
        {
            self.fade_voice(vi);
        }
        self.bound = [None; ZONES];
        for (i, p) in self.pads.iter().enumerate() {
            if let Some(b) = p.zone.and_then(|z| self.bound.get_mut(usize::from(z))) {
                *b = Some(i as u8);
            }
        }
        self.any_solo = self.pads.iter().any(|p| p.solo);
        for q in 0..PADS {
            if !self.admits(q)
                && let Some(vi) = self.live_pad_voice(q)
            {
                self.fade_voice(vi);
            }
        }
        Ok(())
    }

    /// Pad `index`.
    #[must_use]
    pub fn pad(&self, index: usize) -> Option<&Pad> {
        self.pads.get(index)
    }

    /// The counters.
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics {
        self.diag
    }

    /// Voices sounding now (tails included).
    #[must_use]
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Every voice cut, controllers cleared, the voices' random state back to its seed, every
    /// round-robin counter and group random state restarted, and every
    /// zone's level and pan glide completed: the new value is the current one. A glide is keyed to
    /// the caller's absolute frame, which a reset sets back to zero, so a glide left in place would
    /// replay the old value until the frame climbed back to it.
    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.reset();
        }
        for s in self.zones.iter_mut().chain(self.ghosts.iter_mut()).flatten() {
            s.derived.glide = None;
        }
        self.controls = Controls::new();
        self.pad_voice = [None; PADS];
        self.order_len = 0;
        self.serial = 0;
        self.pairing.clear();
        self.rr.restart();
    }

    /// Renders one block into `out` (1 or 2 channels; the shortest sets the block's length; a mono
    /// output takes the average of left and right), acting on `events` at their frames. The first
    /// block whose context reports a transport state (playing: [`Context::quarters`] is some) other
    /// than the previous block's restarts every round-robin counter and group random state, at its
    /// first frame, before its events.
    pub fn process<B: SourceBank + ?Sized>(&mut self, ctx: &Context, sources: &B, events: &[Event], out: &mut [&mut [f32]]) {
        let playing = ctx.quarters.is_some();
        if self.was_playing.is_some_and(|was| was != playing) {
            self.rr.restart();
        }
        self.was_playing = Some(playing);
        let frames = out.iter().map(|c| c.len()).min().unwrap_or(0);
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let mut i = 0;
        let mut cursor = 0usize;
        loop {
            let mut started = false;
            while let Some(e) = events.get(i) {
                if (e.frame as usize).min(frames) > cursor {
                    break;
                }
                started |= self.apply(e.kind, ctx, cursor, sources);
                i += 1;
            }
            if started {
                self.sort();
            }
            let next = events.get(i).map_or(frames, |e| (e.frame as usize).min(frames));
            if next > cursor {
                self.render(ctx, sources, out, cursor, next);
                cursor = next;
            }
            if i >= events.len() && cursor >= frames {
                break;
            }
        }
        for v in &mut self.voices {
            if v.missed {
                v.missed = false;
                self.diag.underruns += 1;
            }
        }
    }

    fn render<B: SourceBank + ?Sized>(&mut self, ctx: &Context, sources: &B, out: &mut [&mut [f32]], from: usize, to: usize) {
        let SamplerCore { voices, zones, ghosts, controls, order, order_len, rate, .. } = self;
        let per_frame = ctx.tempo / 60.0 / f64::from(*rate);
        for &vi in order.iter().take(*order_len) {
            let Some(v) = voices.get_mut(usize::from(vi)) else { continue };
            if !v.active {
                continue;
            }
            let z = usize::from(v.zone);
            let Some(slot) = zones.get(z).and_then(Option::as_ref).or_else(|| ghosts.get(z).and_then(Option::as_ref)) else {
                v.cut();
                continue;
            };
            let source = sources.source(usize::from(v.source()));
            for n in from..to {
                if !v.active {
                    break;
                }
                let t =
                    Transport { tempo: ctx.tempo, quarters: ctx.quarters.map(|q| q + n as f64 * per_frame), frame: ctx.frame.wrapping_add(n as u64) };
                let (l, r) = v.frame(&slot.zone, &slot.derived, source, controls, t);
                match out {
                    [m] => {
                        if let Some(x) = m.get_mut(n) {
                            *x += (l + r) * 0.5;
                        }
                    }
                    [lo, ro, ..] => {
                        if let Some(x) = lo.get_mut(n) {
                            *x += l;
                        }
                        if let Some(x) = ro.get_mut(n) {
                            *x += r;
                        }
                    }
                    [] => {}
                }
            }
        }
    }

    /// Rebuilds the render order: active voices by zone, then start order (insertion sort, no
    /// allocation).
    fn sort(&mut self) {
        let SamplerCore { voices, order, order_len, .. } = self;
        *order_len = 0;
        for (i, v) in voices.iter().enumerate() {
            if !v.active {
                continue;
            }
            let key = |o: Option<&u8>| o.and_then(|&p| voices.get(usize::from(p))).map_or((0, 0), |p| (p.zone, p.serial));
            let mut k = *order_len;
            if let Some(slot) = order.get_mut(k) {
                *slot = i as u8;
            }
            while k > 0 && key(order.get(k - 1)) > key(order.get(k)) {
                order.swap(k - 1, k);
                k -= 1;
            }
            *order_len += 1;
        }
    }

    /// Acts on one event at frame `n`; returns whether a voice started.
    fn apply<B: SourceBank + ?Sized>(&mut self, kind: EventKind, ctx: &Context, n: usize, sources: &B) -> bool {
        match kind {
            EventKind::NoteOn { channel, key, velocity, id } => {
                if velocity == 0 {
                    self.note_off(channel, key, 64, id);
                    false
                } else if key <= 127 && velocity <= 127 {
                    let seq = if id < 0 {
                        let held = self
                            .voices
                            .iter()
                            .any(|v| v.active && v.held && matches!(v.owner, Owner::Note { channel: c, key: k, .. } if c == channel && k == key));
                        self.pairing.on(channel, key, held)
                    } else {
                        0
                    };
                    self.note_on(Owner::Note { channel, key, id, seq }, key, velocity, ctx, n, sources)
                } else {
                    false
                }
            }
            EventKind::NoteOff { channel, key, velocity, id } => {
                self.note_off(channel, key, velocity, id);
                false
            }
            EventKind::PadPress { pad, velocity } => {
                let p = usize::from(pad);
                match self.pads.get(p) {
                    Some(&Pad { note, .. }) if velocity > 0 && velocity <= 127 => {
                        self.trigger_pad(p, Owner::Press { pad }, note, velocity, ctx, n, sources)
                    }
                    _ => false,
                }
            }
            EventKind::PadRelease { pad } => {
                self.key_up_where(|o| o == Owner::Press { pad }, 0);
                false
            }
            EventKind::Control(c) => {
                self.control(c);
                false
            }
            EventKind::AllNotesOff => {
                self.pairing.clear();
                for i in 0..self.voices.len() {
                    let one_shot = self.voices.get(i).is_some_and(|v| v.active && v.one_shot);
                    if one_shot {
                        self.fade_voice(i);
                    } else {
                        self.release_voice(i);
                    }
                }
                false
            }
            EventKind::AllSoundOff => {
                self.pairing.clear();
                self.voices.iter_mut().for_each(Voice::cut);
                false
            }
            EventKind::ResetControllers => {
                self.control(Control::Sustain(false));
                let k = &mut self.controls;
                k.wheel = 0.0;
                k.bend = 0.0;
                k.aftertouch = 0.0;
                k.poly = [0.0; 128];
                false
            }
            EventKind::Stop => {
                self.controls.sustain = false;
                for i in 0..self.voices.len() {
                    self.fade_voice(i);
                }
                false
            }
            EventKind::Reset => {
                self.reset();
                false
            }
        }
    }

    fn control(&mut self, c: Control) {
        let unit = |x: f32, lo: f32| if x.is_finite() { x.clamp(lo, 1.0) } else { 0.0 };
        let k = &mut self.controls;
        match c {
            Control::Sustain(down) => {
                k.sustain = down;
                if !down {
                    for i in 0..self.voices.len() {
                        if self.voices.get(i).is_some_and(|v| v.active && v.sustained) {
                            self.release_voice(i);
                        }
                    }
                }
            }
            Control::ModWheel(x) => k.wheel = unit(x, 0.0),
            Control::PitchBend(x) => k.bend = unit(x, -1.0),
            Control::Aftertouch(x) => k.aftertouch = unit(x, 0.0),
            Control::PolyAftertouch { key, value } => {
                if let Some(p) = k.poly.get_mut(usize::from(key)) {
                    *p = unit(value, 0.0);
                }
            }
            Control::Macro { index, value } => {
                if let Some(m) = k.macros.get_mut(usize::from(index)) {
                    *m = unit(value, 0.0);
                }
            }
        }
    }

    /// Whether pad `p` admits triggers now (not muted, not excluded by a solo).
    fn admits(&self, p: usize) -> bool {
        self.pads.get(p).is_some_and(|pad| !pad.mute && (!self.any_solo || pad.solo))
    }

    /// Pad `p`'s voice, if it is still the one the pad started.
    fn live_pad_voice(&self, p: usize) -> Option<usize> {
        let (vi, serial) = self.pad_voice.get(p).copied().flatten()?;
        let v = self.voices.get(usize::from(vi))?;
        (v.active && v.serial == serial && v.pad == Some(p as u8)).then_some(usize::from(vi))
    }

    fn fade_voice(&mut self, i: usize) {
        self.serial += 1;
        let order = self.serial;
        if self.voices.get_mut(i).is_some_and(|v| v.choke(order)) {
            self.diag.chokes += 1;
        }
    }

    fn fade_where(&mut self, f: impl Fn(&Voice) -> bool) {
        for i in 0..self.voices.len() {
            if self.voices.get(i).is_some_and(|v| v.active && f(v)) {
                self.fade_voice(i);
            }
        }
    }

    fn derived_of(&self, v: &Voice) -> Option<Derived> {
        self.zones.get(usize::from(v.zone)).and_then(|s| s.as_ref().map(|s| s.derived))
    }

    fn release_voice(&mut self, i: usize) {
        self.serial += 1;
        let order = self.serial;
        let d = self.voices.get(i).and_then(|v| self.derived_of(v));
        if let (Some(v), Some(d)) = (self.voices.get_mut(i), d) {
            v.release(&d, order);
        }
    }

    fn key_up_where(&mut self, owns: impl Fn(Owner) -> bool, velocity: u8) {
        let pedal = self.controls.sustain;
        for i in 0..self.voices.len() {
            let Some(v) = self.voices.get(i) else { continue };
            if !(v.active && v.held && owns(v.owner)) {
                continue;
            }
            let d = self.derived_of(v);
            self.serial += 1;
            let order = self.serial;
            if let (Some(v), Some(d)) = (self.voices.get_mut(i), d) {
                v.key_up(&d, pedal, velocity, order);
            }
        }
    }

    /// A note-off: by the host's note id when it has one, else the voices of the oldest open
    /// note-on of its key and channel (#292; every voice of that key and channel when none is
    /// counted open).
    fn note_off(&mut self, channel: u8, key: u8, velocity: u8, id: i32) {
        let closes = if id >= 0 { None } else { self.pairing.off(channel, key) };
        self.key_up_where(
            |o| match o {
                Owner::Note { channel: c, key: k, id: vid, seq } => {
                    if id >= 0 {
                        vid == id
                    } else {
                        c == channel && k == key && closes.is_none_or(|s| s == seq)
                    }
                }
                Owner::Press { .. } => false,
            },
            velocity,
        );
    }

    fn note_on<B: SourceBank + ?Sized>(&mut self, owner: Owner, key: u8, velocity: u8, ctx: &Context, n: usize, sources: &B) -> bool {
        let mut layers = 0;
        let mut started = false;
        if let Some(p) = self.pads.iter().position(|p| p.note == key && p.zone.is_some()) {
            started |= self.trigger_pad(p, owner, key, velocity, ctx, n, sources);
            layers += 1;
        }
        // The groups already admitted for this note (one bit per group id).
        let mut done = [0u64; GROUPS / 64];
        for z in 0..ZONES {
            if layers >= LAYERS_PER_NOTE {
                break;
            }
            let zone = self.zones.get(z).and_then(Option::as_ref).map(|s| &s.zone);
            if let Some(m) = zone.and_then(|z| z.membership) {
                let bit = 1u64 << (m.group % 64);
                if self.bound.get(z).is_some_and(Option::is_none)
                    && zone.is_some_and(|z| (z.keys.0..=z.keys.1).contains(&key))
                    && let Some(word) = done.get_mut(usize::from(m.group) / 64)
                    && *word & bit == 0
                {
                    *word |= bit;
                    started |= self.trigger_group(m.group, &mut layers, owner, key, velocity, ctx, n, sources);
                }
                continue;
            }
            let selected = selection::unbound_selected(self.bound.get(z), zone, key, velocity);
            if selected && self.start_zone(z, None, owner, key, velocity, ctx, n, sources) {
                started = true;
                layers += 1;
            }
        }
        started
    }

    /// Admits key-map group `group`'s velocity layers that hold `velocity`, in ascending layer order
    /// while the note's layer bound allows, each choosing and starting one alternative (the choice
    /// advances even when the start is refused).
    fn trigger_group<B: SourceBank + ?Sized>(
        &mut self,
        group: u8,
        layers: &mut usize,
        owner: Owner,
        key: u8,
        velocity: u8,
        ctx: &Context,
        n: usize,
        sources: &B,
    ) -> bool {
        let mut started = false;
        for layer in 0..LAYERS as u8 {
            if *layers >= LAYERS_PER_NOTE {
                break;
            }
            let Some(&alternatives) = self.members.get(selection::entry(group, layer)) else { continue };
            let count = selection::count(&alternatives);
            let first = alternatives.first().and_then(|&z| self.zone(usize::from(z)));
            if count == 0 || !first.is_some_and(|z| selection::holds(z, key, velocity)) {
                continue;
            }
            let alt = self.rr.choose(group, layer, count);
            let Some(&z) = alternatives.get(usize::from(alt)) else { continue };
            if self.start_zone(usize::from(z), None, owner, key, velocity, ctx, n, sources) {
                started = true;
                *layers += 1;
            }
        }
        started
    }

    fn trigger_pad<B: SourceBank + ?Sized>(&mut self, p: usize, owner: Owner, key: u8, velocity: u8, ctx: &Context, n: usize, sources: &B) -> bool {
        let Some(pad) = self.pads.get(p).copied() else { return false };
        let Some(z) = pad.zone else { return false };
        if !self.admits(p) {
            return false;
        }
        if self.source_for(usize::from(z), sources).is_none() {
            self.diag.refused_starts += 1;
            return false;
        }
        if pad.choke != 0 {
            for q in 0..PADS {
                if q != p
                    && self.pads.get(q).is_some_and(|o| o.choke == pad.choke)
                    && let Some(vi) = self.live_pad_voice(q)
                {
                    self.fade_voice(vi);
                }
            }
        }
        self.start_zone(usize::from(z), Some(p), owner, key, velocity, ctx, n, sources)
    }

    /// The source's channels and rate ratio for zone `z`, if it can serve the zone's region.
    fn source_for<B: SourceBank + ?Sized>(&self, z: usize, sources: &B) -> Option<(u8, f64)> {
        let slot = self.zones.get(z)?.as_ref()?;
        let r = slot.zone.region;
        let s = sources.source(usize::from(r.source))?;
        let channels = s.channels();
        let sr = s.sample_rate();
        ((1..=2).contains(&channels) && s.frames() >= r.end && sr.is_finite() && sr > 0.0)
            .then(|| (channels as u8, f64::from(sr) / f64::from(self.rate)))
    }

    /// Starts a voice on zone `z` (pad `pad`'s, or unbound).
    fn start_zone<B: SourceBank + ?Sized>(
        &mut self,
        z: usize,
        pad: Option<usize>,
        owner: Owner,
        key: u8,
        velocity: u8,
        ctx: &Context,
        n: usize,
        sources: &B,
    ) -> bool {
        let Some((channels, ratio)) = self.source_for(z, sources) else {
            self.diag.refused_starts += 1;
            return false;
        };
        let Some(Some(slot)) = self.zones.get(z) else { return false };
        let (zone, derived) = (slot.zone, slot.derived);
        // Edge fades scale an unbound zone's velocity after selection (`fades.rs`); a pad plays none.
        let velocity = if pad.is_some() { velocity } else { fades::scaled_velocity(zone.fades, zone.keys, zone.velocities, key, velocity) };
        let vi = match pad.and_then(|p| self.live_pad_voice(p)) {
            Some(vi) => vi,
            None => self.allocate(),
        };
        let abs = ctx.frame.wrapping_add(n as u64) as f64;
        let free = core::array::from_fn(|i| {
            let inc = zone.shaper.lfo.get(i).map_or(0.0, |l| l.increment(ctx.tempo, 0.0, zone.shaper.sr));
            let c = abs * inc;
            if c.is_finite() { c - math::floor(c) } else { 0.0 }
        });
        self.serial += 1;
        let st = Start { zone: z as u16, pad: pad.map(|p| p as u8), owner, key, velocity, serial: self.serial, ratio, channels, free };
        let ok = self.voices.get_mut(vi).is_some_and(|v| v.start(&st, &zone, &derived));
        if !ok {
            self.diag.refused_starts += 1;
            return false;
        }
        if let Some(slot) = pad.and_then(|p| self.pad_voice.get_mut(p)) {
            *slot = Some((vi as u8, st.serial));
        }
        true
    }

    /// A slot for a new voice: free in index order, else the earliest release or fade, else the
    /// oldest sounding voice (ties by index), which is cut and counted as a steal.
    fn allocate(&mut self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        let ending = self.voices.iter().enumerate().filter(|(_, v)| v.is_ending()).min_by_key(|(i, v)| (v.released, *i)).map(|(i, _)| i);
        let i = ending.or_else(|| self.voices.iter().enumerate().min_by_key(|(i, v)| (v.serial, *i)).map(|(i, _)| i)).unwrap_or(0);
        if let Some(v) = self.voices.get_mut(i) {
            v.cut();
        }
        self.diag.steals += 1;
        i
    }

    /// The state fingerprint (voices, controllers, pad voices, render order): equal for equal
    /// schedules from reset, whatever the block sizes (Gate 6).
    #[cfg(any(test, feature = "testing"))]
    pub fn fingerprint(&self, h: &mut dyn core::hash::Hasher) {
        for v in &self.voices {
            v.fingerprint(h);
        }
        let c = &self.controls;
        for x in [c.wheel, c.bend, c.aftertouch].iter().chain(&c.poly).chain(&c.macros) {
            h.write_u32(x.to_bits());
        }
        h.write_u8(u8::from(c.sustain));
        for p in &self.pad_voice {
            let (vi, s) = p.unwrap_or((u8::MAX, 0));
            h.write_u8(vi);
            h.write_u64(s);
        }
        for o in self.order.iter().take(self.order_len) {
            h.write_u8(*o);
        }
        h.write_u64(self.serial);
        // The round robin's counters and random states (not the last block's transport state, which
        // the caller's context sets again each block).
        self.rr.fingerprint(h);
    }
}
