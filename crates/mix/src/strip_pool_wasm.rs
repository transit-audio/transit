//! wasm32 has no render workers: the type exists so the engine's code is the same on every
//! target, and no value of it can be made.

use super::*;

pub(crate) const PENDING_SEEK: u8 = 1;
pub(crate) const PENDING_RESET: u8 = 2;
pub(crate) const PENDING_NOTES_OFF: u8 = 4;
pub(crate) const PENDING_TRANSPORT: u8 = 8;
pub(crate) const PENDING_REVOKE: u8 = 16;

/// No render workers on this target.
pub(crate) enum StripPool {}

/// Reads an engine's [`StripFaults`]; on this target there is never one to read.
#[derive(Clone)]
pub struct StripFaultsReader(());

impl StripFaultsReader {
    /// The figures now (always zero here).
    #[must_use]
    pub fn snapshot(&self) -> StripFaults {
        StripFaults::default()
    }
}

/// The same fields as the native one, so `render_active` builds it on every target; never read
/// here (there is no pool to hand it to).
#[allow(dead_code)]
pub(crate) struct PoolBlock<'a> {
    pub(crate) session: &'a Arc<Session>,
    pub(crate) busses: &'a BusTable,
    pub(crate) keys: &'a KeyBank,
    pub(crate) live: &'a LiveInput<'a>,
    pub(crate) pos: Samples,
    pub(crate) frames: usize,
    pub(crate) any_solo: bool,
    pub(crate) sample_rate: u32,
    pub(crate) start: std::time::Instant,
    pub(crate) budget: Option<std::time::Duration>,
    pub(crate) deadline: Option<std::time::Instant>,
}

impl StripPool {
    pub(crate) fn workers(&self) -> usize {
        match *self {}
    }
    pub(crate) fn seed_costs(&mut self, _cost: std::time::Duration) {
        match *self {}
    }
    pub(crate) fn promotions(&self) -> Vec<Option<transit_rt::Promotion>> {
        match *self {}
    }
    pub(crate) fn reader(&self) -> StripFaultsReader {
        match *self {}
    }
    pub(crate) fn is_away(&self, _i: usize) -> bool {
        match *self {}
    }
    pub(crate) fn any_away(&self) -> bool {
        match *self {}
    }
    pub(crate) fn mark_away(&mut self, _flag: u8) {
        match *self {}
    }
    pub(crate) fn last_overruns(&self) -> u32 {
        match *self {}
    }
    pub(crate) fn is_silent(&self, _i: usize) -> bool {
        match *self {}
    }
    pub(crate) fn park(
        &mut self,
        _i: usize,
        _ev: &InstrumentEvent,
        _events: &events::EngineEvents,
        _block_len: usize,
        _home: Option<Option<SlotGen>>,
    ) -> (EventOutcome, Option<u64>) {
        match *self {}
    }
    pub(crate) fn deliver_home(&mut self, _strips: &mut [Strip], _events: &mut events::EngineEvents, _s: &Session, _frame: u64, _frames: usize) {
        match *self {}
    }
    pub(crate) fn settle(&mut self, _strips: &mut [Strip], _events: &mut events::EngineEvents, _s: &Session, _block_len: usize) -> bool {
        match *self {}
    }
    pub(crate) fn parks(&self, _i: usize) -> bool {
        match *self {}
    }
    pub(crate) fn flush_parked(&mut self, _strips: &mut [Strip], _events: &mut events::EngineEvents, _s: &Session, _block_len: usize) {
        match *self {}
    }
    pub(crate) fn discard_parked(&mut self) {
        match *self {}
    }
    pub(crate) fn render(&mut self, _b: &PoolBlock<'_>, _p1: &mut [Strip], _events: &mut events::EngineEvents) {
        match *self {}
    }
    pub(crate) fn retire(self, _strips: &mut [Strip], _session: &Session) {
        match self {}
    }
}
