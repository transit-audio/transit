//! The plan compiler's side of the built-in Sampler's patch (ADR 0041, job C2).
//!
//! An instrument slot whose [`Insert::sampler_patch`] is present is a core-mode Sampler: its
//! [`InstanceSpec::sampler_core`](crate::InstanceSpec::sampler_core) carries the patch's seed, so
//! making or removing a patch, or a new seed, is a fresh instance (the plan never switches a
//! running one). The compiler owns one [`Link`] per such instance, keyed by the insert's uid (as
//! the editor windows are) and checked against the instance's [`SlotGen`]:
//!
//! - **Fresh instance** (control side, before it first processes): [`Links::attach`] switches it
//!   with `Plugin::enable_sampler_core`, keeps the returned `Publisher`, sets every saved control
//!   with `set_param` and publishes the structure. Setting the controls before the first
//!   publication is adopted gives the same zones as setting them after: adoption composes every
//!   zone from the slot table.
//! - **Running instance**: each compile ([`Links::sync`]) publishes the structure again only when
//!   it changed (sources, zones, pads), and sends the controls that changed through the
//!   publisher's control lane (a removed entry sends its default).
//! - **Service tick** ([`Links::service`], from the plan bridge's poll and every compile): collects
//!   retired generations on this thread and retries a publication that waited for a credit.
//!
//! A zone whose source is missing from the pool, disk-backed, or not mono or stereo, or whose
//! region runs past its source, is left out of the publication (its pad unbound there), so it plays
//! silence while the patch keeps its reference. A key-map group member in that state is published
//! instead as a silent placeholder ([`sampler::SILENT_SOURCE`]): ADR 0041 selects it like the other
//! alternatives, so the round robin keeps its order and relinking does not reorder it. An opaque
//! patch publishes nothing and plays silence.

use super::*;
use crate::compile::PoolSample;
use std::collections::{BTreeMap, HashMap};
use transit_dsp::plugins::sampler::{
    self as sampler, LoopMode as CoreLoop, Membership as CoreMembership, Pad, PatchSpec, PlayMode as CorePlay, PublishError, Publisher, Region, Zone,
};
use transit_model::Insert;
use transit_model::sampler_patch::{LoopMode, PadEntry, PatchV1, PlayMode, ZoneEntry};

/// What a publication carries, compared to decide whether a running instance needs a new one: the
/// identity of each source's audio (0: unusable), the zones and the pads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Structure {
    sources: Vec<usize>,
    zones: Vec<ZoneEntry>,
    pads: Vec<PadEntry>,
}

/// One core-mode instance's control side.
pub(crate) struct Link {
    generation: SlotGen,
    publisher: Publisher,
    /// The structure of the last publication (`None`: none yet).
    published: Option<Structure>,
    /// A structure the instance refused (not retried until it changes).
    refused: Option<Structure>,
    /// A publication waiting for a credit (retried at the service tick).
    waiting: Option<(Structure, PatchSpec)>,
    /// The controls the instance holds (as last sent).
    controls: BTreeMap<String, f32>,
    /// The wrapper handed out for each pool audio, so an unchanged sample keeps its identity (and
    /// its source id) across publications.
    wrappers: HashMap<usize, Arc<dyn SampleSource>>,
}

/// Every core-mode instance's link, by insert uid.
#[derive(Default)]
pub(crate) struct Links {
    map: HashMap<u64, Link>,
}

fn audio_id(a: &Arc<SourceAudio>) -> usize {
    Arc::as_ptr(a) as *const () as usize
}

/// The pool audio a source reference plays, when a Sampler zone can read it from memory.
fn usable(s: &Session, source: transit_model::SourceId) -> Option<&Arc<SourceAudio>> {
    s.pool.get(source).filter(|a| !a.is_disk() && (1..=2).contains(&a.buffer.num_channels()))
}

/// Frames every channel of `a` the Sampler reads holds.
fn readable(a: &SourceAudio) -> u64 {
    a.buffer.channels.iter().take(2).map(Vec::len).min().unwrap_or(0) as u64
}

/// The structure of `p` in `s`.
pub(crate) fn structure(p: &PatchV1, s: &Session) -> Structure {
    Structure {
        sources: p.sources.iter().map(|r| usable(s, r.source).map_or(0, audio_id)).collect(),
        zones: p.zones.clone(),
        pads: p.pads.clone(),
    }
}

fn core_zone(z: &ZoneEntry, source: u16) -> Zone {
    let loop_mode = match z.loop_mode {
        LoopMode::Off => CoreLoop::Off,
        LoopMode::Forward => CoreLoop::Forward,
        LoopMode::PingPong => CoreLoop::PingPong,
    };
    let mut zone = Zone::new(Region { source, start: z.start, end: z.end, loop_mode, loop_start: z.loop_start, loop_end: z.loop_end });
    zone.keys = (z.keys[0], z.keys[1]);
    zone.velocities = (z.velocities[0], z.velocities[1]);
    zone.membership = z.membership.as_ref().map(|m| CoreMembership { group: m.group, layer: m.layer, alternative: m.alternative });
    let p = &mut zone.playback;
    p.transpose = z.transpose;
    p.cents = z.cents;
    p.reverse = z.reverse;
    p.mode = if z.mode == PlayMode::OneShot { CorePlay::OneShot } else { CorePlay::Gate };
    p.key_tracking = z.key_tracking;
    p.root = z.root;
    p.attack_ms = z.attack_ms;
    p.decay_ms = z.decay_ms;
    p.sustain = z.sustain;
    p.release_ms = z.release_ms;
    p.declick_ms = z.declick_ms;
    p.velocity_sensitivity = z.velocity_sensitivity;
    zone
}

/// The publication of `p` in `s`: its usable sources (wrapped through `wrappers`), the zones that
/// can play and the group members that cannot as silent placeholders, the 64 pads (a pad of a zone
/// left out is unbound here). Also the ids of the silent zones
/// (missing, disk-backed or unusable media, or a region past the source).
pub(crate) fn spec_of(p: &PatchV1, s: &Session, wrappers: &mut HashMap<usize, Arc<dyn SampleSource>>) -> (PatchSpec, Vec<u16>) {
    let mut spec = PatchSpec::default();
    let mut local: Vec<Option<(u16, u64)>> = Vec::with_capacity(p.sources.len());
    let mut keep: HashMap<usize, Arc<dyn SampleSource>> = HashMap::new();
    for r in &p.sources {
        let entry = usable(s, r.source).and_then(|a| {
            let id = audio_id(a);
            let w = wrappers.get(&id).cloned().unwrap_or_else(|| Arc::new(PoolSample(Arc::clone(a))) as Arc<dyn SampleSource>);
            keep.insert(id, Arc::clone(&w));
            let index = u16::try_from(spec.sources.len()).ok()?;
            spec.sources.push(w);
            Some((index, readable(a)))
        });
        local.push(entry);
    }
    // Only the wrappers this publication uses stay cached; the generations hold their own.
    *wrappers = keep;
    spec.zones = vec![None; sampler::ZONES];
    let mut silent = Vec::new();
    for z in &p.zones {
        match local.get(usize::from(z.source)).copied().flatten() {
            Some((index, frames)) if z.end <= frames => {
                if let Some(slot) = spec.zones.get_mut(usize::from(z.id)) {
                    *slot = Some(core_zone(z, index));
                }
            }
            _ => {
                silent.push(z.id);
                if z.membership.is_some()
                    && let Some(slot) = spec.zones.get_mut(usize::from(z.id))
                {
                    *slot = Some(core_zone(z, sampler::SILENT_SOURCE));
                }
            }
        }
    }
    spec.pads = core::array::from_fn(|i| {
        let e = p.pad(i as u8);
        let playable = |z: &u16| spec.zones.get(usize::from(*z)).and_then(Option::as_ref).is_some_and(|z| z.region.source != sampler::SILENT_SOURCE);
        let zone = e.zone.filter(playable);
        Pad { zone, note: e.trigger(), mute: e.mute, solo: e.solo, choke: e.choke }
    });
    (spec, silent)
}

/// Checks `p` in `s` as the instance will check its publication (the model's own rules first,
/// then the core's), for a command to refuse an edit before it changes the document.
pub fn check_sampler_patch(p: &PatchV1, s: &Session) -> Result<(), String> {
    p.validate().map_err(|e| e.0)?;
    // A zone whose audio is there must fit in it; missing audio is allowed (it plays silence until
    // relinked).
    for z in &p.zones {
        let audio = p.sources.get(usize::from(z.source)).and_then(|r| usable(s, r.source));
        if audio.is_some_and(|a| z.end > readable(a)) {
            return Err(format!("zone {}: its region ends past its audio ({} frames)", z.id + 1, audio.map_or(0, |a| readable(a))));
        }
    }
    let (spec, _) = spec_of(p, s, &mut HashMap::new());
    sampler::check_spec(&spec).map_err(|e| format!("the Sampler refuses the patch: {e:?}"))
}

/// The zones of `p` that play silence in `s` (missing or unusable media, or a region past it).
pub fn silent_sampler_zones(p: &PatchV1, s: &Session) -> Vec<u16> {
    spec_of(p, s, &mut HashMap::new()).1
}

/// The default of slot control `id` (`None`: not a slot parameter this build knows).
fn control_default(id: &str) -> Option<f32> {
    let (_, field) = sampler::resolve(id)?;
    sampler::field_info(field).map(|i| i.default)
}

impl Link {
    fn publish(&mut self, st: Structure, spec: PatchSpec) {
        if !self.publisher.has_credit() {
            self.publisher.collect();
        }
        if !self.publisher.has_credit() {
            self.waiting = Some((st, spec));
            return;
        }
        self.waiting = None;
        match self.publisher.publish(spec) {
            Ok(()) => self.published = Some(st),
            Err(PublishError::NoCredit) => {}
            Err(e) => {
                log::warn!("sampler: a patch publication was refused ({e:?}); the old patch keeps playing");
                self.refused = Some(st);
            }
        }
    }

    fn send_controls(&mut self, controls: &BTreeMap<String, f32>) {
        for (id, v) in controls {
            if self.controls.get(id) != Some(v) {
                self.publisher.set_control(id, *v);
            }
        }
        for id in self.controls.keys().filter(|id| !controls.contains_key(*id)) {
            if let Some(d) = control_default(id) {
                self.publisher.set_control(id, d);
            }
        }
        self.controls.clone_from(controls);
    }
}

impl Links {
    /// Switches a fresh instance made for `ins` (seed `seed`) to the core path before it first
    /// processes, sets its saved controls and publishes its structure. Control side.
    pub(crate) fn attach(&mut self, plugin: &mut dyn Plugin, seed: u32, generation: SlotGen, uid: u64, ins: &Insert, s: &Session) {
        let Some(publisher) = plugin.enable_sampler_core(seed) else { return };
        let mut link =
            Link { generation, publisher, published: None, refused: None, waiting: None, controls: BTreeMap::new(), wrappers: HashMap::new() };
        if let Some(p) = ins.sampler_patch.as_ref().and_then(transit_model::SamplerPatch::ready) {
            for (id, v) in &p.controls {
                plugin.set_param(id, *v);
            }
            link.controls.clone_from(&p.controls);
            let st = structure(p, s);
            let (spec, _) = spec_of(p, s, &mut link.wrappers);
            link.publish(st, spec);
        }
        self.map.insert(uid, link);
    }

    /// Brings the running instance `generation` of insert `uid` up to `ins`'s patch: a new
    /// publication when the structure changed, the changed controls through the lane.
    pub(crate) fn sync(&mut self, uid: u64, generation: SlotGen, ins: &Insert, s: &Session) {
        let Some(link) = self.map.get_mut(&uid).filter(|l| l.generation == generation) else { return };
        let Some(p) = ins.sampler_patch.as_ref().and_then(transit_model::SamplerPatch::ready) else { return };
        let st = structure(p, s);
        let pending = link.waiting.as_ref().map(|(w, _)| w);
        if link.published.as_ref() != Some(&st) && link.refused.as_ref() != Some(&st) && pending != Some(&st) {
            let (spec, _) = spec_of(p, s, &mut link.wrappers);
            link.publish(st, spec);
        }
        link.send_controls(&p.controls);
    }

    /// The service tick: collects retired generations here and sends what waited.
    pub(crate) fn service(&mut self) {
        for link in self.map.values_mut() {
            link.publisher.collect();
            if let Some((st, spec)) = link.waiting.take() {
                link.publish(st, spec);
            }
        }
    }

    /// Forgets the links of instances no longer in `live` (by uid and generation).
    pub(crate) fn retain(&mut self, live: &[(u64, SlotGen)]) {
        self.map.retain(|uid, l| live.contains(&(*uid, l.generation)));
    }

    /// The link count (tests).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}
