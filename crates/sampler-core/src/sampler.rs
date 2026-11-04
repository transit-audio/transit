//! [`SamplerCore`]: the patch, the voice pool, event handling and the block render.
//!
//! **Admission.** A note-on triggers the pad whose trigger note it is and then the unbound zones whose
//! inclusive key and velocity ranges hold it, in zone order, at most [`crate::LAYERS_PER_NOTE`] voices (the
//! pad counts as one; of a round-robin group only alternative 0 plays until #270). A muted pad, or a
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
use crate::patch::{self, Derived, Pad, PatchError, Zone};
use crate::reader::SourceBank;
use crate::voice::{Controls, Owner, Start, Transport, Voice};
use crate::{LAYERS_PER_NOTE, PADS, VOICES, ZONES, math};

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
        }
    }

    /// Prepares for `sample_rate` (Hz): allocates the voices and the zone settings (once), works the
    /// zones' durations out at the rate, seeds the voices' random state from `seed`, and resets. The
    /// zones' shaper settings must be built at this rate.
    pub fn prepare(&mut self, sample_rate: f32, seed: u32) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        if self.zones.len() != ZONES {
            self.zones = (0..ZONES).map(|_| None).collect();
        }
        if self.voices.len() != VOICES || self.seed != seed {
            self.voices =
                (0..VOICES as u32).map(|i| Voice::new(seed.wrapping_mul(0x9E37_79B9) ^ i.wrapping_add(1).wrapping_mul(0x85EB_CA6B))).collect();
        }
        self.seed = seed;
        let rate = self.rate;
        for s in self.zones.iter_mut().flatten() {
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
    /// region changes. A zone bound to a pad cannot be cleared or given membership.
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
        let old = self.zones.get(index).and_then(|s| s.as_ref().map(|s| s.zone.region));
        if old != zone.map(|z| z.region) {
            self.fade_where(|v| usize::from(v.zone) == index);
        }
        let rate = self.rate;
        if let Some(slot) = self.zones.get_mut(index) {
            *slot = zone.map(|zone| Slot { derived: Derived::of(&zone.playback, rate), zone });
        }
        Ok(())
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

    /// Every voice cut, controllers cleared, the voices' random state back to its seed.
    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.reset();
        }
        self.controls = Controls::new();
        self.pad_voice = [None; PADS];
        self.order_len = 0;
        self.serial = 0;
    }

    /// Renders one block into `out` (1 or 2 channels; the shortest sets the block's length; a mono
    /// output takes the average of left and right), acting on `events` at their frames.
    pub fn process<B: SourceBank + ?Sized>(&mut self, ctx: &Context, sources: &B, events: &[Event], out: &mut [&mut [f32]]) {
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
        let SamplerCore { voices, zones, controls, order, order_len, rate, .. } = self;
        let per_frame = ctx.tempo / 60.0 / f64::from(*rate);
        for &vi in order.iter().take(*order_len) {
            let Some(v) = voices.get_mut(usize::from(vi)) else { continue };
            if !v.active {
                continue;
            }
            let Some(Some(slot)) = zones.get(usize::from(v.zone)) else {
                v.cut();
                continue;
            };
            let source = sources.source(usize::from(v.source()));
            for n in from..to {
                if !v.active {
                    break;
                }
                let t = Transport { tempo: ctx.tempo, quarters: ctx.quarters.map(|q| q + n as f64 * per_frame) };
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
                    self.note_on(Owner::Note { channel, key, id }, key, velocity, ctx, n, sources)
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
                self.voices.iter_mut().for_each(Voice::cut);
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

    fn note_off(&mut self, channel: u8, key: u8, velocity: u8, id: i32) {
        self.key_up_where(
            |o| match o {
                Owner::Note { channel: c, key: k, id: vid } => {
                    if id >= 0 {
                        vid == id
                    } else {
                        c == channel && k == key
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
        for z in 0..ZONES {
            if layers >= LAYERS_PER_NOTE {
                break;
            }
            let selected = self.bound.get(z).is_some_and(Option::is_none)
                && self.zones.get(z).and_then(Option::as_ref).is_some_and(|s| {
                    let zone = &s.zone;
                    (zone.keys.0..=zone.keys.1).contains(&key)
                        && (zone.velocities.0..=zone.velocities.1).contains(&velocity)
                        && zone.membership.is_none_or(|m| m.alternative == 0)
                });
            if selected && self.start_zone(z, None, owner, key, velocity, ctx, n, sources) {
                started = true;
                layers += 1;
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
    }
}
