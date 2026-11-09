//! The patch handoff between the control side and the audio thread (ADR 0041 "Capacities and
//! admission", "Patch handoff"): two publication slots, two retirement slots, at most one pending
//! publication and four bank-generation credits.
//!
//! - **Publication.** The control side builds a [`Generation`] (the [`PreparedBank`] of `Arc`
//!   samples and the checked zones and pads) off the audio thread and sends it through a queue of
//!   one entry: the audio thread plays the current generation (the first publication slot) and at
//!   most one waits in the queue (the second). Edits made while one waits are coalesced on the
//!   control side into one waiting generation, sent when the queue empties ([`Publisher::collect`]).
//! - **Adoption.** At a block boundary the audio thread takes the waiting generation only when it
//!   can retire the old one, that is when one of its two retained slots is free; the old current
//!   moves there. Otherwise the publication waits and the old patch keeps playing.
//! - **Retirement.** After each block a retained generation whose sources no sounding voice still
//!   reads (other than through the current or another retained generation, which share the ids)
//!   goes back through the retirement queue of two entries. A full queue keeps it retained, and so
//!   blocks further adoption until the control side collects. The control side drops retired
//!   generations, so the last `Arc` of a sample is released there, never on the audio thread.
//! - **Credits.** A generation holds one of [`CREDITS`] from preparation until the control side
//!   collects it (waiting, pending, current, retained or in the retirement queue). A fifth
//!   preparation is refused ([`PublishError::NoCredit`]) and waits on the control side; replacing
//!   a waiting generation reuses its credit.
//!
//! - **Controls.** Slot parameter values set on the control side ([`Publisher::set_control`])
//!   travel through a third ring of [`CONTROL_RING`] numeric entries, drained at the next block
//!   start into the slot table exactly as `set_param` stores them; while the ring is full they
//!   wait on the control side, in order, and go at [`Publisher::collect`].
//!
//! The queues are `rtrb` rings allocated with the channel; moving a `Box` or a numeric entry
//! through them neither allocates nor frees.

use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use transit_instrument_core::matrix::Matrix;
use transit_instrument_core::shaper::Settings;
use transit_sampler_core::patch::{GroupMode, Pad, PatchError, Zone, check_patch};
use transit_sampler_core::{GROUPS, PADS, ZONES};

use super::bank::{CREDITS, PreparedBank, SOURCE_IDS, readable_frames};
use super::params::resolve;
use crate::SampleSource;

/// Control entries the ring between the control side and the audio thread holds.
pub const CONTROL_RING: usize = 4096;

/// One slot parameter value: zone, field, value (clamped on the audio side, as `set_param` does).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ControlEdit {
    pub(super) zone: u16,
    pub(super) field: u16,
    pub(super) value: f32,
}

/// The source index of a silent placeholder zone: a zone whose media is missing or unusable but
/// which keeps its place in the patch (a round-robin alternative, which ADR 0041 selects even
/// while its media is missing, so relinking does not reorder the others). The handoff publishes it
/// without a source; the core selects it like any zone, refuses its start (counted in
/// `refused_starts`) and plays nothing.
pub const SILENT_SOURCE: u16 = u16::MAX;

/// What the control side asks the core path to play.
pub struct PatchSpec {
    /// The samples the zones play; a zone's `region.source` indexes this list (at most [`ZONES`]
    /// entries; an entry no zone uses takes no source id), or is [`SILENT_SOURCE`].
    pub sources: Vec<Arc<dyn SampleSource>>,
    /// Zone slots from slot 0 (at most [`ZONES`]; missing slots are empty). A zone's
    /// `playback.level_db`, `playback.pan`, `shaper` and `matrix` are not read: the zone slot's
    /// parameters set them (`zoneNNN_level`, `zoneNNN_pan`, the shaper and `mod` ids).
    pub zones: Vec<Option<Zone>>,
    pub pads: [Pad; PADS],
    /// Each key-map group's round-robin mode, by group id (default: every group Sequential).
    pub groups: [GroupMode; GROUPS],
}

impl Default for PatchSpec {
    fn default() -> Self {
        PatchSpec { sources: Vec::new(), zones: Vec::new(), pads: core::array::from_fn(Pad::new), groups: [GroupMode::Sequential; GROUPS] }
    }
}

/// Why a publication was refused (nothing was published; the old patch keeps playing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishError {
    /// More than [`ZONES`] sources or zones.
    TooMany,
    /// Zone `zone` names a source that is not in the list.
    Source { zone: usize },
    /// Zone `zone`'s source is not mono or stereo (convert it first).
    Channels { zone: usize },
    /// Zone `zone`'s region runs past its source's end.
    Region { zone: usize },
    /// Zone `zone`'s region runs past the shorter channel of its stereo source (whose channels
    /// differ in length; the shorter one is what plays).
    ShortChannel { zone: usize },
    /// The core refuses the zones or pads.
    Patch(PatchError),
    /// Every bank-generation credit is in use: collect retired generations first.
    NoCredit,
}

/// One prepared patch generation.
pub(super) struct Generation {
    pub(super) bank: PreparedBank,
    /// [`ZONES`] entries, sources by id.
    pub(super) zones: Vec<Option<Zone>>,
    pub(super) pads: [Pad; PADS],
    pub(super) groups: [GroupMode; GROUPS],
}

impl Generation {
    /// Whether this generation holds source id `id`.
    pub(super) fn holds(&self, id: u16) -> bool {
        self.bank.holds(usize::from(id))
    }
}

/// The audio thread's ends of the two queues.
pub(super) struct AudioEnd {
    pub(super) publications: Consumer<Box<Generation>>,
    pub(super) retirements: Producer<Box<Generation>>,
    pub(super) controls: Consumer<ControlEdit>,
}

impl AudioEnd {
    /// Sends a retired generation back; gives it back when the queue is full.
    pub(super) fn retire(&mut self, g: Box<Generation>) -> Result<(), Box<Generation>> {
        self.retirements.push(g).map_err(|PushError::Full(g)| g)
    }
}

/// A source id the control side has handed out: the sample and how many live generations hold it.
struct Held {
    sample: Arc<dyn SampleSource>,
    generations: u32,
}

/// The control side of the handoff (allocates; never on the audio thread).
pub struct Publisher {
    publications: Producer<Box<Generation>>,
    retirements: Consumer<Box<Generation>>,
    ids: Vec<Option<Held>>,
    /// Generations holding a credit.
    live: usize,
    /// The newest prepared generation, not yet sent (coalesced edits).
    waiting: Option<Box<Generation>>,
    controls: Producer<ControlEdit>,
    /// Control values the full ring could not take yet, oldest first.
    backlog: std::collections::VecDeque<ControlEdit>,
}

/// The two ends of a new handoff.
pub(super) fn channel() -> (Publisher, AudioEnd) {
    let (publish, publications) = RingBuffer::new(1);
    let (retire, retirements) = RingBuffer::new(2);
    let (control, controls) = RingBuffer::new(CONTROL_RING);
    let publisher = Publisher {
        publications: publish,
        retirements,
        ids: (0..SOURCE_IDS).map(|_| None).collect(),
        live: 0,
        waiting: None,
        controls: control,
        backlog: std::collections::VecDeque::new(),
    };
    (publisher, AudioEnd { publications, retirements: retire, controls })
}

/// Identity of a shared sample (its data pointer; the vtable is not compared).
fn identity(s: &Arc<dyn SampleSource>) -> *const () {
    Arc::as_ptr(s).cast::<()>()
}

impl Publisher {
    /// Prepares a generation from `spec`, checks it whole, and sends it (or, while a publication
    /// is pending, keeps it as the waiting one, replacing an older waiting one).
    pub fn publish(&mut self, spec: PatchSpec) -> Result<(), PublishError> {
        if self.live >= CREDITS && self.waiting.is_none() {
            return Err(PublishError::NoCredit);
        }
        let zones = check(&spec)?;
        // A waiting generation is replaced: its credit and its ids go to the new one first, so the
        // ids of at most four generations are ever held.
        let reused = self.waiting.take().map(|old| self.release(old)).is_some();
        let mut g = Box::new(Generation { bank: PreparedBank::new(), zones, pads: spec.pads, groups: spec.groups });
        let mut local: Vec<Option<u16>> = vec![None; spec.sources.len()];
        let mut failed = false;
        for z in g.zones.iter_mut().flatten().filter(|z| z.region.source != SILENT_SOURCE) {
            let li = usize::from(z.region.source);
            let id = match (local.get(li).copied().flatten(), spec.sources.get(li)) {
                (Some(id), _) => Some(id),
                (None, Some(sample)) => {
                    // A sample listed twice takes one id and one holder count (the generation
                    // releases each of its ids once).
                    let twin = spec.sources.iter().zip(&local).find_map(|(s, l)| l.filter(|_| identity(s) == identity(sample)));
                    let id = twin.or_else(|| {
                        let id = self.take_id(sample)?;
                        g.bank.insert(id, Arc::clone(sample));
                        Some(id)
                    });
                    if let (Some(slot), Some(id)) = (local.get_mut(li), id) {
                        *slot = Some(id);
                    }
                    id
                }
                (None, None) => None,
            };
            match id {
                Some(id) => z.region.source = id,
                None => failed = true,
            }
        }
        if failed {
            // Not reachable within the credits (four generations of at most ZONES ids each).
            self.release(g);
            if reused {
                self.live -= 1;
            }
            return Err(PublishError::NoCredit);
        }
        if !reused {
            self.live += 1;
        }
        self.waiting = Some(g);
        self.flush();
        Ok(())
    }

    /// Drops the generations the audio thread retired (here, on the control side) and returns
    /// their credits; then sends the waiting generation if the queue has room. Returns how many
    /// were collected.
    pub fn collect(&mut self) -> usize {
        let mut n = 0;
        while let Ok(g) = self.retirements.pop() {
            self.release(g);
            self.live -= 1;
            n += 1;
        }
        self.flush();
        self.flush_controls();
        n
    }

    /// Whether a publication now would get a credit (collect first when it would not).
    #[must_use]
    pub fn has_credit(&self) -> bool {
        self.live < CREDITS || self.waiting.is_some()
    }

    /// Sends slot parameter `id`'s new value (`zone001_level`, ...) to the instance, which stores
    /// it at its next block start as `set_param` would. `false` for an id that is not a slot
    /// parameter. Values the full ring cannot take wait here, in order (see [`Publisher::collect`]).
    pub fn set_control(&mut self, id: &str, value: f32) -> bool {
        let Some((zone, field)) = resolve(id) else { return false };
        let (Ok(zone), Ok(field)) = (u16::try_from(zone), u16::try_from(field)) else { return false };
        self.backlog.push_back(ControlEdit { zone, field, value });
        self.flush_controls();
        true
    }

    /// Source ids handed out and still held by a live generation.
    #[cfg(test)]
    pub(super) fn held_ids(&self) -> usize {
        self.ids.iter().flatten().count()
    }

    /// Control values still waiting for room in the ring.
    #[must_use]
    pub fn controls_waiting(&self) -> usize {
        self.backlog.len()
    }

    fn flush_controls(&mut self) {
        while let Some(&c) = self.backlog.front() {
            if self.controls.push(c).is_err() {
                return;
            }
            self.backlog.pop_front();
        }
    }

    /// Credits in use (at most [`CREDITS`]).
    #[must_use]
    pub fn credits_in_use(&self) -> usize {
        self.live
    }

    /// A publication sent and not yet adopted.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.publications.is_full()
    }

    /// A prepared generation not yet sent (it goes when the pending one is adopted).
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }

    fn flush(&mut self) {
        if !self.publications.is_full()
            && let Some(g) = self.waiting.take()
            && let Err(PushError::Full(g)) = self.publications.push(g)
        {
            self.waiting = Some(g);
        }
    }

    /// The id for `sample`: the one a live generation already uses for it, else a free one.
    fn take_id(&mut self, sample: &Arc<dyn SampleSource>) -> Option<u16> {
        let same = self.ids.iter().position(|h| h.as_ref().is_some_and(|h| identity(&h.sample) == identity(sample)));
        let at = same.or_else(|| self.ids.iter().position(Option::is_none))?;
        let slot = self.ids.get_mut(at)?;
        match slot {
            Some(h) => h.generations += 1,
            None => *slot = Some(Held { sample: Arc::clone(sample), generations: 1 }),
        }
        u16::try_from(at).ok()
    }

    /// Lets a generation go: its ids lose a holder (a free id forgets its sample) and it drops here.
    fn release(&mut self, g: Box<Generation>) {
        for id in g.bank.ids() {
            if let Some(slot) = self.ids.get_mut(usize::from(id)) {
                let free = slot.as_mut().is_some_and(|h| {
                    h.generations = h.generations.saturating_sub(1);
                    h.generations == 0
                });
                if free {
                    *slot = None;
                }
            }
        }
        drop(g);
    }
}

/// Checks a spec before anything is prepared and returns its [`ZONES`] zones, sources still by the
/// spec's indices: counts, each zone's source (present, mono or stereo, long enough for its
/// region; a [`SILENT_SOURCE`] placeholder has none to check), then the zones and pads whole as
/// the core will check them on adoption.
pub(super) fn check(spec: &PatchSpec) -> Result<Vec<Option<Zone>>, PublishError> {
    if spec.sources.len() > ZONES || spec.zones.len() > ZONES {
        return Err(PublishError::TooMany);
    }
    let mut zones: Vec<Option<Zone>> = spec.zones.iter().copied().chain(std::iter::repeat(None)).take(ZONES).collect();
    for (i, z) in zones.iter_mut().enumerate() {
        let Some(z) = z else { continue };
        if z.region.source != SILENT_SOURCE {
            let s = spec.sources.get(usize::from(z.region.source)).ok_or(PublishError::Source { zone: i })?;
            if !(1..=2).contains(&s.num_channels()) {
                return Err(PublishError::Channels { zone: i });
            }
            if readable_frames(s.as_ref()) < z.region.end {
                // The shorter channel of a stereo source is what plays: a region past it is
                // refused, and named as such when the source's own length would have held it.
                return Err(if (s.frames() as u64) < z.region.end {
                    PublishError::Region { zone: i }
                } else {
                    PublishError::ShortChannel { zone: i }
                });
            }
        }
        // The controls are the slot parameters', always in range: check the structure.
        z.playback.level_db = 0.0;
        z.playback.pan = 0.0;
        z.shaper = Settings::default();
        z.matrix = Matrix::default();
    }
    check_patch(&zones, &spec.pads).map_err(PublishError::Patch)?;
    Ok(zones)
}
