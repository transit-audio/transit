//! The Filter's modulation sources (#242): the paraphonic note state that gates the envelopes and
//! the envelope follower. The ADSR is the built-in instruments' own
//! ([`crate::plugins::instrument_queue::Adsr`]); the LFO is shared ([`crate::plugins::lfo`]).

use crate::util::time_coef;

/// Notes the paraphonic gate tracks at once (more than the mix holds open, 224).
pub(crate) const MAX_HELD: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    used: bool,
    channel: u8,
    key: u8,
    /// The mix's voice id, -1 when the note has none (matched by channel and key then).
    voice: i32,
    /// Released while the sustain pedal was down: it holds the gate until the pedal comes up.
    sustained: bool,
}

const FREE: Held = Held { used: false, channel: 0, key: 0, voice: -1, sustained: false };

/// What a note event did to the paraphonic gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Nothing the envelopes need to hear.
    Unchanged,
    /// A note-on that (re)starts the envelopes, at this velocity.
    Trigger(u8),
    /// The last note let go: the envelopes release.
    Release,
}

/// The paraphonic gate: one gate for every held note. It is open while any note is held (or
/// sustained by the pedal); in Every note mode each note-on retriggers, in Legato only a note-on
/// with nothing held does. Key tracking follows the last note played.
#[derive(Debug, Clone)]
pub(crate) struct Notes {
    held: [Held; MAX_HELD],
    count: usize,
    sustain: bool,
    /// The last note-on's key (middle C, no key tracking offset, before the first note).
    pub(crate) last_key: u8,
}

impl Notes {
    pub(crate) fn new() -> Notes {
        Notes { held: [FREE; MAX_HELD], count: 0, sustain: false, last_key: 60 }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.count > 0
    }

    fn matches(h: &Held, channel: u8, key: u8, voice: i32) -> bool {
        h.used && !h.sustained && h.channel == channel && if voice >= 0 { h.voice == voice } else { h.key == key }
    }

    pub(crate) fn on(&mut self, channel: u8, key: u8, velocity: u8, voice: i32, legato: bool) -> Gate {
        if velocity == 0 {
            return self.off(channel, key, voice);
        }
        let was_open = self.is_open();
        self.last_key = key;
        // A re-strike of a held key (same voice, or same key without voice ids) keeps one entry.
        if !self.held.iter().any(|h| Self::matches(h, channel, key, voice))
            && let Some(h) = self.held.iter_mut().find(|h| !h.used)
        {
            *h = Held { used: true, channel, key, voice, sustained: false };
            self.count += 1;
        }
        // A note the full table could not hold still triggers; its release finds nothing, and the
        // gate closes with the others (or at an all-notes-off).
        if legato && was_open { Gate::Unchanged } else { Gate::Trigger(velocity) }
    }

    pub(crate) fn off(&mut self, channel: u8, key: u8, voice: i32) -> Gate {
        let sustain = self.sustain;
        let Some(h) = self.held.iter_mut().find(|h| Self::matches(h, channel, key, voice)) else { return Gate::Unchanged };
        if sustain {
            h.sustained = true;
            return Gate::Unchanged;
        }
        *h = FREE;
        self.count = self.count.saturating_sub(1);
        if self.is_open() { Gate::Unchanged } else { Gate::Release }
    }

    pub(crate) fn pedal(&mut self, down: bool) -> Gate {
        self.sustain = down;
        if down {
            return Gate::Unchanged;
        }
        let before = self.count;
        for h in self.held.iter_mut().filter(|h| h.used && h.sustained) {
            *h = FREE;
            self.count = self.count.saturating_sub(1);
        }
        if before > 0 && !self.is_open() { Gate::Release } else { Gate::Unchanged }
    }

    /// Every note let go (an all-notes-off, a skipped block, a reset). The pedal stays as it is.
    pub(crate) fn all_off(&mut self) -> Gate {
        let was_open = self.is_open();
        self.held = [FREE; MAX_HELD];
        self.count = 0;
        if was_open { Gate::Release } else { Gate::Unchanged }
    }

    pub(crate) fn reset(&mut self) {
        *self = Notes::new();
    }
}

/// The envelope follower: the input's peak across channels through a one-pole attack and release.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Follower {
    pub(crate) level: f32,
    attack: f32,
    release: f32,
}

impl Follower {
    pub(crate) fn set_times(&mut self, attack_ms: f32, release_ms: f32, sr: f32) {
        self.attack = time_coef(attack_ms, sr);
        self.release = time_coef(release_ms, sr);
    }

    #[inline]
    pub(crate) fn tick(&mut self, peak: f32) {
        let x = if peak.is_finite() { peak.min(4.0) } else { 0.0 };
        let c = if x > self.level { self.attack } else { self.release };
        self.level = crate::util::flush(x + (self.level - x) * c);
    }

    /// 0..1 for the cutoff (a full-scale input reads 1).
    pub(crate) fn value(&self) -> f32 {
        self.level.min(1.0)
    }
}
