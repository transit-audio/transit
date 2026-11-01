//! One immutable bank per plan: its generation and slot keys never change.
use std::sync::atomic::{AtomicU8, Ordering};
use transit_mix::{INSTRUMENT_SLOT, MixEngine, ParameterDelivery, PlanGeneration};
use transit_model::{Session, TrackId};

pub(super) struct Bank {
    generation: PlanGeneration,
    slots: Vec<((TrackId, usize), AtomicU8)>,
}

impl Bank {
    pub(super) fn bytes(session: &Session) -> usize {
        session
            .tracks
            .iter()
            .map(|t| t.mixer.inserts.len() + usize::from(t.instrument.is_some()))
            .sum::<usize>()
            .saturating_mul(size_of::<((TrackId, usize), AtomicU8)>())
    }

    pub(super) fn new(session: &Session, generation: PlanGeneration) -> Self {
        let mut slots = Vec::with_capacity(Self::bytes(session) / size_of::<((TrackId, usize), AtomicU8)>());
        for t in &session.tracks {
            for (i, ins) in t.mixer.inserts.iter().enumerate() {
                if ins.as_ref().is_some_and(|ins| ins.hardware.is_none()) {
                    slots.push(((t.id, i), AtomicU8::new(0)));
                }
            }
            if t.instrument.is_some() {
                slots.push(((t.id, INSTRUMENT_SLOT), AtomicU8::new(0)));
            }
        }
        slots.sort_unstable_by_key(|(key, _)| *key);
        Self { generation, slots }
    }

    pub(super) fn publish(&self, mix: &MixEngine) {
        for ((track, slot), value) in &self.slots {
            let state = match mix.parameter_delivery(*track, *slot) {
                ParameterDelivery::Pending => 0,
                ParameterDelivery::Ready => 1,
                ParameterDelivery::Failed => 2,
            };
            value.store(state, Ordering::Release);
        }
    }

    pub(super) fn get(&self, generation: PlanGeneration, track: TrackId, slot: usize) -> ParameterDelivery {
        if generation == PlanGeneration::EMPTY || generation != self.generation {
            return ParameterDelivery::Pending;
        }
        let value = self
            .slots
            .binary_search_by_key(&(track, slot), |(key, _)| *key)
            .ok()
            .and_then(|i| self.slots.get(i))
            .map_or(0, |(_, value)| value.load(Ordering::Acquire));
        match value {
            1 => ParameterDelivery::Ready,
            2 => ParameterDelivery::Failed,
            _ => ParameterDelivery::Pending,
        }
    }

    pub(super) fn all(&self, generation: PlanGeneration) -> bool {
        generation != PlanGeneration::EMPTY && generation == self.generation && self.slots.iter().all(|(_, value)| value.load(Ordering::Acquire) == 1)
    }
}
