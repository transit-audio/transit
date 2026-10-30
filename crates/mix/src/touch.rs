//! Touch mode (#188): a held control's lane is not read, and on release the parameter returns to
//! its lane over [`TOUCH_RETURN_S`].
//!
//! Logic and Ableton agree: in Touch, while the user holds a control the lane for that parameter is
//! not read (the held value wins), and on release the parameter goes back to the lane, Logic over
//! its Ramp Time (`docs/contributing/knowledge/automation-modes.md`). The UI says which controls are
//! held through the session's runtime `Mixer::touched` (it reaches the audio thread with the
//! session it already publishes for every control move); the mix decides the rest here.
//!
//! A strip keeps a fixed table of the parameters it holds or returns ([`TouchRamps`]), advanced once
//! per block on the audio thread before anything reads automation ([`TouchRamps::begin`]). A
//! release seen at a block's start ramps from that block's first sample: linear from the held
//! value (the control's static value, which the hold left there) to the lane's value, weight 0 at
//! the first sample and the lane's own value [`TOUCH_RETURN_S`] of rendered samples later.
//! Allocation-free and lock-free: plain `Copy` slots, string compares and a hash of the parameter.

use transit_model::{AutoParam, AutomationLane, AutomationMode, Track};
use transit_time::Samples;

/// How long a released control takes to return to its lane in Touch, in seconds. Logic's
/// Automation settings call it Ramp Time; transit has no setting for it yet, so this is the
/// constant (#223 exposes it as an Automation preference). 200 ms: long enough that the
/// return does not click, short enough that the lane is back before the next phrase.
pub const TOUCH_RETURN_S: f64 = 0.2;

/// The return ramp's length in rendered samples at `rate` Hz.
pub(crate) fn return_samples(rate: u32) -> u32 {
    (TOUCH_RETURN_S * f64::from(rate)).round() as u32
}

/// Most parameters of one track held or returning at once. Past it a parameter is still not read
/// while held (that reads `Mixer::touched` directly), but returns without a ramp.
const TOUCH_SLOTS: usize = 8;

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// [`param_key`] of the parameter; 0 is a free slot.
    key: u64,
    held: bool,
    /// Ramp samples left at the current block's first sample (0 while held).
    left: u32,
}

/// One strip's held and returning parameters (see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TouchRamps {
    slots: [Slot; TOUCH_SLOTS],
    /// The ramp length of the current block, in samples.
    len: u32,
    /// The frames of the block `begin` last started (what the next one advances by).
    last: u32,
}

impl TouchRamps {
    /// Audio thread, once at the start of each block of `frames`, before anything reads `t`'s
    /// automation: advances running ramps by the previous block, starts one (of `len` samples) for
    /// each parameter released since, and notes the parameters held now. A parameter held again
    /// while it returns is held at once.
    pub(crate) fn begin(&mut self, t: &Track, frames: usize, len: u32) {
        let last = std::mem::replace(&mut self.last, u32::try_from(frames).unwrap_or(u32::MAX));
        self.len = len;
        let touch = t.mixer.automation_mode == AutomationMode::Touch;
        let holds = |key: u64| touch && t.mixer.touched.iter().any(|p| param_key(p) == key);
        for s in self.slots.iter_mut().filter(|s| s.key != 0) {
            if s.held {
                if !holds(s.key) {
                    *s = Slot { key: s.key, held: false, left: len };
                }
            } else {
                s.left = s.left.saturating_sub(last);
            }
            if !s.held && s.left == 0 {
                *s = Slot::default();
            }
        }
        if !touch {
            return;
        }
        for p in &t.mixer.touched {
            let key = param_key(p);
            if let Some(s) = self.slots.iter_mut().find(|s| s.key == key) {
                *s = Slot { key, held: true, left: 0 };
            } else if let Some(s) = self.slots.iter_mut().find(|s| s.key == 0) {
                *s = Slot { key, held: true, left: 0 };
            }
        }
    }

    /// The lane's weight `offset` frames into the block for a parameter returning to it (0 at the
    /// release's first sample, rising linearly), or `None` when it is not returning: the lane
    /// applies as it is.
    fn weight(&self, key: u64, offset: usize) -> Option<f32> {
        let s = self.slots.iter().find(|s| s.key == key && !s.held)?;
        let left = s.left.saturating_sub(u32::try_from(offset).unwrap_or(u32::MAX));
        (left > 0 && self.len > 0).then(|| (1.0 - f64::from(left) / f64::from(self.len)) as f32)
    }

    /// Ramp samples left at `offset` frames into the block for `key` (0: not returning).
    fn left(&self, key: u64, offset: usize) -> u32 {
        self.slots.iter().find(|s| s.key == key && !s.held).map_or(0, |s| s.left.saturating_sub(u32::try_from(offset).unwrap_or(u32::MAX)))
    }
}

/// Whether `t` holds the parameter `is` picks, in Touch: its lane is not read.
pub(crate) fn held(t: &Track, is: impl Fn(&AutoParam) -> bool) -> bool {
    t.mixer.automation_mode == AutomationMode::Touch && t.mixer.touched.iter().any(is)
}

/// What a reader applies for `param` of `t` `offset` frames into the block, at timeline position
/// `at`: the static `base` while the control is held, the lane's value otherwise, eased in from
/// `base` while the parameter returns. `ramps` is the track's own strip table (`None` reads
/// without a return ramp: a VCA master read from a member strip).
pub(crate) fn lane_value(
    t: &Track,
    ramps: Option<&TouchRamps>,
    param: &AutoParam,
    lane: &AutomationLane,
    base: f32,
    at: Samples,
    offset: usize,
) -> f32 {
    if held(t, |p| p == param) {
        return base;
    }
    let v = lane.value_at(at, base);
    match ramps.and_then(|r| r.weight(param_key(param), offset)) {
        Some(w) => base + (v - base) * w,
        None => v,
    }
}

/// [`lane_value`] for an insert or instrument parameter (`slot`, `name`), without building an
/// [`AutoParam`] (that allocates). Also returns the ramp samples left at `offset` (0 when not
/// returning) and whether it is held, so the caller holds back the lane's breakpoints until the
/// ramp is over.
pub(crate) fn plugin_value(t: &Track, ramps: &TouchRamps, slot: u8, name: &str, lane: &AutomationLane, base: f32, at: Samples) -> (f32, u32, bool) {
    if held(t, |p| matches!(p, AutoParam::Plugin { slot: s, param } if *s == slot && param == name)) {
        return (base, 0, true);
    }
    let v = lane.value_at(at, base);
    let key = plugin_key(slot, name);
    match ramps.weight(key, 0) {
        Some(w) => (base + (v - base) * w, ramps.left(key, 0), false),
        None => (v, 0, false),
    }
}

/// A stable, allocation-free key for a parameter (FNV-1a over its kind, index and name); never 0.
pub(crate) fn param_key(p: &AutoParam) -> u64 {
    match p {
        AutoParam::Volume => key_of(1, 0, ""),
        AutoParam::Pan(i) => key_of(2, *i, ""),
        AutoParam::Mute => key_of(3, 0, ""),
        AutoParam::SendLevel(i) => key_of(4, *i, ""),
        AutoParam::SendPan(i) => key_of(5, *i, ""),
        AutoParam::SendMute(i) => key_of(6, *i, ""),
        AutoParam::Plugin { slot, param } => plugin_key(*slot, param),
    }
}

fn plugin_key(slot: u8, name: &str) -> u64 {
    key_of(7, slot, name)
}

fn key_of(kind: u8, index: u8, name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in [kind, index].into_iter().chain(name.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.max(1)
}
